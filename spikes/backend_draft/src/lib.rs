#![deny(unsafe_op_in_unsafe_fn)]
//! Temporary adaptation surface based on DESIGN §6; NOT the unpublished core trait.
use std::time::{Duration, Instant};
#[cfg(all(target_os = "wasi", target_env = "p2"))]
pub mod wasi_p2;
pub mod wasi_p3;
pub mod web;
#[derive(Clone, Copy, Debug)]
pub enum Timeout {
    Now,
    After(Duration),
    Until(Instant),
    Forever,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integration {
    RuntimeOwned,
    HostCallback,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnInfo {
    pub completions: usize,
    pub waited: bool,
}
/// Core owns liveness, tokens, provided buffers, close ordering and deadline selection.
/// The input timeout here is already min(host budget, next core timer deadline).
pub trait DraftBackend {
    type Completion;
    type Error;
    fn integration(&self) -> Integration;
    /// Replaces caller-owned output in reserved storage. Implementations never dispatch user handlers.
    fn turn(
        &mut self,
        timeout: Timeout,
        out: &mut Vec<Self::Completion>,
    ) -> Result<TurnInfo, Self::Error>;
}
