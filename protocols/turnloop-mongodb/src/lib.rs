//! Sans-IO MongoDB components. The host owns sockets, TLS, DNS, entropy and time.
//! Call `Connection::connected`, complete explicit TLS requests, then drive bytes
//! through `transmit` / `consume_transmit` and `receive` / `poll_event`.
//! One operation per connection follows MongoDB's request/response turn-taking.
//! Raw BSON replies borrow the receive buffer until `release_reply`.
#![deny(unsafe_op_in_unsafe_fn)]
#![forbid(unsafe_code)]

pub use bson;
pub mod auth;
pub mod command;
pub mod connection;
pub mod error;
pub mod operation;
pub mod pool;
pub mod retry;
pub mod session;
pub mod topology;
pub mod uri;
pub mod wire;
pub use connection::{Connection, ConnectionEvent};
pub use error::{Error, ErrorKind, Result};
