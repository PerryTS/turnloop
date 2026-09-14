use crate::{BufLease, Error, Handle, OpId, Token};
use std::{any::Any, net::SocketAddr, time::Duration};

/// Common small posts need no box/allocation. Larger host results can own bytes or
/// a Send Rust value; the driver never invokes a function from this payload.
#[derive(Debug)]
pub enum Payload {
    U64(u64),
    Bytes(Vec<u8>),
    Boxed(Box<dyn Any + Send>),
}
pub type BlockingResult = crate::Result<Payload>;

#[derive(Debug)]
pub struct Completion {
    pub token: Token,
    /// None for unsolicited posts and the final Closed notification.
    pub op: Option<OpId>,
    pub handle: Option<Handle>,
    pub terminal: bool,
    pub result: OpResult,
}
#[derive(Debug)]
pub enum OpResult {
    /// Child exit, after reaping; produced exactly once per accepted spawn.
    Exited(crate::ExitStatus),
    /// Process-wide signal delivered to this subscribed loop.
    Signal(crate::Signal),
    /// Accepted local stream, already attached to this loop.
    PipeAccepted { conn: Handle },
    /// Received transport, already attached to this loop.
    HandleReceived { handle: Handle },
    /// One transport was passed to the receiving process.
    HandleSent,
    Connected,
    Accepted {
        conn: Handle,
        peer: SocketAddr,
    },
    Read {
        n: usize,
        lease: Option<BufLease>,
    },
    Eof,
    Wrote(usize),
    RecvFrom {
        n: usize,
        from: SocketAddr,
        lease: Option<BufLease>,
    },
    Shutdown,
    Timer,
    Posted(Payload),
    Blocking(Payload),
    Resolved(Vec<SocketAddr>),
    Cancelled,
    Closed,
    Stopped,
    Err(Error),
}
/// Fixed-capacity output. Each turn clears old completions and fills at most this
/// capacity. Retained completions remain in the driver, providing backpressure.
#[derive(Debug)]
pub struct Completions {
    pub(crate) entries: Vec<Completion>,
}
impl Completions {
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.entries.capacity()
    }
    pub fn iter(&self) -> std::slice::Iter<'_, Completion> {
        self.entries.iter()
    }
    pub fn drain(&mut self) -> std::vec::Drain<'_, Completion> {
        self.entries.drain(..)
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
impl Default for Completions {
    fn default() -> Self {
        Self::with_capacity(256)
    }
}
impl std::ops::Deref for Completions {
    type Target = [Completion];
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
#[derive(Clone, Copy, Debug)]
pub struct TurnInfo {
    pub completions: usize,
    pub waited: Duration,
    pub alive: bool,
    pub os_waits: u32,
    /// Number of OS waits that returned zero native events.
    pub zero_event_waits: u32,
}
