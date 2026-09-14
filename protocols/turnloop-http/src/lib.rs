//! Sans-I/O HTTP codecs and host-driven client policy. No socket, executor or clock reads.
//!
//! # Getting started on turnloop
//! Enable the `turnloop` feature for the `asynchronous` module. The embedding
//! host owns `LocalExecutor` and calls `turn`; adapters await its streams and
//! deadline futures. See the crate README and turnloop-io for ownership, streaming
//! and cancellation examples. Default features retain the sans-I/O API.
#![deny(unsafe_op_in_unsafe_fn)]
pub mod client;
pub mod compression;
pub mod hpack;
pub mod http1;
pub mod http2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: &'static str,
}
impl Error {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

mod recycling;

#[cfg(feature = "turnloop")]
pub mod asynchronous;
