//! Reusable plain/compressed MySQL framing. mysql_common's compressed codec
//! creates a zlib encoder/decoder per frame; this one resets retained states.
use crate::{Error, Result};
use bytes::{Buf, BytesMut};
use flate2::{Compress, Compression, Decompress, FlushDecompress, Status};
const CHUNK: usize = 0xff_ffff;
fn length(b: &[u8]) -> usize {
    b[0] as usize | ((b[1] as usize) << 8) | ((b[2] as usize) << 16)
}
fn put_len(out: &mut BytesMut, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes()[..3]);
}
struct Compressed {
    seq: u8,
    input: BytesMut,
    output: BytesMut,
    encoded: Vec<u8>,
    decoded: Vec<u8>,
    encoder: Compress,
    decoder: Decompress,
}
pub(crate) struct PacketCodec {
    pub max_allowed_packet: usize,
    seq: u8,
    compressed: Option<Box<Compressed>>,
}
impl Default for PacketCodec {
    fn default() -> Self {
        Self {
            max_allowed_packet: 64 * 1024 * 1024,
            seq: 0,
            compressed: None,
        }
    }
}
impl PacketCodec {
    pub fn reset_seq_id(&mut self) {
        self.seq = 0;
        if let Some(c) = &mut self.compressed {
            c.seq = 0;
        }
    }
    pub fn compress(&mut self, level: Compression) {
        self.compressed = Some(Box::new(Compressed {
            seq: 0,
            input: BytesMut::with_capacity(8192),
            output: BytesMut::with_capacity(8192),
            encoded: Vec::new(),
            decoded: Vec::new(),
            // Raw deflate: zlib.rs writes the RFC 1950 wrapper per frame.
            encoder: Compress::new(level, false),
            decoder: Decompress::new(true),
        }));
        self.seq = 0;
    }
    pub fn encode(&mut self, src: &mut &[u8], dst: &mut BytesMut) -> Result<()> {
        if src.len() > self.max_allowed_packet {
            return Err(Error::Limit);
        }
        if let Some(c) = &mut self.compressed {
            c.output.clear();
            encode_plain(&mut self.seq, src, &mut c.output);
            for chunk in c.output.chunks(CHUNK) {
                // One retained deflate state, one zlib stream per frame; see zlib.rs.
                c.encoded.clear();
                crate::zlib::append_stream(&mut c.encoder, chunk, &mut c.encoded)
                    .map_err(Error::Protocol)?;
                let (payload, plain) = if c.encoded.len() < chunk.len() {
                    (&c.encoded[..], chunk.len())
                } else {
                    (chunk, 0)
                };
                put_len(dst, payload.len());
                dst.extend_from_slice(&[c.seq]);
                put_len(dst, plain);
                dst.extend_from_slice(payload);
                c.seq = c.seq.wrapping_add(1);
            }
            // MySQL net_serv synchronizes inner sequence after compressed writes.
            self.seq = c.seq;
        } else {
            encode_plain(&mut self.seq, src, dst);
        }
        *src = &[];
        Ok(())
    }
    pub fn decode(&mut self, src: &mut BytesMut, dst: &mut Vec<u8>) -> Result<bool> {
        let Some(c) = &mut self.compressed else {
            return decode_plain(&mut self.seq, src, dst, self.max_allowed_packet, None);
        };
        loop {
            if decode_plain(
                &mut self.seq,
                &mut c.input,
                dst,
                self.max_allowed_packet,
                Some(c.seq.wrapping_sub(1)),
            )? {
                return Ok(true);
            }
            if src.len() < 7 {
                return Ok(false);
            }
            let size = length(src);
            let plain = length(&src[4..]);
            if size > self.max_allowed_packet || plain > self.max_allowed_packet || size == 0 {
                return Err(Error::Limit);
            }
            if src[3] != c.seq {
                return Err(Error::Protocol("compressed packets out of order"));
            }
            if src.len() < size + 7 {
                return Ok(false);
            }
            if c.input
                .len()
                .saturating_add(if plain == 0 { size } else { plain })
                > self.max_allowed_packet + 4
            {
                return Err(Error::Limit);
            }
            if plain == 0 {
                c.input.extend_from_slice(&src[7..7 + size]);
            } else {
                c.decoder.reset(true);
                c.decoded.resize(plain, 0);
                let status = c
                    .decoder
                    .decompress(&src[7..7 + size], &mut c.decoded, FlushDecompress::Finish)
                    .map_err(|_| Error::Protocol("invalid compressed payload"))?;
                if status != Status::StreamEnd
                    || c.decoder.total_in() != size as u64
                    || c.decoder.total_out() != plain as u64
                {
                    return Err(Error::Protocol("compressed size mismatch"));
                }
                c.input.extend_from_slice(&c.decoded);
            }
            src.advance(size + 7);
            c.seq = c.seq.wrapping_add(1);
        }
    }
}
fn encode_plain(seq: &mut u8, src: &[u8], dst: &mut BytesMut) {
    for chunk in src.chunks(CHUNK) {
        put_len(dst, chunk.len());
        dst.extend_from_slice(&[*seq]);
        dst.extend_from_slice(chunk);
        *seq = seq.wrapping_add(1);
    }
    if src.len().is_multiple_of(CHUNK) {
        put_len(dst, 0);
        dst.extend_from_slice(&[*seq]);
        *seq = seq.wrapping_add(1);
    }
}
fn decode_plain(
    seq: &mut u8,
    src: &mut BytesMut,
    dst: &mut Vec<u8>,
    max: usize,
    alternate: Option<u8>,
) -> Result<bool> {
    loop {
        if src.len() < 4 {
            return Ok(false);
        }
        let size = length(src);
        if size > max.saturating_sub(dst.len()) {
            return Err(Error::Limit);
        }
        if src[3] != *seq && Some(src[3]) != alternate {
            return Err(Error::Protocol("packets out of order"));
        }
        if src.len() < size + 4 {
            return Ok(false);
        }
        *seq = src[3].wrapping_add(1);
        dst.extend_from_slice(&src[4..size + 4]);
        src.advance(size + 4);
        if size < CHUNK {
            return Ok(true);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_fragmented_roundtrip_and_bad_lengths() {
        let body = vec![b'x'; 70000];
        let mut encoder = PacketCodec::default();
        let mut decoder = PacketCodec::default();
        encoder.compress(Compression::fast());
        decoder.compress(Compression::fast());
        let mut bytes = BytesMut::new();
        encoder.encode(&mut &body[..], &mut bytes).unwrap();
        assert!(bytes.len() < body.len() / 4);
        let mut input = BytesMut::new();
        let mut output = Vec::new();
        let mut done = false;
        for b in bytes {
            input.extend_from_slice(&[b]);
            if decoder.decode(&mut input, &mut output).unwrap() {
                assert!(!done);
                done = true;
            }
        }
        assert!(done);
        assert_eq!(output, body);
        let mut bad = BytesMut::from(&[1, 0, 0, 0, 1, 0, 0, 0][..]);
        let mut decoder = PacketCodec::default();
        decoder.compress(Compression::fast());
        assert!(decoder.decode(&mut bad, &mut Vec::new()).is_err());
    }
    /// One retained deflate state must still emit one independently decodable
    /// RFC 1950 stream per frame (`src/zlib.rs`). mysql_common's compressed codec
    /// is an independent implementation and never sees our encoder's state.
    #[test]
    fn consecutive_compressed_frames_decode_with_the_upstream_codec() {
        let mut encoder = PacketCodec::default();
        encoder.compress(Compression::fast());
        let mut upstream = mysql_common::proto::codec::PacketCodec::default();
        upstream.compress(Compression::fast());
        let mut decoded = 0;
        for round in 0..32_u8 {
            let body = vec![b'a' + round % 26; 4096];
            let mut bytes = BytesMut::new();
            encoder.encode(&mut &body[..], &mut bytes).unwrap();
            assert!(bytes.len() < body.len() / 4, "round {round} must compress");
            let mut output = Vec::new();
            assert!(upstream.decode(&mut bytes, &mut output).unwrap());
            assert_eq!(output, body, "round {round}");
            assert!(bytes.is_empty(), "round {round} left trailing bytes");
            decoded += 1;
        }
        assert_eq!(decoded, 32);
    }
    #[test]
    fn full_fragment_requires_empty_terminator() {
        let body = vec![7; CHUNK];
        let mut codec = PacketCodec::default();
        let mut bytes = BytesMut::new();
        codec.encode(&mut &body[..], &mut bytes).unwrap();
        assert_eq!(&bytes[CHUNK + 4..], &[0, 0, 0, 1]);
        let mut decode = PacketCodec::default();
        let mut output = Vec::new();
        let tail = bytes.split_off(CHUNK + 4);
        assert!(!decode.decode(&mut bytes, &mut output).unwrap());
        assert_eq!(output.len(), CHUNK);
        bytes.extend_from_slice(&tail);
        assert!(decode.decode(&mut bytes, &mut output).unwrap());
        assert_eq!(output, body);
    }
}
