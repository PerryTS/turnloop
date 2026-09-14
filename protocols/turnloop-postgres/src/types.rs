//! Common PostgreSQL codecs. `Int8` and `Numeric` remain exact: pg's default
//! JS parsers use strings. JSON parsing and local-time JS Date construction
//! belong to the host; neither reads a clock here.
use crate::{Error, Result, wire::Cursor};
use std::{borrow::Cow, fmt::Write};

#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    Null,
    Bool(bool),
    Int(i32),
    Int8(i64),
    Float(f64),
    Text(Cow<'a, str>),
    Numeric(Cow<'a, str>),
    Bytes(Cow<'a, [u8]>),
    Json(Cow<'a, str>),
    Uuid(Cow<'a, str>),
    /// ISO DateStyle text; preserves infinity and local-time timestamp meaning.
    TemporalText {
        oid: u32,
        text: Cow<'a, str>,
    },
    /// PostgreSQL epoch: days after 2000-01-01. i32 extrema are infinities.
    Date(i32),
    /// Microseconds after 2000-01-01; timestamptz=true means UTC.
    Timestamp {
        microseconds: i64,
        timestamptz: bool,
    },
    Array(Vec<Value<'a>>),
    /// Unknown binary types are deliberately not interpreted as UTF-8.
    Raw {
        oid: u32,
        bytes: Cow<'a, [u8]>,
    },
}
impl Value<'_> {
    pub fn into_owned(self) -> Value<'static> {
        match self {
            Self::Null => Value::Null,
            Self::Bool(v) => Value::Bool(v),
            Self::Int(v) => Value::Int(v),
            Self::Int8(v) => Value::Int8(v),
            Self::Float(v) => Value::Float(v),
            Self::Text(v) => Value::Text(Cow::Owned(v.into_owned())),
            Self::Numeric(v) => Value::Numeric(Cow::Owned(v.into_owned())),
            Self::Bytes(v) => Value::Bytes(Cow::Owned(v.into_owned())),
            Self::Json(v) => Value::Json(Cow::Owned(v.into_owned())),
            Self::Uuid(v) => Value::Uuid(Cow::Owned(v.into_owned())),
            Self::TemporalText { oid, text } => Value::TemporalText {
                oid,
                text: Cow::Owned(text.into_owned()),
            },
            Self::Date(v) => Value::Date(v),
            Self::Timestamp {
                microseconds,
                timestamptz,
            } => Value::Timestamp {
                microseconds,
                timestamptz,
            },
            Self::Array(v) => Value::Array(v.into_iter().map(Value::into_owned).collect()),
            Self::Raw { oid, bytes } => Value::Raw {
                oid,
                bytes: Cow::Owned(bytes.into_owned()),
            },
        }
    }
}
fn text(b: &[u8]) -> Result<&str> {
    std::str::from_utf8(b).map_err(|_| Error::Protocol("invalid UTF-8 value"))
}
fn number<T: std::str::FromStr>(s: &str) -> Result<T> {
    s.parse()
        .map_err(|_| Error::Protocol("invalid numeric value"))
}
fn array_element(oid: u32) -> Option<u32> {
    Some(match oid {
        1000 => 16,
        1001 => 17,
        1005 => 21,
        1007 => 23,
        1009 => 25,
        1014 => 1042,
        1015 => 1043,
        1016 => 20,
        1021 => 700,
        1022 => 701,
        1115 => 1114,
        1182 => 1082,
        1185 => 1184,
        1231 => 1700,
        199 => 114,
        2951 => 2950,
        3807 => 3802,
        _ => return None,
    })
}
/// Decode one field. Borrowed text/binary rows remain allocation-free; arrays,
/// bytea unescaping, binary numeric and binary UUID allocate their result only.
pub fn decode(oid: u32, format: i16, bytes: Option<&[u8]>) -> Result<Value<'_>> {
    let Some(b) = bytes else {
        return Ok(Value::Null);
    };
    if !matches!(format, 0 | 1) {
        return Err(Error::Protocol("invalid value format"));
    }
    if let Some(element) = array_element(oid) {
        return if format == 0 {
            text_array(element, text(b)?)
        } else {
            binary_array(element, b)
        };
    }
    if format == 0 {
        let s = text(b)?;
        return Ok(match oid {
            16 => Value::Bool(match s {
                "t" => true,
                "f" => false,
                _ => return Err(Error::Protocol("invalid boolean")),
            }),
            21 | 23 => Value::Int(number(s)?),
            20 => Value::Int8(number(s)?),
            700 | 701 => Value::Float(number(s)?),
            1700 => Value::Numeric(s.into()),
            17 => Value::Bytes(Cow::Owned(bytea(s)?)),
            114 | 3802 => Value::Json(s.into()),
            2950 => Value::Uuid(s.into()),
            1082 | 1114 | 1184 => Value::TemporalText {
                oid,
                text: s.into(),
            },
            _ => Value::Text(s.into()),
        });
    }
    let mut c = Cursor(b);
    let value = match oid {
        16 => Value::Bool(match c.u8()? {
            0 => false,
            1 => true,
            _ => return Err(Error::Protocol("invalid boolean")),
        }),
        21 => Value::Int(c.i16()? as i32),
        23 => Value::Int(c.i32()?),
        20 => Value::Int8(i64::from_be_bytes(c.take(8)?.try_into().unwrap())),
        700 => Value::Float(f32::from_be_bytes(c.take(4)?.try_into().unwrap()) as f64),
        701 => Value::Float(f64::from_be_bytes(c.take(8)?.try_into().unwrap())),
        17 => {
            c.take(b.len())?;
            Value::Bytes(b.into())
        }
        25 | 1042 | 1043 | 19 => {
            c.take(b.len())?;
            Value::Text(text(b)?.into())
        }
        114 => {
            c.take(b.len())?;
            Value::Json(text(b)?.into())
        }
        3802 => {
            if c.u8()? != 1 {
                return Err(Error::Protocol("unsupported jsonb version"));
            }
            let s = text(c.0)?;
            c.take(c.0.len())?;
            Value::Json(s.into())
        }
        1082 => Value::Date(c.i32()?),
        1114 | 1184 => Value::Timestamp {
            microseconds: i64::from_be_bytes(c.take(8)?.try_into().unwrap()),
            timestamptz: oid == 1184,
        },
        2950 => {
            let bytes = c.take(16)?;
            let mut s = String::with_capacity(36);
            for (i, b) in bytes.iter().enumerate() {
                if matches!(i, 4 | 6 | 8 | 10) {
                    s.push('-');
                }
                write!(s, "{b:02x}").unwrap();
            }
            Value::Uuid(s.into())
        }
        1700 => {
            c.take(b.len())?;
            Value::Numeric(numeric(b)?.into())
        }
        _ => {
            c.take(b.len())?;
            Value::Raw {
                oid,
                bytes: b.into(),
            }
        }
    };
    c.end()?;
    Ok(value)
}
fn bytea(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 2);
    if let Some(hex) = s.strip_prefix("\\x") {
        if hex.len() % 2 != 0 {
            return Err(Error::Protocol("odd bytea hex length"));
        }
        for pair in hex.as_bytes().as_chunks::<2>().0 {
            let h = |b: u8| {
                (b as char)
                    .to_digit(16)
                    .map(|v| v as u8)
                    .ok_or(Error::Protocol("invalid bytea hex"))
            };
            out.push(h(pair[0])? * 16 + h(pair[1])?);
        }
    } else {
        let mut c = Cursor(s.as_bytes());
        while !c.0.is_empty() {
            let b = c.u8()?;
            if b != b'\\' {
                out.push(b);
            } else if c.0.first() == Some(&b'\\') {
                c.u8()?;
                out.push(b'\\');
            } else {
                let b = c.take(3)?;
                if b[0] > b'3' || b.iter().any(|v| !(b'0'..=b'7').contains(v)) {
                    return Err(Error::Protocol("invalid bytea escape"));
                }
                out.push((b[0] - b'0') * 64 + (b[1] - b'0') * 8 + b[2] - b'0');
            }
        }
    }
    Ok(out)
}
fn numeric(b: &[u8]) -> Result<String> {
    let mut c = Cursor(b);
    let count = c.u16()? as usize;
    let weight = c.i16()? as i32;
    let sign = c.u16()?;
    let scale = c.u16()? as usize;
    let digits = c.take(count.checked_mul(2).ok_or(Error::Limit)?)?;
    c.end()?;
    if scale > 16383 {
        return Err(Error::Protocol("invalid numeric scale"));
    }
    for d in digits.as_chunks::<2>().0 {
        if u16::from_be_bytes(*d) >= 10000 {
            return Err(Error::Protocol("invalid numeric digit"));
        }
    }
    if let Some(s) = match sign {
        0xc000 => Some("NaN"),
        0xd000 => Some("Infinity"),
        0xf000 => Some("-Infinity"),
        0 | 0x4000 => None,
        _ => return Err(Error::Protocol("invalid numeric sign")),
    } {
        return Ok(s.into());
    }
    let digit = |power: i32| {
        let i = weight - power;
        if i < 0 || i as usize >= count {
            0
        } else {
            u16::from_be_bytes(
                digits[i as usize * 2..i as usize * 2 + 2]
                    .try_into()
                    .unwrap(),
            )
        }
    };
    let mut s = String::new();
    if sign == 0x4000 {
        s.push('-');
    }
    if weight < 0 {
        s.push('0');
    } else {
        for power in (0..=weight).rev() {
            if power == weight {
                write!(s, "{}", digit(power)).unwrap();
            } else {
                write!(s, "{:04}", digit(power)).unwrap();
            }
        }
    }
    if scale > 0 {
        s.push('.');
        let start = s.len();
        for i in 1..=scale.div_ceil(4) {
            write!(s, "{:04}", digit(-(i as i32))).unwrap();
        }
        s.truncate(start + scale);
    }
    Ok(s)
}
fn binary_array(element: u32, b: &[u8]) -> Result<Value<'_>> {
    let mut c = Cursor(b);
    let dimensions = c.i32()?;
    let nulls = c.i32()?;
    let actual = c.u32()?;
    if !(0..=6).contains(&dimensions) || !matches!(nulls, 0 | 1) || actual != element {
        return Err(Error::Protocol("invalid array header"));
    }
    let mut shape = [0usize; 6];
    let mut count = if dimensions == 0 { 0 } else { 1usize };
    for n in &mut shape[..dimensions as usize] {
        let len = c.i32()?;
        c.i32()?;
        if len < 0 {
            return Err(Error::Protocol("negative array length"));
        }
        *n = len as usize;
        count = count.checked_mul(*n).ok_or(Error::Limit)?;
    }
    if count > c.0.len() / 4 {
        return Err(Error::Protocol("truncated array elements"));
    }
    fn values<'a>(c: &mut Cursor<'a>, shape: &[usize], element: u32) -> Result<Value<'a>> {
        let mut out = Vec::with_capacity(shape.first().copied().unwrap_or(0));
        if let Some(&n) = shape.first() {
            for _ in 0..n {
                out.push(if shape.len() > 1 {
                    values(c, &shape[1..], element)?
                } else {
                    let len = c.i32()?;
                    let bytes = match len {
                        -1 => None,
                        0.. => Some(c.take(len as usize)?),
                        _ => return Err(Error::Protocol("invalid array element length")),
                    };
                    decode(element, 1, bytes)?
                });
            }
        }
        Ok(Value::Array(out))
    }
    let v = values(&mut c, &shape[..dimensions as usize], element)?;
    c.end()?;
    Ok(v)
}
fn text_array(element: u32, s: &str) -> Result<Value<'_>> {
    let mut c = Cursor(s.as_bytes());
    // pg drops custom lower bounds in the JS array. Validate the prefix grammar.
    while c.0.first() == Some(&b'[') {
        c.u8()?;
        let end =
            c.0.iter()
                .position(|b| *b == b']')
                .ok_or(Error::Protocol("invalid array bounds"))?;
        let bounds = text(c.take(end)?)?;
        let (low, high) = bounds
            .split_once(':')
            .ok_or(Error::Protocol("invalid array bounds"))?;
        let _: i32 = number(low)?;
        let _: i32 = number(high)?;
        c.u8()?;
    }
    if c.0.first() == Some(&b'=') {
        c.u8()?;
    }
    fn array<'a>(c: &mut Cursor<'a>, element: u32, depth: usize) -> Result<Value<'a>> {
        if depth > 6 || c.u8()? != b'{' {
            return Err(Error::Protocol("invalid array nesting"));
        }
        let mut values = Vec::new();
        if c.0.first() == Some(&b'}') {
            c.u8()?;
            return Ok(Value::Array(values));
        }
        loop {
            if c.0.first() == Some(&b'{') {
                values.push(array(c, element, depth + 1)?);
            } else {
                let quoted = c.0.first() == Some(&b'"');
                if quoted {
                    c.u8()?;
                }
                let start = c.0;
                let mut end = 0;
                let mut escaped = false;
                loop {
                    let b = *c.0.get(end).ok_or(Error::Protocol("unterminated array"))?;
                    if b == b'\\' {
                        escaped = true;
                        end += 2;
                        if end > c.0.len() {
                            return Err(Error::Protocol("truncated array escape"));
                        }
                        continue;
                    }
                    if (quoted && b == b'"') || (!quoted && matches!(b, b',' | b'}')) {
                        break;
                    }
                    end += 1;
                }
                c.take(end)?;
                if quoted {
                    c.u8()?;
                }
                let raw = &start[..end];
                if !quoted && raw == b"NULL" {
                    values.push(Value::Null);
                } else if escaped {
                    let mut buf = Vec::with_capacity(raw.len());
                    let mut i = 0;
                    while i < raw.len() {
                        if raw[i] == b'\\' {
                            i += 1;
                        }
                        buf.push(raw[i]);
                        i += 1;
                    }
                    values.push(decode(element, 0, Some(&buf))?.into_owned());
                } else {
                    values.push(decode(element, 0, Some(raw))?);
                }
            }
            match c.u8()? {
                b',' => {}
                b'}' => break,
                _ => return Err(Error::Protocol("invalid array separator")),
            }
        }
        Ok(Value::Array(values))
    }
    let v = array(&mut c, element, 1)?;
    c.end()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_types_and_arrays() {
        assert_eq!(
            decode(20, 0, Some(b"9223372036854775807")).unwrap(),
            Value::Int8(i64::MAX)
        );
        assert_eq!(
            decode(1700, 0, Some(b"1.2300")).unwrap(),
            Value::Numeric("1.2300".into())
        );
        assert_eq!(
            decode(17, 0, Some(b"\\x00ff")).unwrap(),
            Value::Bytes(vec![0, 255].into())
        );
        assert_eq!(
            decode(1009, 0, Some(br#"{"hello, world","NULL",NULL,"a\\b"}"#)).unwrap(),
            Value::Array(vec![
                Value::Text("hello, world".into()),
                Value::Text("NULL".into()),
                Value::Null,
                Value::Text("a\\b".into())
            ])
        );
        assert_eq!(
            decode(1007, 0, Some(b"[2:3]={1,2}")).unwrap(),
            Value::Array(vec![Value::Int(1), Value::Int(2)])
        );
        assert!(decode(1007, 0, Some(b"{1,}")).is_err());
        let numeric = [0, 2, 0, 0, 0, 0, 0, 4, 0, 1, 8, 252];
        assert_eq!(
            decode(1700, 1, Some(&numeric)).unwrap(),
            Value::Numeric("1.2300".into())
        );
        assert_eq!(
            decode(3802, 1, Some(b"\x01{\"x\":1}")).unwrap(),
            Value::Json("{\"x\":1}".into())
        );
        assert!(decode(23, 1, Some(&[1])).is_err());
        assert_eq!(
            decode(2950, 1, Some(&[0; 16])).unwrap(),
            Value::Uuid("00000000-0000-0000-0000-000000000000".into())
        );
        let mut array = Vec::new();
        for n in [1i32, 1, 23, 2, 1, 4, 42, -1] {
            array.extend_from_slice(&n.to_be_bytes());
        }
        assert_eq!(
            decode(1007, 1, Some(&array)).unwrap(),
            Value::Array(vec![Value::Int(42), Value::Null])
        );
    }
}
