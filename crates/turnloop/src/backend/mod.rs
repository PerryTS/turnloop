//! Internal platform contract, revision 2 (empty-wait instrumentation).
//!
//! Public only so the contract runner and independently developed backends can use
//! it; not a stable end-user extension API. All identifiers and buffers are core
//! types. No raw fd, OVERLAPPED pointer, pollable or browser object crosses here.
//!
//! # Ownership and completion rules (D1–D4)
//!
//! * `open`/`attach` bind a resource to the supplied generational Handle. On error
//!   they retain nothing. Open::Tcp creates an unconnected resource; the Connect
//!   request performs the connection. Once validated/accepted, even a synchronous
//!   I/O error is queued as a terminal Event. Submit errors reject before acceptance
//!   (unsupported/invalid operation, capacity/resource setup failure).
//!   `submit` accepts ownership of a Request only on Ok; on
//!   Err it must have stopped all access to its buffers before returning.
//! * Every accepted Request yields one terminal Event, including cancellation and
//!   errors. Multishot accepts/reads may yield preceding nonterminal Events. Their
//!   terminal Event acknowledges that buffers and native op storage are quiescent.
//! * Native completions carry the **full OpId**, never a token or a reusable index
//!   alone. Delayed kernel events must not complete a later generation. IOCP must
//!   keep OVERLAPPED storage pinned until the final kernel acknowledgement.
//! * `cancel` initiates cancellation, never frees native buffers prematurely. It
//!   must eventually emit a terminal event even when cancel races with success.
//!   The core arbitrates the result and emits Cancelled if cancellation won.
//! * The core calls `release` only after every associated operation terminated
//!   and the Closed completion was appended to host output. Drop must synchronously
//!   quiesce any remaining I/O before freeing buffers (native completion draining
//!   may be necessary). The driver never invokes user callbacks.
//!
//! # Turn and boundedness (D5–D7)
//!
//! * `poll` appends at most `events.capacity() - events.len()` Events; never grows
//!   the vector. A full buffer retains work for the next turn, without losing an
//!   edge. It performs at most one blocking OS wait, and zero waits if work is
//!   already queued or can be executed using cached readiness. `None` means an
//!   unbounded wait. Durations must retain sub-millisecond precision.
//! * Readiness backends execute I/O in poll, cache readiness until EAGAIN, and
//!   requeue partially processed work fairly. Completion backends drain native
//!   completions. WASI 0.2 polls pollables; 0.3 drives a waitable set; web drains
//!   callback results and rejects blocking waits on the main thread.
//! * EINTR ends this poll early; it must not restart the timeout. `PollInfo.waits`
//!   reports actual wait invocations for the contract tests; `zero_event_waits`
//!   counts OS waits that returned zero native events, including EINTR.
//! * Cached readiness ending in EAGAIN with no completion must retain the original
//!   timeout for the one permitted OS wait. It must not force a zero-timeout turn.
//! * `has_work` covers queued completions and cached runnable I/O. `wake` is called
//!   only after the notifier observed PARKED. It must be safe after loop drop:
//!   the wake object owns its native resource or detects closure, never uses a
//!   possibly reused raw handle. Errors cannot silently lose a wake.
//! * `integration` opts into external waiting. The core then leaves its notifier
//!   PARKED between turns; hosts call it again only while preparing external waits.
//!   This reconciles a readable integration fd with zero syscalls while RUNNING.
//!
//! # Transfer and platform constraints (§5a, §7)
//!
//! * `detach` is called only when all operations are quiescent. It unregisters the
//!   source loop before returning an owning Send transport. `attach` re-registers
//!   on the destination. Unsupported resources return Unsupported. For asynchronous
//!   cancellation, public detach can return WouldBlock while cancellation drains;
//!   the caller turns the source loop and retries. No blocking wait is hidden here.
//! * `Detached` may be an enum, not an integer: IOCP socket migration may require
//!   duplication; WASI/browser objects need not support transfer. `Resource` is
//!   intentionally absent: each backend owns its native tables keyed by Handle.
//! * `Wake` is Send + Sync on every target, but single-thread WASI/web may implement
//!   it as a flag/host scheduler token. No backend creates a loop-driving thread.
//!   Windows' explicitly requested Integration::Event helper is the D7 exception.
//! * Unsupported platforms/capabilities return errors; no fake successful I/O.
use crate::{
    BufLease, BufferPool, Config, Error, Handle, Integration, OpId, Open, ReadBuf, Result,
    WriteBuf, WriteVectored,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};

/// Only the notifier calls this method; implementations should expose syscall
/// counts here, counting attempts (including failures), not guessed notifications.
pub trait Wake: Send + Sync + 'static {
    fn wake(&self) -> Result<()>;
    fn syscall_count(&self) -> u64;
}

#[derive(Debug)]
pub struct Request {
    pub op: OpId,
    pub handle: Handle,
    pub operation: Operation,
}
#[derive(Debug)]
pub enum Operation {
    /// Observe this process until its exit has been reaped.
    ProcessExit,
    /// Multishot subscription to this signal resource.
    WatchSignal,
    /// Duplicate a transport into the receiving process. The backend retains an
    /// independent native reference until its send completes or is cancelled.
    SendHandle(Handle),
    /// Receive one transport over a local IPC stream.
    RecvHandle,
    Connect,
    Accept { multishot: bool },
    Read { buf: ReadBuf, multishot: bool },
    Write(WriteBuf),
    Writev(WriteVectored),
    SendTo { buf: WriteBuf, to: SocketAddr },
    RecvFrom(ReadBuf),
    Shutdown,
}
#[derive(Debug)]
pub struct Event<D> {
    pub op: OpId,
    pub terminal: bool,
    pub result: std::result::Result<Outcome<D>, Error>,
}
#[derive(Debug)]
pub enum Outcome<D> {
    /// Reaped child termination.
    Exited(crate::ExitStatus),
    /// Coalesced signal delivery.
    Signal(crate::Signal),
    /// Accepted local connection (has no IP peer address).
    PipeAccepted(D),
    /// Received independent transport ownership.
    HandleReceived(D),
    /// One transport was passed successfully.
    HandleSent,
    Connected,
    Accepted {
        transport: D,
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
    Cancelled,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct PollInfo {
    pub waits: u32,
    /// OS waits returning zero native events (including interrupted waits).
    pub zero_event_waits: u32,
}

/// # Safety
/// Implementers must uphold the buffer-lifetime and native-quiescence guarantees
/// above: no native I/O may access a buffer after its terminal Event or Drop.
/// Core lifetime and handle-release safety relies on those guarantees.
pub unsafe trait Backend: Sized + 'static {
    type Wake: Wake;
    type Detached: Send + 'static;
    fn new(config: &Config, pool: BufferPool) -> Result<Self>;
    /// Give process-wide services this loop's parking-aware notification endpoint.
    fn set_notifier(&mut self, _notifier: crate::Notifier) {}
    /// Spawn and bind the child and requested parent pipe handles atomically.
    /// Failure must release all supplied handles and reap any created child.
    fn spawn(&mut self, _handle: Handle, _pipes: [Option<Handle>; 3], _spec: &crate::ProcessSpec) -> Result<u32> {
        Err(Error::new(crate::ErrorKind::Unsupported))
    }
    /// Signal an owned child, or its explicitly isolated process group.
    fn kill(&mut self, _handle: Handle, _signal: crate::Signal, _group: bool) -> Result<()> {
        Err(Error::new(crate::ErrorKind::Unsupported))
    }
    /// Bind a process-wide signal subscription to a new core handle.
    fn signal(&mut self, _handle: Handle, _signal: crate::Signal) -> Result<()> {
        Err(Error::new(crate::ErrorKind::Unsupported))
    }
    /// Set a terminal mode, retaining the original settings until release/drop.
    fn tty_set_mode(&mut self, _handle: Handle, _mode: crate::TtyMode) -> Result<()> {
        Err(Error::new(crate::ErrorKind::Unsupported))
    }
    /// Query current terminal dimensions.
    fn tty_window_size(&self, _handle: Handle) -> Result<crate::WindowSize> {
        Err(Error::new(crate::ErrorKind::Unsupported))
    }
    /// Monotonic, nondecreasing time in a consistent domain across calls. Native
    /// adapters return std::time::Instant::now(). Web adapters construct
    /// crate::Instant::from_duration from their monotonic host clock. This method
    /// must not schedule work, block, or invoke a user callback.
    fn now(&self) -> crate::Instant;
    /// Validate the original request even when queued completions eliminate the
    /// effective wait. Browser main-thread adapters reject every timeout but Now;
    /// a worker adapter may support blocking. Native adapters use this default.
    fn validate_timeout(&self, _timeout: crate::Timeout) -> Result<()> {
        Ok(())
    }
    /// The core owns timer semantics. Callback-driven adapters arm one host timer
    /// for this earliest deadline, whose callback schedules a turn. None disarms
    /// it. Native pollers need no action: poll receives the effective timeout.
    /// This is a scheduling import, never a user callback. If host scheduling
    /// fails, preserve the failure and return it from poll; never fake expiry.
    fn deadline_changed(&mut self, _deadline: Option<crate::Instant>) {}
    fn waker(&self) -> Arc<Self::Wake>;
    fn open(&mut self, handle: Handle, spec: Open) -> Result<()>;
    fn local_addr(&self, handle: Handle) -> Result<SocketAddr>;
    fn submit(&mut self, request: Request) -> Result<()>;
    fn cancel(&mut self, op: OpId) -> Result<()>;
    fn has_work(&self) -> bool;
    fn poll(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<Event<Self::Detached>>,
    ) -> Result<PollInfo>;
    fn release(&mut self, handle: Handle);
    fn detach(&mut self, handle: Handle) -> Result<Self::Detached>;
    fn attach(&mut self, handle: Handle, transport: Self::Detached) -> Result<()>;
    fn integration(&mut self) -> Result<Integration>;
}

#[cfg(turnloop_backend = "epoll")]
mod epoll;
#[cfg(turnloop_backend = "kqueue")]
mod kqueue;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod poller;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod socket;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub mod unix;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub use unix::Unix as Platform;

#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod ipc;

#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod signals;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod services;

#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod files;
