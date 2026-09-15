//! Sans-IO MongoDB components. The host owns sockets, TLS, DNS, entropy and time.
//! Call `Connection::connected`, complete explicit TLS requests, then drive bytes
//! through `transmit` / `consume_transmit` and `receive` / `poll_event`.
//! One operation per connection follows MongoDB's request/response turn-taking.
//! Raw BSON replies borrow the receive buffer until `release_reply`.
//!
//! # Getting started on turnloop
//! Enable the `turnloop` feature for the `asynchronous` module. The embedding
//! host owns `LocalExecutor` and calls `turn`; adapters await its streams and
//! deadline futures. See the crate README and turnloop-io for ownership, streaming
//! and cancellation examples. Default features retain the sans-I/O API.
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

#[cfg(all(target_os = "wasi", target_env = "p3"))]
use turnloop_wasi_random as _;

pub use bson;
pub mod auth;
pub mod command;
pub mod connection;
pub mod error;
pub mod operation;
pub mod pool;
pub mod retry;
pub mod session;
pub mod time;
pub mod topology;
pub use time::Instant;
pub mod uri;
pub mod wire;
mod zlib;
pub use connection::{Connection, ConnectionEvent};
pub use error::{Error, ErrorKind, Result};

#[cfg(feature = "turnloop")]
pub mod asynchronous;
