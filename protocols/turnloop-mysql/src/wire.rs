use crate::{Error, Result};
pub use mysql_common::value::Value;
use mysql_common::{
    constants::{ColumnFlags, ColumnType},
    io::ParseBuf,
    proto::MyDeserialize,
    value::{BinValue, ValueDeserializer},
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cursor<'a>(pub &'a [u8]);
impl<'a> Cursor<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(Error::Protocol("truncated packet"));
        }
        let (v, r) = self.0.split_at(n);
        self.0 = r;
        Ok(v)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn lenenc(&mut self) -> Result<u64> {
        match self.u8()? {
            0xfc => Ok(self.u16()? as u64),
            0xfd => {
                let b = self.take(3)?;
                Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]) as u64)
            }
            0xfe => Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            0xfb | 0xff => Err(Error::Protocol("invalid length encoded integer")),
            n => Ok(n as u64),
        }
    }
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = usize::try_from(self.lenenc()?).map_err(|_| Error::Limit)?;
        self.take(n)
    }
    pub fn end(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Protocol("trailing packet bytes"))
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnTypeInfo {
    pub column_type: ColumnType,
    pub flags: ColumnFlags,
    pub character_set: u16,
}
#[derive(Debug, Clone, Copy)]
pub struct Column<'a> {
    pub schema: &'a [u8],
    pub table: &'a [u8],
    pub original_table: &'a [u8],
    pub name: &'a [u8],
    pub original_name: &'a [u8],
    pub column_length: u32,
    pub decimals: u8,
    pub type_info: ColumnTypeInfo,
}
impl<'a> Column<'a> {
    pub(crate) fn parse(b: &'a [u8]) -> Result<Self> {
        let mut c = Cursor(b);
        if c.bytes()? != b"def" {
            return Err(Error::Protocol("invalid column catalog"));
        }
        let schema = c.bytes()?;
        let table = c.bytes()?;
        let original_table = c.bytes()?;
        let name = c.bytes()?;
        let original_name = c.bytes()?;
        if c.lenenc()? != 12 {
            return Err(Error::Protocol("invalid column fixed length"));
        }
        let character_set = c.u16()?;
        let column_length = c.u32()?;
        let column_type = ColumnType::try_from(c.u8()?)
            .map_err(|_| Error::Protocol("unsupported column type"))?;
        let flags = ColumnFlags::from_bits_truncate(c.u16()?);
        let decimals = c.u8()?;
        c.take(2)?;
        c.end()?;
        Ok(Self {
            schema,
            table,
            original_table,
            name,
            original_name,
            column_length,
            decimals,
            type_info: ColumnTypeInfo {
                column_type,
                flags,
                character_set,
            },
        })
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum RawValue<'a> {
    Null,
    Bytes(&'a [u8]),
    Scalar(Value),
}
/// One row packet, decoded lazily and validated as it is iterated.
///
/// `Row::parse` checks the header (binary marker and NULL bitmap) eagerly; each
/// value is bounds-checked and decoded exactly once as the iterator reaches it,
/// and the final value also rejects trailing packet bytes. The first `Err` ends
/// the iteration (every later `next` is `None`), so nothing past a malformed
/// value is ever yielded. A row error means the server sent a malformed packet:
/// treat it like any other parsing error and `abort` the connection.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    cursor: Cursor<'a>,
    columns: &'a [ColumnTypeInfo],
    bitmap: &'a [u8],
    binary: bool,
    at: usize,
}
impl<'a> Row<'a> {
    pub(crate) fn parse(
        bytes: &'a [u8],
        columns: &'a [ColumnTypeInfo],
        binary: bool,
    ) -> Result<Self> {
        let mut cursor = Cursor(bytes);
        let bitmap = if binary {
            if cursor.u8()? != 0 {
                return Err(Error::Protocol("invalid binary row marker"));
            }
            cursor.take((columns.len() + 2).div_ceil(8))?
        } else {
            &[]
        };
        Ok(Self {
            cursor,
            columns,
            bitmap,
            binary,
            at: 0,
        })
    }
    fn value(&mut self, index: usize) -> Result<RawValue<'a>> {
        if !self.binary {
            if self.cursor.0.first() == Some(&0xfb) {
                self.cursor.take(1)?;
                return Ok(RawValue::Null);
            }
            return Ok(RawValue::Bytes(self.cursor.bytes()?));
        }
        if self.bitmap[(index + 2) / 8] & (1 << ((index + 2) % 8)) != 0 {
            return Ok(RawValue::Null);
        }
        let info = self.columns[index];
        let t = info.column_type as u8;
        if matches!(t, 0 | 15 | 16 | 245..=255 | 242) {
            return Ok(RawValue::Bytes(self.cursor.bytes()?));
        }
        if !matches!(t, 1..=14) {
            return Err(Error::Protocol("unsupported binary column type"));
        }
        // Numeric/calendar scalars from mysql_common contain no heap buffers.
        let mut b = ParseBuf(self.cursor.0);
        let value =
            ValueDeserializer::<BinValue>::deserialize((info.column_type, info.flags), &mut b)
                .map_err(|_| Error::Protocol("invalid binary scalar"))?
                .0;
        self.cursor.0 = b.0;
        Ok(if value == Value::NULL {
            RawValue::Null
        } else {
            RawValue::Scalar(value)
        })
    }
}
impl<'a> Iterator for Row<'a> {
    type Item = Result<RawValue<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.at == self.columns.len() {
            return None;
        }
        let i = self.at;
        self.at += 1;
        let value = self.value(i).and_then(|value| {
            if self.at == self.columns.len() {
                self.cursor.end()?;
            }
            Ok(value)
        });
        if value.is_err() {
            // Fuse: never decode past a malformed value.
            self.at = self.columns.len();
        }
        Some(value)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.columns.len() - self.at;
        (n, Some(n))
    }
}
impl ExactSizeIterator for Row<'_> {}

#[derive(Debug, Clone, Copy)]
pub struct ServerError<'a> {
    pub errno: u16,
    pub sql_state: &'a str,
    pub sql_message: &'a str,
}
impl<'a> ServerError<'a> {
    pub(crate) fn parse(b: &'a [u8]) -> Result<Self> {
        let mut c = Cursor(b);
        if c.u8()? != 0xff {
            return Err(Error::Protocol("invalid ERR marker"));
        }
        let errno = c.u16()?;
        let sql_state = if c.0.first() == Some(&b'#') {
            c.u8()?;
            std::str::from_utf8(c.take(5)?).map_err(|_| Error::Protocol("invalid SQLSTATE"))?
        } else {
            "HY000"
        };
        let sql_message =
            std::str::from_utf8(c.0).map_err(|_| Error::Protocol("invalid error encoding"))?;
        Ok(Self {
            errno,
            sql_state,
            sql_message,
        })
    }
    pub fn code(&self) -> Option<&'static str> {
        error_code(self.errno)
    }
}
pub fn error_code(errno: u16) -> Option<&'static str> {
    Some(match errno {
        1022 => "ER_DUP_KEY",
        1040 => "ER_CON_COUNT_ERROR",
        1044 => "ER_DBACCESS_DENIED_ERROR",
        1045 => "ER_ACCESS_DENIED_ERROR",
        1048 => "ER_BAD_NULL_ERROR",
        1049 => "ER_BAD_DB_ERROR",
        1050 => "ER_TABLE_EXISTS_ERROR",
        1051 => "ER_BAD_TABLE_ERROR",
        1052 => "ER_NON_UNIQ_ERROR",
        1054 => "ER_BAD_FIELD_ERROR",
        1062 => "ER_DUP_ENTRY",
        1064 => "ER_PARSE_ERROR",
        1146 => "ER_NO_SUCH_TABLE",
        1169 => "ER_DUP_UNIQUE",
        1205 => "ER_LOCK_WAIT_TIMEOUT",
        1213 => "ER_LOCK_DEADLOCK",
        1216 => "ER_NO_REFERENCED_ROW",
        1217 => "ER_ROW_IS_REFERENCED",
        1264 => "ER_WARN_DATA_OUT_OF_RANGE",
        1292 => "ER_TRUNCATED_WRONG_VALUE",
        1364 => "ER_NO_DEFAULT_FOR_FIELD",
        1406 => "ER_DATA_TOO_LONG",
        1451 => "ER_ROW_IS_REFERENCED_2",
        1452 => "ER_NO_REFERENCED_ROW_2",
        1524 => "ER_PLUGIN_IS_NOT_LOADED",
        1586 => "ER_DUP_ENTRY_WITH_KEY_NAME",
        1830 => "ER_FK_COLUMN_NOT_NULL",
        1834 => "ER_FK_CANNOT_DELETE_PARENT",
        1859 => "ER_DUP_UNKNOWN_IN_INDEX",
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn info(column_type: ColumnType) -> ColumnTypeInfo {
        ColumnTypeInfo {
            column_type,
            flags: ColumnFlags::empty(),
            character_set: 45,
        }
    }
    fn text() -> [ColumnTypeInfo; 2] {
        [info(ColumnType::MYSQL_TYPE_VAR_STRING); 2]
    }
    #[test]
    fn a_valid_row_is_decoded_in_one_pass() {
        let columns = text();
        let mut row = Row::parse(b"\x02ab\xfb", &columns, false).expect("valid header");
        assert_eq!(row.len(), 2);
        assert_eq!(row.next(), Some(Ok(RawValue::Bytes(b"ab"))));
        assert_eq!(row.len(), 1);
        assert_eq!(row.next(), Some(Ok(RawValue::Null)));
        assert_eq!(row.next(), None);
    }
    /// Values are validated as they are reached; nothing after the first error
    /// is decoded or yielded.
    #[test]
    fn a_malformed_value_ends_the_row() {
        let columns = text();
        // Second value claims 9 bytes but only 2 remain.
        let mut row = Row::parse(b"\x02ab\x09cd", &columns, false)
            .expect("parse no longer decodes values ahead of the host");
        assert_eq!(row.next(), Some(Ok(RawValue::Bytes(b"ab"))));
        assert_eq!(row.next(), Some(Err(Error::Protocol("truncated packet"))));
        assert_eq!(row.len(), 0);
        assert_eq!(row.next(), None);
    }
    #[test]
    fn trailing_bytes_fail_the_last_value() {
        let columns = text();
        let mut row = Row::parse(b"\x02ab\x01c!", &columns, false).expect("valid header");
        assert_eq!(row.next(), Some(Ok(RawValue::Bytes(b"ab"))));
        assert_eq!(
            row.next(),
            Some(Err(Error::Protocol("trailing packet bytes")))
        );
        assert_eq!(row.next(), None);
    }
    #[test]
    fn binary_rows_validate_the_header_eagerly_and_scalars_lazily() {
        let long = [info(ColumnType::MYSQL_TYPE_LONG)];
        assert_eq!(
            Row::parse(&[1, 0, 42, 0, 0, 0], &long, true).err(),
            Some(Error::Protocol("invalid binary row marker"))
        );
        assert_eq!(
            Row::parse(&[0], &long, true).err(),
            Some(Error::Protocol("truncated packet"))
        );
        let mut row = Row::parse(&[0, 0, 42, 0, 0, 0], &long, true).expect("valid row");
        assert_eq!(row.next(), Some(Ok(RawValue::Scalar(Value::Int(42)))));
        assert_eq!(row.next(), None);
        // A LONG needs four bytes; two are present.
        let mut row = Row::parse(&[0, 0, 42, 0], &long, true).expect("valid header");
        assert_eq!(
            row.next(),
            Some(Err(Error::Protocol("invalid binary scalar")))
        );
        assert_eq!(row.next(), None);
    }
}
