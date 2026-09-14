//! Bounded decompression into a caller-reused result buffer. This convenience API
//! accepts a complete encoded body; wire body streaming is exposed by the codecs.
use crate::{Error, Result};
use std::io::{Cursor, Read};
pub fn decode(encoding: &str, input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<()> {
    output.clear();
    let mut reader: Box<dyn Read + '_> = match encoding.trim() {
        "" | "identity" => Box::new(Cursor::new(input)),
        "gzip" | "x-gzip" => Box::new(flate2::read::MultiGzDecoder::new(input)),
        "deflate" => {
            if input.len() >= 2
                && input[0] & 15 == 8
                && u16::from_be_bytes([input[0], input[1]]).is_multiple_of(31)
            {
                Box::new(flate2::read::ZlibDecoder::new(input))
            } else {
                Box::new(flate2::read::DeflateDecoder::new(input))
            }
        }
        "br" => Box::new(brotli::Decompressor::new(input, 4096)),
        #[cfg(not(target_arch = "wasm32"))]
        "zstd" => Box::new(
            zstd::stream::read::Decoder::new(input)
                .map_err(|_| Error::new("UND_ERR_SOCKET", "invalid zstd stream"))?,
        ),
        _ => {
            return Err(Error::new(
                "UND_ERR_NOT_SUPPORTED",
                "unsupported content encoding",
            ));
        }
    };
    let mut bytes = [0; 8192];
    loop {
        let n = reader
            .read(&mut bytes)
            .map_err(|_| Error::new("UND_ERR_SOCKET", "invalid compressed body"))?;
        if n == 0 {
            break;
        }
        if n > limit.saturating_sub(output.len()) {
            return Err(Error::new(
                "UND_ERR_RES_EXCEEDED_MAX_SIZE",
                "decoded body exceeds limit",
            ));
        }
        output.extend_from_slice(&bytes[..n]);
    }
    Ok(())
}
