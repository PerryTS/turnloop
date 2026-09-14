#![deny(unsafe_op_in_unsafe_fn)]
//! Host-driven, completion-shaped I/O. No thread is created by the driver.

/// Backend selected from the compilation target, independently of Cargo features.
pub const BACKEND_NAME: &str = env!("TURNLOOP_BACKEND");

pub mod backend;
mod buffer;
mod types;
pub use buffer::*;
pub use types::*;
mod completion;
mod driver;
mod table;
#[doc(hidden)]
pub mod timer;
pub use completion::*;
pub use driver::Driver;

mod notifier;
mod queue;
mod sync;
pub use notifier::{Notifier, PostError, Poster};

#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub type Loop = Driver<backend::Platform>;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub use backend::unix::Detached;

mod blocking;
pub use blocking::{DnsRequest, PoolConfig};

mod time;
pub use time::Instant;

#[cfg(all(test, not(loom)))]
mod portable_tests;
