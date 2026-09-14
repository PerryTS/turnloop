//! Internal platform contract, revision 2 (empty-wait instrumentation and native services).
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
//! * `prepare_close` starts nonblocking teardown before cancellation. Process
//!   backends terminate the owned child and retain its exit operation until reaped,
//!   including when cancellation is already pending. Other resources need no hook.
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
//!   counts OS waits with no native I/O or notifier events, including EINTR.
//!   Private timeout events (e.g. timerfd expiry) count as zero-event waits, just
//!   like a timed OS wait returning zero; never infer this from user completions.
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
    /// Wake the native wait primitive; the notifier calls this only after observing PARKED.
    fn wake(&self) -> Result<()>;
    /// Return the number of native wake syscall attempts, including failures.
    fn syscall_count(&self) -> u64;
}

#[derive(Debug)]
/// One accepted backend operation, keyed by a full generational operation ID.
pub struct Request {
    /// Full generational operation identity, including its owning loop.
    pub op: OpId,
    /// Resource identity belonging to the submitting or receiving loop.
    pub handle: Handle,
    /// Requested operation and any memory it retains.
    pub operation: Operation,
}
#[derive(Debug)]
/// Backend requests with completion-shaped ownership and cancellation semantics.
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
    /// Establish the connection configured when the stream was opened.
    Connect,
    /// Accept one or repeatedly accept incoming stream connections.
    Accept {
        /// Continue producing nonterminal results until stopped, cancelled, EOF or error.
        multishot: bool,
    },
    /// Read bytes, with a byte count and optional pooled lease in the completion.
    Read {
        /// The owned/provided buffer for this operation.
        buf: ReadBuf,
        /// Continue producing nonterminal results until stopped, cancelled, EOF or error.
        multishot: bool,
    },
    /// Write the complete supplied byte buffer, permitting native partial writes internally.
    Write(WriteBuf),
    /// Write all inline segments in order.
    Writev(WriteVectored),
    /// Send one UDP datagram to the specified destination.
    SendTo {
        /// The owned/provided buffer for this operation.
        buf: WriteBuf,
        /// Destination UDP endpoint.
        to: SocketAddr,
    },
    /// Receive one UDP datagram and report its source address.
    RecvFrom(ReadBuf),
    /// Complete a stream write-side shutdown after preceding queued writes.
    Shutdown,
}
#[derive(Debug)]
/// Backend completion awaiting translation into a host-visible completion.
pub struct Event<D> {
    /// Full generational operation identity, including its owning loop.
    pub op: OpId,
    /// True once this operation will produce no further results or native buffer accesses.
    pub terminal: bool,
    /// The operation outcome or its original error.
    pub result: std::result::Result<Outcome<D>, Error>,
}
#[derive(Debug)]
/// Successful backend outcomes; accepted transports remain owned until attached.
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
    /// The connection is established and ready for stream operations.
    Connected,
    /// An incoming TCP connection with its peer address.
    Accepted {
        /// Exclusive ownership of an unregistered accepted resource.
        transport: D,
        /// Remote TCP peer endpoint.
        peer: SocketAddr,
    },
    /// Read bytes, with a byte count and optional pooled lease in the completion.
    Read {
        /// Number of bytes transferred by this completion.
        n: usize,
        /// Owned pooled read bytes, present only when a pooled buffer was used.
        lease: Option<BufLease>,
    },
    /// The peer or file reached the end of its readable stream.
    Eof,
    /// Number of bytes successfully written.
    Wrote(usize),
    /// Receive one UDP datagram and report its source address.
    RecvFrom {
        /// Number of bytes transferred by this completion.
        n: usize,
        /// Source endpoint of the received datagram.
        from: SocketAddr,
        /// Owned pooled read bytes, present only when a pooled buffer was used.
        lease: Option<BufLease>,
    },
    /// Complete a stream write-side shutdown after preceding queued writes.
    Shutdown,
    /// The operation was cancelled and its native buffer access has ended.
    Cancelled,
}
#[derive(Clone, Copy, Debug, Default)]
/// Instrumentation for the single bounded backend wait.
pub struct PollInfo {
    /// Actual OS wait invocations, at most one for this poll.
    pub waits: u32,
    /// OS waits with no native I/O or notifier events (including interrupted waits).
    /// Private timeout events, such as timerfd expiry, are not native work.
    pub zero_event_waits: u32,
}

/// # Safety
/// Implementers must uphold the buffer-lifetime and native-quiescence guarantees
/// above: no native I/O may access a buffer after its terminal Event or Drop.
/// Core lifetime and handle-release safety relies on those guarantees.
pub unsafe trait Backend: Sized + 'static {
    /// Thread-safe native wake endpoint type.
    type Wake: Wake;
    /// Owning transferable representation of an unregistered resource.
    type Detached: Send + 'static;
    /// Construct backend storage and native wait resources with the supplied shared read pool.
    fn new(config: &Config, pool: BufferPool) -> Result<Self>;
    /// Give process-wide services this loop's parking-aware notification endpoint.
    fn set_notifier(&mut self, _notifier: crate::Notifier) {}
    /// Spawn and bind the child and requested parent pipe handles atomically.
    /// Failure must release all supplied handles and reap any created child.
    fn spawn(
        &mut self,
        _handle: Handle,
        _pipes: [Option<Handle>; 3],
        _spec: &crate::ProcessSpec,
    ) -> Result<u32> {
        Err(Error::new(crate::ErrorKind::Unsupported))
    }
    /// Begin nonblocking resource teardown before the core cancels its operations.
    /// Process backends terminate the owned child/tree here and defer its terminal
    /// acknowledgement until exit/reaping, so release never blocks a normal turn.
    /// Other resources need no action. An error leaves the core handle open.
    fn prepare_close(&mut self, _handle: Handle) -> Result<()> {
        Ok(())
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
    /// Clone the lifetime-safe native wake endpoint for this backend.
    fn waker(&self) -> Arc<Self::Wake>;
    /// Bind a new resource to the supplied handle, retaining nothing on error.
    fn open(&mut self, handle: Handle, spec: Open) -> Result<()>;
    /// Return the local IP endpoint of a bound socket.
    fn local_addr(&self, handle: Handle) -> Result<SocketAddr>;
    /// Accept one operation or reject it before retaining native buffer access.
    fn submit(&mut self, request: Request) -> Result<()>;
    /// Request cancellation; terminal acknowledgement must precede memory reuse.
    fn cancel(&mut self, op: OpId) -> Result<()>;
    /// Whether completions or immediately runnable cached operations are available.
    fn has_work(&self) -> bool;
    /// Append bounded completions, performing at most one OS wait with the supplied exact timeout.
    fn poll(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<Event<Self::Detached>>,
    ) -> Result<PollInfo>;
    /// Release a quiescent resource after Closed was appended to host output.
    fn release(&mut self, handle: Handle);
    /// Unregister a quiescent resource and return its owning transferable representation.
    fn detach(&mut self, handle: Handle) -> Result<Self::Detached>;
    /// Register an owning transport on this loop; failure drops the rejected transport.
    fn attach(&mut self, handle: Handle, transport: Self::Detached) -> Result<()>;
    /// Opt into external host waiting and return the borrowed integration primitive.
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
mod services;
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod signals;

#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
mod files;
