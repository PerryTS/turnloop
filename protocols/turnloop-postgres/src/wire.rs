use crate::{Error, Result};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cursor<'a>(pub &'a [u8]);
impl<'a> Cursor<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() { return Err(Error::Protocol("truncated message")); }
        let (v, rest) = self.0.split_at(n); self.0 = rest; Ok(v)
    }
    pub fn u8(&mut self) -> Result<u8> { Ok(self.take(1)?[0]) }
    pub fn i16(&mut self) -> Result<i16> { Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap())) }
    pub fn u16(&mut self) -> Result<u16> { Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap())) }
    pub fn i32(&mut self) -> Result<i32> { Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    pub fn u32(&mut self) -> Result<u32> { Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    pub fn cstr(&mut self) -> Result<&'a str> {
        let n = self.0.iter().position(|b| *b == 0).ok_or(Error::Protocol("missing terminator"))?;
        let s = self.take(n + 1)?; std::str::from_utf8(&s[..n]).map_err(|_| Error::Protocol("invalid UTF-8"))
    }
    pub fn end(self) -> Result<()> { if self.0.is_empty() { Ok(()) } else { Err(Error::Protocol("trailing message bytes")) } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field<'a> {
    pub name: &'a str, pub table_id: u32, pub column_id: i16, pub data_type_id: u32,
    pub data_type_size: i16, pub data_type_modifier: i32, pub format: i16,
}
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> { cursor: Cursor<'a>, remaining: u16 }
impl<'a> Fields<'a> {
    pub(crate) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut cursor = Cursor(body); let remaining = cursor.u16()?;
        let fields = Self { cursor, remaining }; let mut check = fields;
        for field in &mut check { field?; } check.cursor.end()?; Ok(fields)
    }
}
impl<'a> Iterator for Fields<'a> {
    type Item = Result<Field<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 { return None; } self.remaining -= 1;
        Some((|| { Ok(Field { name: self.cursor.cstr()?, table_id: self.cursor.u32()?, column_id: self.cursor.i16()?, data_type_id: self.cursor.u32()?, data_type_size: self.cursor.i16()?, data_type_modifier: self.cursor.i32()?, format: self.cursor.i16()? }) })())
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.remaining as usize, Some(self.remaining as usize)) }
}
impl ExactSizeIterator for Fields<'_> {}

/// Borrowed, already validated PostgreSQL row. Iteration never allocates.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> { cursor: Cursor<'a>, remaining: u16 }
impl<'a> Row<'a> {
    pub(crate) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut cursor = Cursor(body); let remaining = cursor.u16()?;
        let row = Self { cursor, remaining }; let mut check = row;
        for value in &mut check { value?; } check.cursor.end()?; Ok(row)
    }
}
impl<'a> Iterator for Row<'a> {
    type Item = Result<Option<&'a [u8]>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 { return None; } self.remaining -= 1;
        Some((|| { let n = self.cursor.i32()?; match n { -1 => Ok(None), 0.. => Ok(Some(self.cursor.take(n as usize)?)), _ => Err(Error::Protocol("invalid field length")) } })())
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.remaining as usize, Some(self.remaining as usize)) }
}
impl ExactSizeIterator for Row<'_> {}

/// ErrorResponse and NoticeResponse retain every field, including future fields.
#[derive(Clone, Copy, Debug)]
pub struct ServerError<'a>(pub(crate) &'a [u8]);
impl<'a> ServerError<'a> {
    pub(crate) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut c = Cursor(body);
        while c.u8()? != 0 { c.cstr()?; } c.end()?; Ok(Self(body))
    }
    pub fn fields(self) -> impl Iterator<Item = (u8, &'a str)> {
        let mut c = Cursor(self.0);
        std::iter::from_fn(move || { let tag = c.u8().ok()?; if tag == 0 { None } else { Some((tag, c.cstr().ok()?)) } })
    }
    pub fn get(self, tag: u8) -> Option<&'a str> { self.fields().find(|(t, _)| *t == tag).map(|(_, v)| v) }
    pub fn code(self) -> &'a str { self.get(b'C').unwrap_or("") }
    pub fn message(self) -> &'a str { self.get(b'M').unwrap_or("") }
    pub fn severity(self) -> Option<&'a str> { self.get(b'S') }
    pub fn detail(self) -> Option<&'a str> { self.get(b'D') }
    pub fn hint(self) -> Option<&'a str> { self.get(b'H') }
    pub fn position(self) -> Option<&'a str> { self.get(b'P') }
}
