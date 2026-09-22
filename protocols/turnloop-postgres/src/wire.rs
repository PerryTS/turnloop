use crate::{Error, Result};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cursor<'a>(pub &'a [u8]);
impl<'a> Cursor<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(Error::Protocol("truncated message"));
        }
        let (v, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(v)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn cstr(&mut self) -> Result<&'a str> {
        let n = self
            .0
            .iter()
            .position(|b| *b == 0)
            .ok_or(Error::Protocol("missing terminator"))?;
        let s = self.take(n + 1)?;
        std::str::from_utf8(&s[..n]).map_err(|_| Error::Protocol("invalid UTF-8"))
    }
    pub fn end(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Protocol("trailing message bytes"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field<'a> {
    pub name: &'a str,
    pub table_id: u32,
    pub column_id: i16,
    pub data_type_id: u32,
    pub data_type_size: i16,
    pub data_type_modifier: i32,
    pub format: i16,
}
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    cursor: Cursor<'a>,
    remaining: u16,
}
impl<'a> Fields<'a> {
    pub(crate) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut cursor = Cursor(body);
        let remaining = cursor.u16()?;
        let fields = Self { cursor, remaining };
        let mut check = fields;
        for field in &mut check {
            field?;
        }
        check.cursor.end()?;
        Ok(fields)
    }
}
impl<'a> Iterator for Fields<'a> {
    type Item = Result<Field<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some((|| {
            Ok(Field {
                name: self.cursor.cstr()?,
                table_id: self.cursor.u32()?,
                column_id: self.cursor.i16()?,
                data_type_id: self.cursor.u32()?,
                data_type_size: self.cursor.i16()?,
                data_type_modifier: self.cursor.i32()?,
                format: self.cursor.i16()?,
            })
        })())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining as usize, Some(self.remaining as usize))
    }
}
impl ExactSizeIterator for Fields<'_> {}
impl Fields<'_> {
    /// Copy the unread field descriptions out of the connection's buffer.
    /// One allocation; the result outlives every later mutable call.
    pub fn into_owned(self) -> OwnedFields {
        OwnedFields {
            data: self.cursor.0.into(),
            remaining: self.remaining,
        }
    }
}
/// Owned RowDescription. `fields()` iterates it exactly like the borrowed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedFields {
    data: Box<[u8]>,
    remaining: u16,
}
impl OwnedFields {
    pub fn fields(&self) -> Fields<'_> {
        Fields {
            cursor: Cursor(&self.data),
            remaining: self.remaining,
        }
    }
    pub fn len(&self) -> usize {
        self.remaining as usize
    }
    pub fn is_empty(&self) -> bool {
        self.remaining == 0
    }
}

/// Borrowed, already validated PostgreSQL row. Iteration never allocates.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    cursor: Cursor<'a>,
    remaining: u16,
}
impl<'a> Row<'a> {
    pub(crate) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut cursor = Cursor(body);
        let remaining = cursor.u16()?;
        let row = Self { cursor, remaining };
        let mut check = row;
        for value in &mut check {
            value?;
        }
        check.cursor.end()?;
        Ok(row)
    }
}
impl<'a> Iterator for Row<'a> {
    type Item = Result<Option<&'a [u8]>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some((|| {
            let n = self.cursor.i32()?;
            match n {
                -1 => Ok(None),
                0.. => Ok(Some(self.cursor.take(n as usize)?)),
                _ => Err(Error::Protocol("invalid field length")),
            }
        })())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining as usize, Some(self.remaining as usize))
    }
}
impl ExactSizeIterator for Row<'_> {}
impl Row<'_> {
    /// Copy the unread values out of the connection's buffer in one allocation,
    /// so the row can be retained past the next mutable call on the connection.
    pub fn into_owned(self) -> OwnedRow {
        OwnedRow {
            data: self.cursor.0.into(),
            remaining: self.remaining,
        }
    }
}
/// Owned, already validated DataRow. `row()` lends the same borrowed iterator,
/// so one host decoder serves both forms; `get` indexes a column directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedRow {
    data: Box<[u8]>,
    remaining: u16,
}
impl OwnedRow {
    pub fn row(&self) -> Row<'_> {
        Row {
            cursor: Cursor(&self.data),
            remaining: self.remaining,
        }
    }
    /// `None` past the last column; `Some(None)` is SQL NULL.
    pub fn get(&self, index: usize) -> Option<Option<&[u8]>> {
        self.row().nth(index)?.ok()
    }
    pub fn len(&self) -> usize {
        self.remaining as usize
    }
    pub fn is_empty(&self) -> bool {
        self.remaining == 0
    }
}

/// ErrorResponse and NoticeResponse retain every field, including future fields.
#[derive(Clone, Copy, Debug)]
pub struct ServerError<'a>(pub(crate) &'a [u8]);

/// Owned terminal server diagnostic. Clones share one copy of all wire fields.
/// Only connection failure allocates; ordinary statement errors remain borrowed.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionFailure(std::sync::Arc<[u8]>);
impl ConnectionFailure {
    pub(crate) fn new(error: ServerError<'_>) -> Self {
        Self(error.0.into())
    }
    pub fn server_error(&self) -> ServerError<'_> {
        ServerError(&self.0)
    }
}
impl std::fmt::Display for ConnectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error = self.server_error();
        write!(f, "{}: {}", error.code(), error.message())
    }
}
impl std::fmt::Debug for ConnectionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.server_error().fields()).finish()
    }
}
/// Owned ErrorResponse/NoticeResponse with every field, including future ones.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnedServerError(Box<[u8]>);
impl OwnedServerError {
    pub fn server_error(&self) -> ServerError<'_> {
        ServerError(&self.0)
    }
}
impl std::fmt::Display for OwnedServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error = self.server_error();
        write!(f, "{}: {}", error.code(), error.message())
    }
}
impl std::fmt::Debug for OwnedServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.server_error().fields()).finish()
    }
}
impl<'a> ServerError<'a> {
    /// Copy every field out of the connection's buffer in one allocation.
    pub fn into_owned(self) -> OwnedServerError {
        OwnedServerError(self.0.into())
    }
    pub(crate) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut c = Cursor(body);
        while c.u8()? != 0 {
            c.cstr()?;
        }
        c.end()?;
        Ok(Self(body))
    }
    pub fn fields(self) -> impl Iterator<Item = (u8, &'a str)> {
        let mut c = Cursor(self.0);
        std::iter::from_fn(move || {
            let tag = c.u8().ok()?;
            if tag == 0 {
                None
            } else {
                Some((tag, c.cstr().ok()?))
            }
        })
    }
    pub fn get(self, tag: u8) -> Option<&'a str> {
        self.fields().find(|(t, _)| *t == tag).map(|(_, v)| v)
    }
    pub fn code(self) -> &'a str {
        self.get(b'C').unwrap_or("")
    }
    pub fn message(self) -> &'a str {
        self.get(b'M').unwrap_or("")
    }
    pub fn severity(self) -> Option<&'a str> {
        self.get(b'S')
    }
    pub fn detail(self) -> Option<&'a str> {
        self.get(b'D')
    }
    pub fn hint(self) -> Option<&'a str> {
        self.get(b'H')
    }
    pub fn internal_position(self) -> Option<&'a str> {
        self.get(b'p')
    }
    pub fn internal_query(self) -> Option<&'a str> {
        self.get(b'q')
    }
    pub fn context(self) -> Option<&'a str> {
        self.get(b'W')
    }
    pub fn schema(self) -> Option<&'a str> {
        self.get(b's')
    }
    pub fn table(self) -> Option<&'a str> {
        self.get(b't')
    }
    pub fn column(self) -> Option<&'a str> {
        self.get(b'c')
    }
    pub fn data_type(self) -> Option<&'a str> {
        self.get(b'd')
    }
    pub fn constraint(self) -> Option<&'a str> {
        self.get(b'n')
    }
    pub fn file(self) -> Option<&'a str> {
        self.get(b'F')
    }
    pub fn line(self) -> Option<&'a str> {
        self.get(b'L')
    }
    pub fn routine(self) -> Option<&'a str> {
        self.get(b'R')
    }
    pub fn position(self) -> Option<&'a str> {
        self.get(b'P')
    }
}
