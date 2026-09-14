#![deny(unsafe_op_in_unsafe_fn)]
//! Standalone Windows mechanism probes. Runtime tests require a Windows host.

#[cfg(windows)]
#[path = "../backend_draft/mod.rs"]
pub mod backend_draft;
#[cfg(windows)]
pub mod console;
#[cfg(windows)]
pub mod integration;
#[cfg(windows)]
mod operation;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod port;
#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod stdio;
#[cfg(windows)]
pub mod tcp;
#[cfg(windows)]
pub mod timer;
