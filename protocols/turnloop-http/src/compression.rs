//! Bounded decompression into a caller-reused result buffer. This convenience API
//! accepts a complete encoded body; wire body streaming is exposed by the codecs.
use crate::{Error, Result};
/// Convenience whole-body decode. Cache `StreamingDecoder` and use `reset` for
/// allocation-free repeated responses instead of constructing one per call.
pub fn decode(encoding: &str, input: &[u8], output: &mut Vec<u8>, limit: usize) -> Result<()> {
    output.clear();
    let mut decoder = StreamingDecoder::new(encoding.trim(), limit)?;
    let mut pos = 0;
    loop {
        let mut bytes = [0; 8192];
        let step = decoder.process(&input[pos..], &mut bytes, true)?;
        pos += step.consumed;
        output.extend_from_slice(&bytes[..step.written]);
        if step.finished {
            return Ok(());
        }
        if step.consumed == 0 && step.written == 0 {
            return Err(corrupt());
        }
    }
}

/// A single incremental decode step. Retain input after `consumed`, and consume
/// output before calling again. No internal input/output staging allocation.
#[derive(Debug)]
pub struct DecodeStep {
    pub consumed: usize,
    pub written: usize,
    pub finished: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Header,
    Body,
    Trailer,
    Done,
}
type BytePool = crate::recycling::Pool<u8>;
type IntPool = crate::recycling::Pool<u32>;
type CodePool = crate::recycling::Pool<brotli::HuffmanCode>;
type BrotliState = brotli::BrotliState<BytePool, IntPool, CodePool>;
// Keep the WASM decoder inline: 480 bytes avoids another per-response allocation.
#[allow(clippy::large_enum_variant)]
enum Engine {
    Identity,
    Deflate {
        decoder: flate2::Decompress,
        gzip: bool,
        detect: bool,
        stage: Stage,
        crc: flate2::Crc,
        members: usize,
    },
    Brotli {
        state: Box<BrotliState>,
        bytes: BytePool,
        ints: IntPool,
        codes: CodePool,
    },
    #[cfg(not(target_arch = "wasm32"))]
    Zstd {
        decoder: zstd::stream::raw::Decoder<'static>,
        boundary: bool,
    },
    #[cfg(target_arch = "wasm32")]
    Zstd {
        decoder: ruzstd::decoding::FrameDecoder,
        reset: bool,
        boundary: bool,
    },
}
pub struct StreamingDecoder {
    engine: Engine,
    total: usize,
    limit: usize,
    done: bool,
    failed: bool,
}
fn corrupt() -> Error {
    Error::new("UND_ERR_SOCKET", "invalid or incomplete compressed body")
}
impl StreamingDecoder {
    pub fn new(encoding: &str, limit: usize) -> Result<Self> {
        let engine = match encoding {
            "" | "identity" => Engine::Identity,
            "gzip" | "x-gzip" => Engine::Deflate {
                decoder: flate2::Decompress::new(false),
                gzip: true,
                detect: false,
                stage: Stage::Header,
                crc: flate2::Crc::new(),
                members: 0,
            },
            "deflate" => Engine::Deflate {
                decoder: flate2::Decompress::new(false),
                gzip: false,
                detect: true,
                stage: Stage::Body,
                crc: flate2::Crc::new(),
                members: 0,
            },
            "br" => {
                let bytes = BytePool::default();
                let ints = IntPool::default();
                let codes = CodePool::default();
                let state = Box::new(BrotliState::new(bytes.clone(), ints.clone(), codes.clone()));
                Engine::Brotli {
                    state,
                    bytes,
                    ints,
                    codes,
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            "zstd" => {
                let mut decoder = zstd::stream::raw::Decoder::new().map_err(|_| corrupt())?;
                decoder
                    .set_parameter(zstd::stream::raw::DParameter::WindowLogMax(27))
                    .map_err(|_| corrupt())?;
                Engine::Zstd {
                    decoder,
                    boundary: false,
                }
            }
            #[cfg(target_arch = "wasm32")]
            "zstd" => Engine::Zstd {
                decoder: ruzstd::decoding::FrameDecoder::new(),
                reset: true,
                boundary: false,
            },
            _ => {
                return Err(Error::new(
                    "UND_ERR_NOT_SUPPORTED",
                    "unsupported content encoding",
                ));
            }
        };
        Ok(Self {
            engine,
            total: 0,
            limit,
            done: false,
            failed: false,
        })
    }
    /// Reuse algorithm and scratch storage for another body of the same encoding.
    /// Warm up with the largest expected body shape before measuring allocations.
    pub fn reset(&mut self, limit: usize) -> Result<()> {
        match &mut self.engine {
            Engine::Identity => {}
            Engine::Deflate {
                decoder,
                gzip,
                detect,
                stage,
                crc,
                members,
            } => {
                decoder.reset(false);
                *detect = !*gzip;
                *stage = if *gzip { Stage::Header } else { Stage::Body };
                crc.reset();
                *members = 0;
            }
            Engine::Brotli {
                state,
                bytes,
                ints,
                codes,
            } => {
                **state = BrotliState::new(bytes.clone(), ints.clone(), codes.clone());
            }
            #[cfg(not(target_arch = "wasm32"))]
            Engine::Zstd { decoder, boundary } => {
                use zstd::stream::raw::Operation;
                decoder.reinit().map_err(|_| corrupt())?;
                *boundary = false;
            }
            #[cfg(target_arch = "wasm32")]
            Engine::Zstd {
                reset, boundary, ..
            } => {
                *reset = true;
                *boundary = false;
            }
        }
        self.total = 0;
        self.limit = limit;
        self.done = false;
        self.failed = false;
        Ok(())
    }
    /// `end` means no more encoded bytes will arrive, not that output is unbounded.
    pub fn process(&mut self, input: &[u8], output: &mut [u8], end: bool) -> Result<DecodeStep> {
        if self.failed {
            return Err(corrupt());
        }
        let result = self.process_inner(input, output, end);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn process_inner(&mut self, input: &[u8], output: &mut [u8], end: bool) -> Result<DecodeStep> {
        if self.done {
            if !input.is_empty() {
                return Err(corrupt());
            }
            return Ok(DecodeStep {
                consumed: 0,
                written: 0,
                finished: true,
            });
        }
        if output.is_empty() {
            return Ok(DecodeStep {
                consumed: 0,
                written: 0,
                finished: false,
            });
        }
        let mut consumed = 0;
        let mut written = 0;
        let mut finished = false;
        match &mut self.engine {
            Engine::Identity => {
                consumed = input.len().min(output.len());
                output[..consumed].copy_from_slice(&input[..consumed]);
                written = consumed;
                finished = end && consumed == input.len();
            }
            Engine::Deflate {
                decoder,
                gzip,
                detect,
                stage,
                crc,
                members,
            } => {
                if *stage == Stage::Header {
                    if input.is_empty() && end && *members > 0 {
                        finished = true;
                    } else if let Some(n) = gzip_header(input)? {
                        consumed = n;
                        decoder.reset(false);
                        crc.reset();
                        *stage = Stage::Body;
                    } else if end {
                        return Err(corrupt());
                    }
                } else if *stage == Stage::Trailer {
                    if input.len() >= 8 {
                        if u32::from_le_bytes(input[..4].try_into().unwrap()) != crc.sum()
                            || u32::from_le_bytes(input[4..8].try_into().unwrap()) != crc.amount()
                        {
                            return Err(corrupt());
                        }
                        consumed = 8;
                        *members += 1;
                        *stage = Stage::Header;
                        finished = end && input.len() == 8;
                    } else if end {
                        return Err(corrupt());
                    }
                } else if *stage == Stage::Done {
                    if !input.is_empty() {
                        return Err(corrupt());
                    }
                    finished = end;
                } else {
                    if *detect {
                        if input.len() < 2 {
                            if end {
                                return Err(corrupt());
                            }
                            return Ok(DecodeStep {
                                consumed: 0,
                                written: 0,
                                finished: false,
                            });
                        }
                        let zlib = input[0] & 15 == 8
                            && u16::from_be_bytes([input[0], input[1]]).is_multiple_of(31);
                        decoder.reset(zlib);
                        *detect = false;
                    }
                    let before_in = decoder.total_in();
                    let before_out = decoder.total_out();
                    let status = decoder
                        .decompress(input, output, flate2::FlushDecompress::None)
                        .map_err(|_| corrupt())?;
                    consumed = (decoder.total_in() - before_in) as usize;
                    written = (decoder.total_out() - before_out) as usize;
                    if *gzip {
                        crc.update(&output[..written]);
                    }
                    if status == flate2::Status::StreamEnd {
                        *stage = if *gzip { Stage::Trailer } else { Stage::Done };
                        finished = !*gzip && end && consumed == input.len();
                    } else if end && consumed == input.len() && written < output.len() {
                        return Err(corrupt());
                    }
                }
            }
            Engine::Brotli { state, .. } => {
                let mut available_in = input.len();
                let mut available_out = output.len();
                let mut total = self.total;
                let result = brotli::BrotliDecompressStream(
                    &mut available_in,
                    &mut consumed,
                    input,
                    &mut available_out,
                    &mut written,
                    output,
                    &mut total,
                    state,
                );
                match result {
                    brotli::BrotliResult::ResultSuccess => {
                        if consumed != input.len() {
                            return Err(corrupt());
                        }
                        finished = true;
                    }
                    brotli::BrotliResult::ResultFailure => return Err(corrupt()),
                    brotli::BrotliResult::NeedsMoreInput if end => return Err(corrupt()),
                    _ => {}
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Engine::Zstd { decoder, boundary } => {
                use zstd::stream::raw::Operation;
                if input.is_empty() && end && *boundary {
                    self.done = true;
                    return Ok(DecodeStep {
                        consumed: 0,
                        written: 0,
                        finished: true,
                    });
                }
                let result = decoder
                    .run_on_buffers(input, output)
                    .map_err(|_| corrupt())?;
                consumed = result.bytes_read;
                written = result.bytes_written;
                *boundary = result.remaining == 0;
                finished = result.remaining == 0 && end && consumed == input.len();
                if result.remaining != 0 && end && consumed == input.len() && written < output.len()
                {
                    return Err(corrupt());
                }
            }
            #[cfg(target_arch = "wasm32")]
            Engine::Zstd {
                decoder,
                reset,
                boundary,
            } => {
                if input.is_empty() && end && *boundary {
                    self.done = true;
                    return Ok(DecodeStep {
                        consumed: 0,
                        written: 0,
                        finished: true,
                    });
                }
                let mut source = input;
                if *reset {
                    if source.len() < 18 && !end {
                        return Ok(DecodeStep {
                            consumed: 0,
                            written: 0,
                            finished: false,
                        });
                    }
                    decoder.reset(&mut source).map_err(|_| corrupt())?;
                    consumed = input.len() - source.len();
                    *reset = false;
                    *boundary = false;
                }
                let result = decoder
                    .decode_from_to(source, output)
                    .map_err(|_| corrupt())?;
                consumed += result.0;
                written = result.1;
                if decoder.is_finished() && decoder.can_collect() == 0 {
                    if let Some(expected) = decoder.get_checksum_from_data()
                        && decoder.get_calculated_checksum() != Some(expected)
                    {
                        return Err(corrupt());
                    }
                    *boundary = true;
                    *reset = true;
                    finished = end && consumed == input.len();
                } else if end && consumed == 0 && written == 0 {
                    return Err(corrupt());
                }
            }
        }
        if written > self.limit.saturating_sub(self.total) {
            return Err(Error::new(
                "UND_ERR_RES_EXCEEDED_MAX_SIZE",
                "decoded body exceeds limit",
            ));
        }
        self.total += written;
        self.done = finished;
        Ok(DecodeStep {
            consumed,
            written,
            finished,
        })
    }
}
fn gzip_header(input: &[u8]) -> Result<Option<usize>> {
    if input.len() < 10 {
        return Ok(None);
    }
    if input[..3] != [31, 139, 8] || input[3] & 0xe0 != 0 {
        return Err(corrupt());
    }
    let flags = input[3];
    let mut pos = 10;
    if flags & 4 != 0 {
        if input.len() < 12 {
            return Ok(None);
        }
        pos = 12 + u16::from_le_bytes(input[10..12].try_into().unwrap()) as usize;
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            if pos > input.len() {
                break;
            }
            if let Some(n) = input[pos..].iter().position(|b| *b == 0) {
                pos += n + 1;
            } else {
                if input.len() > 8192 {
                    return Err(corrupt());
                }
                return Ok(None);
            }
        }
    }
    if pos > 8192 {
        return Err(corrupt());
    }
    if input.len() < pos {
        return Ok(None);
    }
    if flags & 2 != 0 {
        if input.len() < pos + 2 {
            return Ok(None);
        }
        let mut crc = flate2::Crc::new();
        crc.update(&input[..pos]);
        if crc.sum() as u16 != u16::from_le_bytes(input[pos..pos + 2].try_into().unwrap()) {
            return Err(corrupt());
        }
        pos += 2;
    }
    Ok(Some(pos))
}
