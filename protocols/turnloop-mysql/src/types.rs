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
/// raw fields and Column metadata are also available without conversion.
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
            16 | 255 => JsValue::Buffer(bytes.into()),
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
                    if microsecond != 0 {
                        s.push_str(&format!(".{microsecond:06}"));
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
        }
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
