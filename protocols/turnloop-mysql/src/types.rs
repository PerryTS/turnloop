//! mysql2 conversion policy, independent of JS allocation, timezone databases or
//! a clock. `Json` and `Date` explicitly request host-side JS construction.
use crate::{ColumnTypeInfo, Error, RawValue, Result, Value};
use std::borrow::Cow;
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub decimal_numbers: bool,
    pub support_big_numbers: bool,
    pub big_number_strings: bool,
    pub date_strings: bool,
    pub json_strings: bool,
    /// mysql2's binary-protocol DATETIME/TIMESTAMP string policy: cut the
    /// fractional seconds of a formatted `date_strings` value to the column's
    /// declared `decimals` (DATETIME(3) gives `.123`, not `.123000`). Without
    /// it all six digits are kept. Text-protocol values already arrive with the
    /// declared digits and are returned verbatim either way.
    pub truncate_fraction_to_decimals: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Date<'a> {
    Text(&'a str),
    Calendar {
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        microsecond: u32,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub enum JsValue<'a> {
    Null,
    Number(f64),
    String(Cow<'a, str>),
    Buffer(Cow<'a, [u8]>),
    Json(&'a str),
    Date(Date<'a>),
    /// A GEOMETRY value (4-byte little-endian SRID, then WKB). mysql2 turns it
    /// into point/array objects; like `Json` and `Date` that is host work.
    Geometry(&'a [u8]),
}
fn utf8(b: &[u8]) -> Result<&str> {
    std::str::from_utf8(b)
        .map_err(|_| Error::Protocol("non-UTF8 text; decode with the column charset in the host"))
}
fn parse<T: std::str::FromStr>(b: &[u8]) -> Result<T> {
    utf8(b)?
        .parse()
        .map_err(|_| Error::Protocol("invalid numeric value"))
}
fn bigint(value: i128, options: Options) -> JsValue<'static> {
    if options.support_big_numbers
        && (options.big_number_strings
            || !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value))
    {
        JsValue::String(value.to_string().into())
    } else {
        JsValue::Number(value as f64)
    }
}
/// Invoke this default conversion after a host typeCast hook chooses `next()`;
/// raw fields and Column metadata are also available without conversion
/// (`Row::typed` pairs every value with its `ColumnTypeInfo`).
///
/// The policy is chosen by MySQL column type first. `character_set == 63`
/// (binary) is reported for every non-string column, so it only selects Buffer
/// for the string/BLOB family; TIME stays a string, BIT a Buffer and GEOMETRY a
/// `Geometry` request, matching mysql2.
pub fn decode<'a>(
    info: ColumnTypeInfo,
    value: RawValue<'a>,
    options: Options,
) -> Result<JsValue<'a>> {
    let kind = info.column_type as u8;
    Ok(match value {
        RawValue::Null => JsValue::Null,
        RawValue::Bytes(bytes) => match kind {
            0 | 246 => {
                if options.decimal_numbers {
                    JsValue::Number(parse(bytes)?)
                } else {
                    JsValue::String(utf8(bytes)?.into())
                }
            }
            1..=5 | 9 | 13 => JsValue::Number(parse(bytes)?),
            8 => {
                if options.support_big_numbers {
                    let value: i128 = parse(bytes)?;
                    if options.big_number_strings
                        || !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&value)
                    {
                        JsValue::String(utf8(bytes)?.into())
                    } else {
                        JsValue::Number(value as f64)
                    }
                } else {
                    JsValue::Number(parse(bytes)?)
                }
            }
            7 | 10 | 12 | 14 => {
                if options.date_strings {
                    JsValue::String(utf8(bytes)?.into())
                } else {
                    JsValue::Date(Date::Text(utf8(bytes)?))
                }
            }
            245 => {
                if options.json_strings {
                    JsValue::String(utf8(bytes)?.into())
                } else {
                    JsValue::Json(utf8(bytes)?)
                }
            }
            11 => JsValue::String(utf8(bytes)?.into()),
            16 => JsValue::Buffer(bytes.into()),
            255 => JsValue::Geometry(bytes),
            _ if info.character_set == 63 => JsValue::Buffer(bytes.into()),
            _ => JsValue::String(utf8(bytes)?.into()),
        },
        RawValue::Scalar(Value::NULL) => JsValue::Null,
        RawValue::Scalar(Value::Int(n)) => {
            if kind == 8 {
                bigint(n as i128, options)
            } else {
                JsValue::Number(n as f64)
            }
        }
        RawValue::Scalar(Value::UInt(n)) => {
            if kind == 8 {
                bigint(n as i128, options)
            } else {
                JsValue::Number(n as f64)
            }
        }
        RawValue::Scalar(Value::Float(n)) => JsValue::Number(n as f64),
        RawValue::Scalar(Value::Double(n)) => JsValue::Number(n),
        RawValue::Scalar(Value::Bytes(_)) => {
            return Err(Error::State("use borrowed bytes for a wire field"));
        }
        RawValue::Scalar(Value::Date(year, month, day, hour, minute, second, microsecond)) => {
            if options.date_strings {
                let s = if kind == 10 {
                    format!("{year:04}-{month:02}-{day:02}")
                } else {
                    let mut s =
                        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
                    let digits = if options.truncate_fraction_to_decimals {
                        usize::from(info.decimals.min(6))
                    } else {
                        6
                    };
                    if microsecond != 0 && digits > 0 {
                        let fraction = format!("{microsecond:06}");
                        s.push('.');
                        s.push_str(&fraction[..digits]);
                    }
                    s
                };
                JsValue::String(s.into())
            } else {
                JsValue::Date(Date::Calendar {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    microsecond,
                })
            }
        }
        RawValue::Scalar(Value::Time(negative, days, hour, minute, second, microsecond)) => {
            let hour = days as u64 * 24 + hour as u64;
            let mut s = format!(
                "{}{hour:02}:{minute:02}:{second:02}",
                if negative { "-" } else { "" }
            );
            if microsecond != 0 {
                s.push_str(&format!(".{microsecond:06}"));
            }
            JsValue::String(s.into())
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColumnFlags, ColumnType};
    fn info(t: ColumnType) -> ColumnTypeInfo {
        ColumnTypeInfo {
            column_type: t,
            flags: ColumnFlags::empty(),
            character_set: 45,
            decimals: 0,
        }
    }
    /// MySQL reports charset 63 for every non-string column; the column type,
    /// not the charset, must pick TIME/BIT/GEOMETRY's policy.
    #[test]
    fn binary_charset_does_not_collapse_time_bit_and_geometry() {
        let binary = |t| ColumnTypeInfo {
            character_set: 63,
            ..info(t)
        };
        let decode = |t, bytes| decode(binary(t), RawValue::Bytes(bytes), Options::default());
        assert_eq!(
            decode(ColumnType::MYSQL_TYPE_TIME, b"-838:59:59.5"),
            Ok(JsValue::String("-838:59:59.5".into()))
        );
        assert_eq!(
            decode(ColumnType::MYSQL_TYPE_BIT, &[0b101]),
            Ok(JsValue::Buffer(Cow::Borrowed(&[0b101])))
        );
        let point = [0, 0, 0, 0, 1, 1, 0, 0, 0];
        assert_eq!(
            decode(ColumnType::MYSQL_TYPE_GEOMETRY, &point),
            Ok(JsValue::Geometry(&point))
        );
        assert_eq!(
            decode(ColumnType::MYSQL_TYPE_VAR_STRING, b"\xff\x00"),
            Ok(JsValue::Buffer(Cow::Borrowed(b"\xff\x00"))),
            "VARBINARY still follows the binary charset"
        );
    }
    #[test]
    fn datetime_fraction_follows_the_column_decimals_on_request() {
        let value = || RawValue::Scalar(Value::Date(2026, 9, 22, 10, 11, 12, 123_456));
        let datetime = |decimals| ColumnTypeInfo {
            decimals,
            ..info(ColumnType::MYSQL_TYPE_DATETIME)
        };
        let strings = Options {
            date_strings: true,
            ..Options::default()
        };
        let truncated = Options {
            truncate_fraction_to_decimals: true,
            ..strings
        };
        assert_eq!(
            decode(datetime(3), value(), strings),
            Ok(JsValue::String("2026-09-22 10:11:12.123456".into()))
        );
        assert_eq!(
            decode(datetime(3), value(), truncated),
            Ok(JsValue::String("2026-09-22 10:11:12.123".into()))
        );
        assert_eq!(
            decode(datetime(6), value(), truncated),
            Ok(JsValue::String("2026-09-22 10:11:12.123456".into()))
        );
        assert_eq!(
            decode(datetime(0), value(), truncated),
            Ok(JsValue::String("2026-09-22 10:11:12".into()))
        );
        assert_eq!(
            decode(
                ColumnTypeInfo {
                    decimals: 2,
                    ..info(ColumnType::MYSQL_TYPE_TIMESTAMP)
                },
                value(),
                truncated
            ),
            Ok(JsValue::String("2026-09-22 10:11:12.12".into()))
        );
    }
    #[test]
    fn node_conversion_options() {
        let decimal = info(ColumnType::MYSQL_TYPE_NEWDECIMAL);
        assert_eq!(
            decode(decimal, RawValue::Bytes(b"1.2300"), Options::default()).unwrap(),
            JsValue::String("1.2300".into())
        );
        let big = info(ColumnType::MYSQL_TYPE_LONGLONG);
        assert_eq!(
            decode(
                big,
                RawValue::Bytes(b"18446744073709551615"),
                Options {
                    support_big_numbers: true,
                    ..Options::default()
                }
            )
            .unwrap(),
            JsValue::String("18446744073709551615".into())
        );
        assert_eq!(
            decode(
                info(ColumnType::MYSQL_TYPE_TINY),
                RawValue::Bytes(b"1"),
                Options::default()
            )
            .unwrap(),
            JsValue::Number(1.)
        );
        assert_eq!(
            decode(
                info(ColumnType::MYSQL_TYPE_JSON),
                RawValue::Bytes(b"{\"x\":1}"),
                Options::default()
            )
            .unwrap(),
            JsValue::Json("{\"x\":1}")
        );
        let mut blob = info(ColumnType::MYSQL_TYPE_BLOB);
        blob.character_set = 63;
        assert_eq!(
            decode(blob, RawValue::Bytes(&[0, 255]), Options::default()).unwrap(),
            JsValue::Buffer(Cow::Borrowed(&[0, 255]))
        );
        assert_eq!(
            decode(
                info(ColumnType::MYSQL_TYPE_DATE),
                RawValue::Scalar(Value::Date(2026, 9, 14, 0, 0, 0, 0)),
                Options {
                    date_strings: true,
                    ..Options::default()
                }
            )
            .unwrap(),
            JsValue::String("2026-09-14".into())
        );
    }
}
