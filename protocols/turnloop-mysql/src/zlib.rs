//! One retained deflate state per connection, one independent zlib stream per
//! compressed packet, and no allocation after the first.
//!
//! MySQL's compressed protocol frames each carry their own RFC 1950 stream,
//! which normally means `flate2::Compress::reset` between frames. That is free
//! natively and is not free on wasm32: miniz_oxide keeps the LZ code buffer on
//! the heap there (`deflate/core.rs`, `LZOxide::new`) and
//! `CompressorOxide::reset` replaces it wholesale, so one reset per frame is one
//! allocation per frame on WASI and on the web. DESIGN §3.3 forbids
//! per-operation allocation in every configuration, so nothing here resets.
//!
//! Each message is flushed with `FlushCompress::Full` instead. Its documented
//! contract writes every pending byte, ends on a byte boundary and clears the
//! match dictionary, so the bytes that follow are decodable by a decompressor
//! that starts there. The RFC 1950 wrapper is then written directly: the
//! two-byte header ahead of the deflate data, and behind it a final empty
//! fixed-Huffman block — valid precisely because the full flush left the stream
//! byte-aligned — followed by the big-endian Adler-32 of the frame. The result
//! is a complete, independently decompressible stream per frame, while the
//! compressor keeps its storage for the life of the connection.
use flate2::{Compress, FlushCompress, Status};

/// CM 8 (deflate) and CINFO 7 (32 KiB window), FLEVEL 2, with FCHECK chosen so
/// the pair is a multiple of 31 as RFC 1950 § 2.2 requires.
const HEADER: [u8; 2] = [0x78, 0x9c];
/// BFINAL 1 and BTYPE 01 (fixed Huffman) followed by the seven-bit end-of-block
/// symbol: the shortest terminator for an already byte-aligned deflate stream.
const FINAL_BLOCK: [u8; 2] = [0x03, 0x00];
/// The four-byte Adler-32 plus `FINAL_BLOCK`.
const TRAILER: usize = FINAL_BLOCK.len() + 4;

/// Bytes [`append_stream`] can append for `len` input bytes. Comfortably above
/// zlib's own `deflateBound` (`len + len/4096 + len/16384 + 13`, the stored-block
/// worst case) plus the two-byte header, the five-byte full-flush marker and the
/// trailer, so a complete flush always leaves unused capacity behind.
fn bound(len: usize) -> usize {
    len + len / 1000 + 64
}

/// Appends one complete zlib stream of `input` to `out`, reusing `compressor`'s
/// storage. Allocates only if `out` lacks [`bound`] spare bytes.
pub(crate) fn append_stream(
    compressor: &mut Compress,
    input: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), &'static str> {
    out.reserve(bound(input.len()));
    out.extend_from_slice(&HEADER);
    let consumed = compressor.total_in();
    let start = out.len();
    let status = compressor
        .compress_vec(input, out, FlushCompress::Full)
        .map_err(|_| "Compression failed")?;
    if status != Status::Ok || compressor.total_in() - consumed != input.len() as u64 {
        return Err("Compression did not consume the message");
    }
    // compress_vec writes into spare capacity only. Capacity left over proves the
    // flush was not cut short by a full output buffer, and holds the trailer.
    if out.capacity() - out.len() < TRAILER {
        return Err("Compression buffer exhausted");
    }
    debug_assert!(out.len() - start <= bound(input.len()) - HEADER.len() - TRAILER);
    out.extend_from_slice(&FINAL_BLOCK);
    out.extend_from_slice(&adler32(input).to_be_bytes());
    Ok(())
}

/// RFC 1950 § 9. 5552 is the longest run that cannot overflow before reduction.
fn adler32(input: &[u8]) -> u32 {
    const BASE: u32 = 65521;
    let (mut low, mut high) = (1_u32, 0_u32);
    for chunk in input.chunks(5552) {
        for &byte in chunk {
            low += u32::from(byte);
            high += low;
        }
        low %= BASE;
        high %= BASE;
    }
    (high << 16) | low
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, Decompress, FlushDecompress};

    fn inflate(stream: &[u8], expected: usize) -> Vec<u8> {
        // A fresh decompressor per message: exactly what the server does.
        let mut decoder = Decompress::new(true);
        let mut out = vec![0; expected + 1];
        let status = decoder
            .decompress(stream, &mut out, FlushDecompress::Finish)
            .expect("fixture operation must succeed");
        assert_eq!(status, Status::StreamEnd, "stream must terminate");
        assert_eq!(decoder.total_in(), stream.len() as u64, "trailing bytes");
        out.truncate(decoder.total_out() as usize);
        out
    }

    #[test]
    fn adler32_matches_rfc1950_examples() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // Two full reduction windows, so the chunked accumulation is exercised.
        assert_eq!(adler32(&[0xff; 11_104]), 0xFF6F_3726);
    }

    #[test]
    fn consecutive_messages_decompress_independently() {
        let mut compressor = Compress::new(Compression::default(), false);
        let mut out = Vec::new();
        let mut decompressed = 0;
        for round in 0..64_u32 {
            let mut message = Vec::new();
            for index in 0..300_u32 {
                message.extend_from_slice(&(round * index).to_le_bytes());
            }
            out.clear();
            append_stream(&mut compressor, &message, &mut out)
                .expect("fixture operation must succeed");
            assert!(out.len() < message.len(), "round {round} must compress");
            assert_eq!(inflate(&out, message.len()), message, "round {round}");
            decompressed += 1;
        }
        assert_eq!(decompressed, 64);
    }

    #[test]
    fn steady_state_appends_without_allocating_or_exceeding_the_bound() {
        let mut compressor = Compress::new(Compression::default(), false);
        // Incompressible input is deflate's worst case for the bound.
        let mut message = Vec::new();
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..8192 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            message.extend_from_slice(&state.to_le_bytes());
        }
        let mut out = Vec::with_capacity(bound(message.len()));
        let capacity = out.capacity();
        for round in 0..16 {
            out.clear();
            append_stream(&mut compressor, &message, &mut out)
                .expect("fixture operation must succeed");
            assert_eq!(out.capacity(), capacity, "round {round} reallocated");
            assert!(out.len() <= bound(message.len()), "round {round} bound");
            assert_eq!(inflate(&out, message.len()), message, "round {round}");
        }
    }

    #[test]
    fn empty_and_single_byte_messages_round_trip() {
        let mut compressor = Compress::new(Compression::default(), false);
        let mut out = Vec::new();
        for message in [b"".as_slice(), b"x".as_slice(), &[0_u8; 1]] {
            out.clear();
            append_stream(&mut compressor, message, &mut out)
                .expect("fixture operation must succeed");
            assert_eq!(inflate(&out, message.len().max(1)), message);
        }
    }
}
