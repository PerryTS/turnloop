#![deny(unsafe_op_in_unsafe_fn, missing_docs)]
//! Host-driven, completion-shaped I/O. No background thread turns the loop.
//!
//! The host owns the thread and decides when to collect completions. Optional
//! process-wide blocking, signal and external-wait services start only on use.
//!
//! ```
//! # #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd"))]
//! # fn main() -> turnloop::Result<()> {
//! use std::time::Duration;
//! use turnloop::{Completions, Config, Loop, OpResult, Timeout, Token};
//! let mut driver = Loop::new(Config::default())?;
//! let deadline = driver.now() + Duration::from_millis(2);
//! let timer = driver.timer(deadline, None, Token(42))?;
//! let mut completions = Completions::default();
//! loop {
//!     driver.turn(Timeout::Until(deadline), &mut completions)?;
//!     if completions.iter().any(|c| c.token == Token(42) && matches!(c.result, OpResult::Timer)) {
//!         break;
//!     }
//! }
//! // Run host callbacks only after turn has returned.
//! driver.close(timer, Token(43))?;
//! driver.turn(Timeout::Now, &mut completions)?;
//! assert!(!driver.alive());
//! # Ok(()) }
//! # #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd")))]
//! # fn main() {}
//! ```
//!
//! Enable the `executor` feature for `executor::LocalExecutor` and futures-io
//! adapters. Each loop/executor belongs to its creating thread; its notifier and
//! poster may be cloned onto other threads.

/// Backend selected from the compilation target, independently of Cargo features.
pub const BACKEND_NAME: &str = env!("TURNLOOP_BACKEND");

pub mod backend;
mod buffer;
mod types;
pub use buffer::*;
pub use types::*;
mod native;
pub use native::*;
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

#[cfg(any(
    turnloop_backend = "kqueue",
    turnloop_backend = "epoll",
    turnloop_backend = "wasi_p2",
    turnloop_backend = "web",
    all(turnloop_backend = "wasi_p3", feature = "wasi-p3-experimental")
))]
/// The platform driver selected for the compilation target; owned by one agent.
pub type Loop = Driver<backend::Platform>;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub use backend::unix::Detached;

mod blocking;
pub use blocking::{DnsRequest, PoolConfig};

mod time;
pub use time::Instant;

mod external_wait;
pub use external_wait::{WaitCondition, WaitResult};

#[cfg(feature = "executor")]
pub mod executor;

#[cfg(feature = "executor")]
pub use executor::{
    AsyncIo, Blocking, Close, ExecutorConfig, ExecutorHandle, JoinError, JoinHandle, LocalExecutor,
    Sleep,
};

#[cfg(all(test, not(loom)))]
mod portable_tests;
