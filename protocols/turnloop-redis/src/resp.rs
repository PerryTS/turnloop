//! Bounded RESP2/RESP3 codec. An allocation-free validation pass precedes result
//! materialization, so fragmented input never creates temporary result trees.
use std::io::Write;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Simple(Vec<u8>),
    Error(Vec<u8>),
    Bulk(Vec<u8>),
    Integer(i64),
    Double(f64),
    Boolean(bool),
    Null,
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Set(Vec<Value>),
    Push(Vec<Value>),
    Verbatim(Vec<u8>),
    BigNumber(Vec<u8>),
    Attribute(Vec<(Value, Value)>, Box<Value>),
}
impl Value {
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Simple(v) | Self::Error(v) | Self::Bulk(v) | Self::Verbatim(v)
            | Self::BigNumber(v) => Some(v),
            _ => None,
        }
    }
    pub fn items(&self) -> Option<&[Value]> {
        match self { Self::Array(v) | Self::Set(v) | Self::Push(v) => Some(v), _ => None }
    }
    pub fn integer(&self) -> Option<i64> {
        if let Self::Integer(v) = self { Some(*v) } else { None }
    }
    /// UTF-8 replacement semantics for ioredis string replies. Buffer variants
    /// should use `bytes` directly, preserving arbitrary binary content.
    pub fn text(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.bytes().map(String::from_utf8_lossy)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError { Incomplete, Invalid, Limit, StreamingUnsupported }
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "RESP {self:?}") }
}
impl std::error::Error for DecodeError {}
#[derive(Debug, Clone, Copy)]
pub struct Limits { pub bytes: usize, pub items: usize, pub depth: usize }
impl Default for Limits {
    fn default() -> Self { Self { bytes: 16 * 1024 * 1024, items: 1_000_000, depth: 64 } }
}
fn line<'a>(input: &'a [u8], p: &mut usize) -> Result<&'a [u8], DecodeError> {
    let tail = input.get(*p..).ok_or(DecodeError::Incomplete)?;
    let end = tail.iter().position(|b| *b == b'\n').ok_or(DecodeError::Incomplete)?;
    if end == 0 || tail[end - 1] != b'\r' { return Err(DecodeError::Invalid); }
    let result = &tail[..end - 1];
    *p += end + 1;
    Ok(result)
}
fn number(v: &[u8]) -> Result<i64, DecodeError> {
    if v.is_empty() || v[0] == b'+' || v.iter().enumerate().any(|(i, b)| !b.is_ascii_digit() && !(i == 0 && *b == b'-')) {
        return Err(DecodeError::Invalid);
    }
    std::str::from_utf8(v).map_err(|_| DecodeError::Invalid)?.parse().map_err(|_| DecodeError::Invalid)
}
fn node(input: &[u8], p: &mut usize, depth: usize, budget: &mut usize, limits: Limits, build: bool) -> Result<Value, DecodeError> {
    if depth > limits.depth || *budget == 0 { return Err(DecodeError::Limit); }
    *budget -= 1;
    let kind = *input.get(*p).ok_or(DecodeError::Incomplete)?;
    *p += 1;
    let head = line(input, p)?;
    let copy = || if build { head.to_vec() } else { Vec::new() };
    Ok(match kind {
        b'+' => Value::Simple(copy()),
        b'-' => Value::Error(copy()),
        b':' => Value::Integer(number(head)?),
        b',' => Value::Double(match head {
            b"inf" | b"+inf" => f64::INFINITY, b"-inf" => f64::NEG_INFINITY, b"nan" => f64::NAN,
            _ => std::str::from_utf8(head).map_err(|_| DecodeError::Invalid)?.parse().map_err(|_| DecodeError::Invalid)?,
        }),
        b'(' => {
            let digits = head.strip_prefix(b"-").or_else(|| head.strip_prefix(b"+")).unwrap_or(head);
            if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) { return Err(DecodeError::Invalid); }
            Value::BigNumber(copy())
        }
        b'#' => match head { b"t" => Value::Boolean(true), b"f" => Value::Boolean(false), _ => return Err(DecodeError::Invalid) },
        b'_' if head.is_empty() => Value::Null,
        b'$' | b'!' | b'=' => {
            if head == b"?" { return Err(DecodeError::StreamingUnsupported); }
            let len = number(head)?;
            if len == -1 && kind == b'$' { return Ok(Value::Null); }
            let len = usize::try_from(len).map_err(|_| DecodeError::Invalid)?;
            if len > limits.bytes { return Err(DecodeError::Limit); }
            let end = p.checked_add(len).ok_or(DecodeError::Limit)?;
            let end_crlf = end.checked_add(2).ok_or(DecodeError::Limit)?;
            let bytes = input.get(*p..end).ok_or(DecodeError::Incomplete)?;
            if input.get(end..end_crlf).ok_or(DecodeError::Incomplete)? != b"\r\n" { return Err(DecodeError::Invalid); }
            if kind == b'=' && (len < 4 || bytes[3] != b':') { return Err(DecodeError::Invalid); }
            *p = end_crlf;
            let value = if build { bytes.to_vec() } else { Vec::new() };
            match kind { b'$' => Value::Bulk(value), b'!' => Value::Error(value), _ => Value::Verbatim(value) }
        }
        b'*' | b'~' | b'>' | b'%' | b'|' => {
            if head == b"?" { return Err(DecodeError::StreamingUnsupported); }
            let count = number(head)?;
            if count == -1 && kind == b'*' { return Ok(Value::Null); }
            let count = usize::try_from(count).map_err(|_| DecodeError::Invalid)?;
            let pairs = kind == b'%' || kind == b'|';
            let total = count.checked_mul(if pairs { 2 } else { 1 }).ok_or(DecodeError::Limit)?;
            if total > *budget { return Err(DecodeError::Limit); }
            let mut values = if build && !pairs { Vec::with_capacity(count) } else { Vec::new() };
            let mut entries = if build && pairs { Vec::with_capacity(count) } else { Vec::new() };
            for _ in 0..count {
                let a = node(input, p, depth + 1, budget, limits, build)?;
                if pairs {
                    let b = node(input, p, depth + 1, budget, limits, build)?;
                    if build { entries.push((a, b)); }
                } else if build { values.push(a); }
            }
            match kind {
                b'*' => Value::Array(values), b'~' => Value::Set(values), b'>' => Value::Push(values), b'%' => Value::Map(entries),
                _ => {
                    let value = node(input, p, depth + 1, budget, limits, build)?;
                    if build { Value::Attribute(entries, Box::new(value)) } else { Value::Null }
                }
            }
        }
        _ => return Err(DecodeError::Invalid),
    })
}
pub fn decode(input: &[u8], limits: Limits) -> Result<Option<(Value, usize)>, DecodeError> {
    let mut end = 0;
    match node(input, &mut end, 0, &mut limits.items.clone(), limits, false) {
        Err(DecodeError::Incomplete) if input.len() <= limits.bytes => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    if end > limits.bytes { return Err(DecodeError::Limit); }
    let mut pos = 0;
    let value = node(input, &mut pos, 0, &mut limits.items.clone(), limits, true)?;
    Ok(Some((value, end)))
}
/// Append directly to reusable transport/replay storage; no temporary strings.
pub fn encode_command(args: &[&[u8]], output: &mut Vec<u8>) {
    write!(output, "*{}\r\n", args.len()).expect("Vec writes cannot fail");
    for arg in args {
        write!(output, "${}\r\n", arg.len()).expect("Vec writes cannot fail");
        output.extend_from_slice(arg);
        output.extend_from_slice(b"\r\n");
    }
}
