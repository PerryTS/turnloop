//! MongoDB specifications: message/OP_MSG.md §§ OP_MSG, flagBits, sections;
//! compression/OP_COMPRESSED.md. Integer arithmetic is checked before slicing.
use crate::{Error, Result};
use bson::raw::RawDocument;
use std::ops::Range;
pub const OP_MSG: i32 = 2013;
pub const OP_COMPRESSED: i32 = 2012;
pub const CHECKSUM: u32 = 1;
pub const MORE_TO_COME: u32 = 2;
pub const EXHAUST_ALLOWED: u32 = 1 << 16;
pub const DEFAULT_MAX_MESSAGE: usize = 48_000_000;
pub fn i32_at(bytes: &[u8], at: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or_else(|| Error::protocol("Truncated int32"))?
            .try_into()
            .unwrap(),
    ))
}
fn length(bytes: &[u8], at: usize, min: usize) -> Result<usize> {
    let n = i32_at(bytes, at)?;
    if n < min as i32 {
        return Err(Error::protocol("Invalid BSON or message length"));
    }
    Ok(n as usize)
}
pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &v in bytes {
        c ^= v as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ (0x82f63b78u32.wrapping_mul(c & 1));
        }
    }
    !c
}
#[derive(Debug)]
pub struct Message<'a> {
    pub request_id: i32,
    pub response_to: i32,
    pub flags: u32,
    pub body: &'a RawDocument,
    sections: &'a [u8],
}
impl<'a> Message<'a> {
    pub fn parse(bytes: &'a [u8], max: usize) -> Result<Self> {
        let n = length(bytes, 0, 26)?;
        if n != bytes.len() || n > max {
            return Err(Error::protocol("Invalid OP_MSG length"));
        }
        if i32_at(bytes, 12)? != OP_MSG {
            return Err(Error::protocol("Expected OP_MSG"));
        }
        let flags = i32_at(bytes, 16)? as u32;
        if flags & 0xffff & !3 != 0 {
            return Err(Error::protocol("Unknown required OP_MSG flag"));
        }
        let end = if flags & CHECKSUM != 0 {
            if n < 30 {
                return Err(Error::protocol("Missing checksum"));
            }
            let end = n - 4;
            if crc32c(&bytes[..end]) != i32_at(bytes, end)? as u32 {
                return Err(Error::protocol("OP_MSG checksum mismatch"));
            }
            end
        } else {
            n
        };
        let sections = &bytes[20..end];
        let mut body = None;
        let mut at = 0;
        while at < sections.len() {
            let start = at;
            let kind = sections[at];
            at += 1;
            let size = length(sections, at, 5)?;
            let stop = at
                .checked_add(size)
                .filter(|&s| s <= sections.len())
                .ok_or_else(|| Error::protocol("Truncated section"))?;
            match kind {
                0 => {
                    if body.is_some() {
                        return Err(Error::protocol("Duplicate OP_MSG body"));
                    }
                    let d = RawDocument::from_bytes(&sections[at..stop])
                        .map_err(|_| Error::protocol("Invalid BSON body"))?;
                    validate(d, 0)?;
                    body = Some(d);
                }
                1 => {
                    let name_end = sections[at + 4..stop]
                        .iter()
                        .position(|&v| v == 0)
                        .map(|p| p + at + 4)
                        .ok_or_else(|| Error::protocol("Unterminated sequence identifier"))?;
                    let name = std::str::from_utf8(&sections[at + 4..name_end])
                        .map_err(|_| Error::protocol("Invalid sequence identifier"))?;
                    // No per-section identifier allocation: bounded re-scan of previous sections.
                    let mut prev = 0;
                    while prev < start {
                        let k = sections[prev];
                        prev += 1;
                        let sz = length(sections, prev, 5)?;
                        if k == 1 {
                            let e = sections[prev + 4..prev + sz]
                                .iter()
                                .position(|&b| b == 0)
                                .unwrap()
                                + prev
                                + 4;
                            if &sections[prev + 4..e] == name.as_bytes() {
                                return Err(Error::protocol("Duplicate sequence identifier"));
                            }
                        }
                        prev += sz;
                    }
                    let mut p = name_end + 1;
                    while p < stop {
                        let len = length(sections, p, 5)?;
                        let e = p
                            .checked_add(len)
                            .filter(|&e| e <= stop)
                            .ok_or_else(|| Error::protocol("Truncated sequence BSON"))?;
                        let d = RawDocument::from_bytes(&sections[p..e])
                            .map_err(|_| Error::protocol("Invalid sequence BSON"))?;
                        validate(d, 0)?;
                        p = e;
                    }
                }
                _ => return Err(Error::protocol("Unknown OP_MSG section type")),
            }
            at = stop;
        }
        let body = body.ok_or_else(|| Error::protocol("Missing OP_MSG body"))?;
        Ok(Self {
            request_id: i32_at(bytes, 4)?,
            response_to: i32_at(bytes, 8)?,
            flags,
            body,
            sections,
        })
    }
    /// Replaces only the body; document sequences retain their original bytes/order.
    pub fn encode_with_body(&self,out:&mut Vec<u8>,body:&RawDocument,max:usize)->Result<()> {
        encode(out,self.request_id,0,0,body,&[],max)?;
        let mut at=0;while at<self.sections.len(){let start=at;let kind=self.sections[at];at+=1;let n=i32_at(self.sections,at)? as usize;at+=n;if kind==1{out.extend_from_slice(&self.sections[start..at]);}}
        if out.len()>max||out.len()>i32::MAX as usize{out.clear();return Err(Error::protocol("Message exceeds maxMessageSizeBytes"));}let n=out.len() as i32;out[..4].copy_from_slice(&n.to_le_bytes());Ok(())
    }
    pub fn sequences(&self) -> Sequences<'a> {
        Sequences {
            bytes: self.sections,
            at: 0,
        }
    }
}
fn validate(doc: &RawDocument, depth: usize) -> Result<()> {
    if depth > 100 {
        return Err(Error::protocol("BSON nesting exceeds 100"));
    }
    for el in doc {
        let (_, v) = el.map_err(|_| Error::protocol("Malformed BSON element"))?;
        match v {
            bson::raw::RawBsonRef::Document(d) => validate(d, depth + 1)?,
            bson::raw::RawBsonRef::Array(a) => {
                let d = RawDocument::from_bytes(a.as_bytes())
                    .map_err(|_| Error::protocol("Malformed BSON array"))?;
                validate(d, depth + 1)?;
            }
            _ => {}
        }
    }
    Ok(())
}
pub struct Sequence<'a> {
    pub identifier: &'a str,
    bytes: &'a [u8],
}
impl<'a> Sequence<'a> {
    pub fn documents(&self) -> impl Iterator<Item = &'a RawDocument> {
        let mut b = self.bytes;
        std::iter::from_fn(move || {
            if b.is_empty() {
                return None;
            }
            let n = i32_at(b, 0).unwrap() as usize;
            let d = RawDocument::from_bytes(&b[..n]).unwrap();
            b = &b[n..];
            Some(d)
        })
    }
}
pub struct Sequences<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Iterator for Sequences<'a> {
    type Item = Sequence<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        while self.at < self.bytes.len() {
            let k = self.bytes[self.at];
            self.at += 1;
            let n = i32_at(self.bytes, self.at).unwrap() as usize;
            let b = &self.bytes[self.at..self.at + n];
            self.at += n;
            if k == 1 {
                let e = b[4..].iter().position(|&v| v == 0).unwrap() + 4;
                return Some(Sequence {
                    identifier: std::str::from_utf8(&b[4..e]).unwrap(),
                    bytes: &b[e + 1..],
                });
            }
        }
        None
    }
}
/// Encodes raw documents into a reusable output buffer. No allocations after capacity warms.
pub fn encode(
    out: &mut Vec<u8>,
    request: i32,
    response: i32,
    flags: u32,
    body: &RawDocument,
    sequences: &[(&str, &[&RawDocument])],
    max: usize,
) -> Result<()> {
    out.clear();
    out.extend_from_slice(&[0; 16]);
    out[4..8].copy_from_slice(&request.to_le_bytes());
    out[8..12].copy_from_slice(&response.to_le_bytes());
    out[12..16].copy_from_slice(&OP_MSG.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.push(0);
    out.extend_from_slice(body.as_bytes());
    for (name, docs) in sequences {
        if name.as_bytes().contains(&0) {
            out.clear();
            return Err(Error::protocol("NUL in sequence identifier"));
        }
        out.push(1);
        let at = out.len();
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        for doc in *docs {
            out.extend_from_slice(doc.as_bytes());
        }
        let n = out.len() - at;
        if n > i32::MAX as usize {
            out.clear();
            return Err(Error::protocol("Sequence too large"));
        }
        out[at..at + 4].copy_from_slice(&(n as i32).to_le_bytes());
    }
    let n = out.len() + if flags & CHECKSUM != 0 { 4 } else { 0 };
    if n > max || n > i32::MAX as usize {
        out.clear();
        return Err(Error::protocol("Message exceeds maxMessageSizeBytes"));
    }
    out[..4].copy_from_slice(&(n as i32).to_le_bytes());
    if flags & CHECKSUM != 0 {
        let c = crc32c(out);
        out.extend_from_slice(&c.to_le_bytes());
    }
    Ok(())
}
/// Bounded incremental frame buffer; the host drains a reply before feeding another.
#[derive(Debug)]
pub struct Decoder {
    bytes: Vec<u8>,
    max: usize,
}
impl Decoder {
    pub fn new(max: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(8192),
            max,
        }
    }
    pub fn feed(&mut self, input: &[u8]) -> Result<usize> {
        let target = if self.bytes.len() < 4 {
            4
        } else {
            length(&self.bytes, 0, 16)?
        };
        if target > self.max {
            return Err(Error::protocol("Message exceeds maxMessageSizeBytes"));
        }
        let n = input.len().min(target - self.bytes.len());
        self.bytes.extend_from_slice(&input[..n]);
        if self.bytes.len() >= 4 && length(&self.bytes, 0, 16)? > self.max {
            return Err(Error::protocol("Message exceeds maxMessageSizeBytes"));
        }
        Ok(n)
    }
    pub fn complete(&self) -> bool {
        self.bytes.len() >= 4
            && i32_at(&self.bytes, 0).is_ok_and(|n| n as usize == self.bytes.len())
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn clear(&mut self) {
        self.bytes.clear();
    }
    pub fn set_max(&mut self, max: usize) {
        self.max = max;
    }
}
/// Raw BSON command builder with a retained backing buffer. Nesting uses offsets,
/// so arrays/documents can be built without temporary Documents or key Strings.
#[derive(Debug, Default)]
pub struct BsonWriter {
    bytes: Vec<u8>,
}
impl BsonWriter {
    pub fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(1024),
        }
    }
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.bytes.extend_from_slice(&[0; 4]);
    }
    fn key(&mut self, kind: u8, key: &str) -> Result<()> {
        if key.as_bytes().contains(&0) {
            return Err(Error::protocol("NUL in BSON key"));
        }
        self.bytes.push(kind);
        self.bytes.extend_from_slice(key.as_bytes());
        self.bytes.push(0);
        Ok(())
    }
    pub fn string(&mut self, k: &str, v: &str) -> Result<()> {
        self.key(2, k)?;
        let n = i32::try_from(v.len() + 1).map_err(|_| Error::protocol("BSON string too large"))?;
        self.bytes.extend_from_slice(&n.to_le_bytes());
        self.bytes.extend_from_slice(v.as_bytes());
        self.bytes.push(0);
        Ok(())
    }
    pub fn int32(&mut self, k: &str, v: i32) -> Result<()> {
        self.key(16, k)?;
        self.bytes.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    pub fn int64(&mut self, k: &str, v: i64) -> Result<()> {
        self.key(18, k)?;
        self.bytes.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    pub fn boolean(&mut self, k: &str, v: bool) -> Result<()> {
        self.key(8, k)?;
        self.bytes.push(u8::from(v));
        Ok(())
    }
    pub fn document(&mut self, k: &str, d: &RawDocument) -> Result<()> {
        self.key(3, k)?;
        self.bytes.extend_from_slice(d.as_bytes());
        Ok(())
    }
    pub fn binary(&mut self, k: &str, subtype: u8, b: &[u8]) -> Result<()> {
        self.key(5, k)?;
        let n = i32::try_from(b.len()).map_err(|_| Error::protocol("Binary too large"))?;
        self.bytes.extend_from_slice(&n.to_le_bytes());
        self.bytes.push(subtype);
        self.bytes.extend_from_slice(b);
        Ok(())
    }
    pub fn start_document(&mut self, k: &str, array: bool) -> Result<usize> {
        self.key(if array { 4 } else { 3 }, k)?;
        let p = self.bytes.len();
        self.bytes.extend_from_slice(&[0; 4]);
        Ok(p)
    }
    pub fn end_document(&mut self, at: usize) -> Result<()> {
        if at + 4 > self.bytes.len() {
            return Err(Error::protocol("Invalid BSON builder offset"));
        }
        self.bytes.push(0);
        let n =
            i32::try_from(self.bytes.len() - at).map_err(|_| Error::protocol("BSON too large"))?;
        self.bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
        Ok(())
    }
    pub fn finish(&mut self) -> Result<&RawDocument> {
        self.end_document(0)?;
        self.as_raw()
    }
    pub fn as_raw(&self) -> Result<&RawDocument> {
        RawDocument::from_bytes(&self.bytes).map_err(|_| Error::protocol("Unfinished BSON builder"))
    }
    pub fn append_fields(&mut self, doc: &RawDocument, omit: &[&str]) -> Result<()> {
        let b = doc.as_bytes();
        let mut start = 4;
        for el in doc.iter_elements() {
            let el = el.map_err(|_| Error::protocol("Malformed BSON options"))?;
            let range: Range<usize> = start..start + 1 + el.key().as_str().len() + 1 + el.size();
            if !omit.contains(&el.key().as_str()) {
                self.bytes.extend_from_slice(&b[range.clone()]);
            }
            start = range.end;
        }
        Ok(())
    }
}
