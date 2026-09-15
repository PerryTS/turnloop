use crate::{
    backend::{Backend, Event, Operation, Outcome, Request},
    table::Table,
    timer::DriverTimerQueue as TimerQueue,
    *,
};
use std::{
    collections::VecDeque,
    marker::PhantomData,
    net::SocketAddr,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, ThreadId},
    time::Duration,
};
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
const NATIVE_EVENTS: usize = 0;
const TIMER_EVENTS: usize = 1;
const POST_EVENTS: usize = 2;
#[derive(Clone, Copy)]
enum Kind {
    Socket,
    Timer {
        op: Option<OpId>,
        repeat: Option<Duration>,
    },
}
struct Resource {
    kind: Kind,
    referenced: bool,
    pending: usize,
    closing: Option<Token>,
    closed_queued: bool,
    head: Option<OpId>,
    tail: Option<OpId>,
}
#[derive(Clone)]
struct Op {
    handle: Option<Handle>,
    token: Token,
    cancel: bool,
    timed_out: bool,
    stop: bool,
    external_wait: bool,
    job_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    previous: Option<OpId>,
    next: Option<OpId>,
}

struct Queued {
    completion: Completion,
    referenced: bool,
    event_class: Option<usize>,
}
impl std::ops::Deref for Queued {
    type Target = Completion;
    fn deref(&self) -> &Completion {
        &self.completion
    }
}

/// A driver belongs to its constructing thread. Backend is an internal extension
/// point used by platform lanes and the common contract suite.
pub struct Driver<B: Backend> {
    backend: B,
    notifier: Notifier,
    poster: Poster,
    external: bool,
    work_port: std::sync::Arc<crate::blocking::WorkPort>,
    owner: u64,
    thread: ThreadId,
    handles: Table<Resource>,
    ops: Table<Op>,
    timers: TimerQueue,
    connect_deadlines: TimerQueue,
    queued: VecDeque<Queued>,
    buffered: [usize; 3],
    events: Vec<Event<B::Detached>>,
    refs: usize,
    outstanding: usize,
    native_pending: usize,
    config: Config,
    _local: PhantomData<Rc<()>>,
}
impl<B: Backend> Driver<B> {
    /// Construct a loop on this thread and allocate its fixed operation and completion storage.
    pub fn new(config: Config) -> Result<Self> {
        if config.max_handles == 0
            || config.max_handles > u32::MAX as usize
            || config.max_operations == 0
            || config.max_operations > u32::MAX as usize
            || config.events_per_turn == 0
            || config.post_capacity == 0
            || config.pooled_buffer_size == 0
        {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let completion_capacity = config
            .events_per_turn
            .checked_mul(3)
            .and_then(|n| n.checked_add(config.max_operations))
            .and_then(|n| n.checked_add(config.max_handles))
            .ok_or(Error::new(ErrorKind::InvalidInput))?;
        let mut backend = B::new(
            &config,
            BufferPool::new(config.pooled_buffers, config.pooled_buffer_size),
        )?;
        let notifier = Notifier::new(backend.waker());
        backend.set_notifier(notifier.clone());
        let poster = Poster::new(config.post_capacity, notifier.clone());
        let work_port = crate::blocking::WorkPort::new(config.max_operations, notifier.clone());
        let owner = loop {
            let current = NEXT_OWNER.load(Ordering::Relaxed);
            let next = current
                .checked_add(1)
                .ok_or(Error::new(ErrorKind::ResourceLimit))?;
            if NEXT_OWNER
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break current;
            }
        };
        Ok(Self {
            backend,
            notifier,
            poster,
            external: false,
            work_port,
            owner,
            thread: thread::current().id(),
            handles: Table::new(config.max_handles),
            ops: Table::new(config.max_operations),
            timers: TimerQueue::new(config.max_handles),
            connect_deadlines: TimerQueue::new(config.max_operations),
            // Terminal operation and Closed credits live until delivery. Native
            // multishot events, repeating timers and posts each have their own
            // bounded reserve, so no source can consume cancellation capacity or
            // prevent another source from making progress with small host output.
            queued: VecDeque::with_capacity(completion_capacity),
            buffered: [0; 3],
            events: Vec::with_capacity(config.events_per_turn),
            refs: 0,
            outstanding: 0,
            native_pending: 0,
            config,
            _local: PhantomData,
        })
    }
    fn assert_owner(&self) {
        debug_assert_eq!(
            self.thread,
            thread::current().id(),
            "completion on foreign thread"
        );
    }
    fn resource(&self, h: Handle) -> Result<&Resource> {
        if h.owner != self.owner {
            return Err(Error::new(ErrorKind::NotFound));
        }
        self.handles
            .get(h.key)
            .ok_or(Error::new(ErrorKind::NotFound))
    }
    fn new_handle(&mut self, kind: Kind) -> Result<Handle> {
        let key = self
            .handles
            .insert(Resource {
                kind,
                referenced: true,
                pending: 0,
                closing: None,
                closed_queued: false,
                head: None,
                tail: None,
            })
            .ok_or(Error::new(ErrorKind::ResourceLimit))?;
        if matches!(kind, Kind::Socket) {
            self.refs += 1;
        }
        Ok(Handle {
            owner: self.owner,
            key,
        })
    }
    fn new_op(&mut self, h: Option<Handle>, token: Token) -> Result<OpId> {
        if self.outstanding == self.config.max_operations {
            return Err(Error::new(ErrorKind::ResourceLimit));
        }
        let previous = h.and_then(|h| self.handles.get(h.key).and_then(|r| r.tail));
        let key = self
            .ops
            .insert(Op {
                handle: h,
                token,
                cancel: false,
                timed_out: false,
                stop: false,
                job_cancel: None,
                external_wait: false,
                previous,
                next: None,
            })
            .ok_or(Error::new(ErrorKind::ResourceLimit))?;
        let id = OpId {
            owner: self.owner,
            key,
        };
        if let Some(previous) = previous {
            self.ops.get_mut(previous.key).expect("previous").next = Some(id);
        }
        if let Some(h) = h {
            let r = self.handles.get_mut(h.key).expect("validated handle");
            if r.head.is_none() {
                r.head = Some(id);
            }
            r.tail = Some(id);
            r.pending += 1;
            if matches!(r.kind, Kind::Socket) {
                self.native_pending += 1;
            }
            if r.referenced {
                self.refs += 1;
            }
        } else {
            self.refs += 1;
        }
        self.outstanding += 1;
        Ok(OpId {
            owner: self.owner,
            key,
        })
    }
    fn retire(&mut self, id: OpId) -> Option<Op> {
        let op = self.ops.remove(id.key)?;
        self.connect_deadlines.cancel(id.key);
        if let Some(previous) = op.previous {
            self.ops.get_mut(previous.key).expect("previous").next = op.next;
        }
        if let Some(next) = op.next {
            self.ops.get_mut(next.key).expect("next").previous = op.previous;
        }
        if let Some(h) = op.handle {
            if let Some(r) = self.handles.get_mut(h.key) {
                if r.head == Some(id) {
                    r.head = op.next;
                }
                if r.tail == Some(id) {
                    r.tail = op.previous;
                }
                r.pending -= 1;
                if matches!(r.kind, Kind::Socket) {
                    self.native_pending -= 1;
                }
                if r.referenced {
                    self.refs -= 1;
                }
                if let Kind::Timer { op, .. } = &mut r.kind {
                    *op = None;
                }
            }
        } else {
            self.refs -= 1;
        }
        Some(op)
    }
    fn maybe_closed(&mut self, h: Handle) {
        if let Some(r) = self.handles.get_mut(h.key)
            && r.pending == 0
            && !r.closed_queued
            && let Some(token) = r.closing
        {
            r.closed_queued = true;
            self.enqueue(
                Completion {
                    token,
                    op: None,
                    handle: Some(h),
                    terminal: true,
                    result: OpResult::Closed,
                },
                false,
            );
        }
    }
    fn enqueue(&mut self, completion: Completion, referenced: bool) {
        let event_class = match completion.result {
            OpResult::Posted(_) => Some(POST_EVENTS),
            OpResult::Timer if !completion.terminal => Some(TIMER_EVENTS),
            _ if !completion.terminal => Some(NATIVE_EVENTS),
            _ => None,
        };
        if let Some(class) = event_class {
            debug_assert!(self.buffered[class] < self.config.events_per_turn);
            self.buffered[class] += 1;
        }
        debug_assert!(self.queued.len() < self.queued.capacity());
        self.refs += usize::from(referenced);
        self.queued.push_back(Queued {
            completion,
            referenced,
            event_class,
        });
    }
    fn finish(&mut self, id: OpId, result: OpResult, terminal: bool) {
        let Some(op) = self.ops.get(id.key).cloned() else {
            return;
        };
        let referenced = terminal
            && op
                .handle
                .is_none_or(|h| self.handles.get(h.key).is_some_and(|r| r.referenced));
        self.enqueue(
            Completion {
                token: op.token,
                op: Some(id),
                handle: op.handle,
                terminal,
                result,
            },
            referenced,
        );
        if terminal {
            self.retire(id);
            if let Some(h) = op.handle {
                self.maybe_closed(h);
            }
        }
    }
    /// Return O(1) reference-counted liveness, including undelivered terminal completions.
    pub fn alive(&self) -> bool {
        self.refs != 0
    }
    /// The backend monotonic clock; portable deadline construction starts here.
    pub fn now(&self) -> Instant {
        self.backend.now()
    }
    /// Return the earliest pending timer or connection deadline in the backend clock domain.
    pub fn next_deadline(&self) -> Option<Instant> {
        let deadline = match (
            self.timers.next_deadline(),
            self.connect_deadlines.next_deadline(),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        #[cfg(target_arch = "wasm32")]
        let deadline = match (deadline, crate::external_wait::deadline(self.owner)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        deadline
    }
    /// Include or exclude a handle and its pending/queued operations from loop liveness.
    pub fn set_ref(&mut self, h: Handle, referenced: bool) -> Result<()> {
        let r = self.resource(h)?;
        let weight = r.pending + usize::from(matches!(r.kind, Kind::Socket) || r.closing.is_some());
        if r.referenced != referenced {
            if referenced {
                self.refs += weight;
            } else {
                self.refs -= weight;
            }
            self.handles
                .get_mut(h.key)
                .expect("validated handle")
                .referenced = referenced;
            // Terminal results retain the operation reference through delivery,
            // including after the native operation has left the active table.
            for q in &mut self.queued {
                if q.handle == Some(h) && q.terminal && q.op.is_some() && q.referenced != referenced
                {
                    if referenced {
                        self.refs += 1;
                    } else {
                        self.refs -= 1;
                    }
                    q.referenced = referenced;
                }
            }
        }
        Ok(())
    }
    /// Create a timer and its operation; repeats coalesce missed intervals and must be nonzero.
    pub fn timer(&mut self, at: Instant, repeat: Option<Duration>, token: Token) -> Result<Handle> {
        if repeat.is_some_and(|r| r.is_zero()) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let h = self.new_handle(Kind::Timer { op: None, repeat })?;
        let op = match self.new_op(Some(h), token) {
            Ok(op) => op,
            Err(e) => {
                self.handles.remove(h.key);
                return Err(e);
            }
        };
        self.handles.get_mut(h.key).expect("inserted").kind = Kind::Timer {
            op: Some(op),
            repeat,
        };
        self.timers.insert(h.key, at);
        self.backend.deadline_changed(self.next_deadline());
        Ok(h)
    }
    /// Move an active timer deadline; returns false for closed, expired or foreign handles.
    pub fn timer_reset(&mut self, h: Handle, at: Instant) -> bool {
        let Ok(r) = self.resource(h) else {
            return false;
        };
        if r.closing.is_some() || !matches!(r.kind, Kind::Timer { op: Some(_), .. }) {
            return false;
        }
        self.timers.cancel(h.key);
        self.timers.insert(h.key, at);
        self.backend.deadline_changed(self.next_deadline());
        true
    }
    /// Return the operation identity of an active timer, for cancellation.
    pub fn timer_op(&self, h: Handle) -> Option<OpId> {
        match self.resource(h).ok()?.kind {
            Kind::Timer { op, .. } => op,
            _ => None,
        }
    }
    fn open(&mut self, spec: Open) -> Result<Handle> {
        let h = self.new_handle(Kind::Socket)?;
        if let Err(e) = self.backend.open(h, spec) {
            self.handles.remove(h.key);
            self.refs -= 1;
            return Err(e);
        }
        Ok(h)
    }
    /// Bind and listen for TCP connections; port zero chooses an ephemeral port.
    pub fn tcp_listen(&mut self, addr: SocketAddr, opts: &ListenOpts) -> Result<Handle> {
        self.open(Open::Listener { addr, opts: *opts })
    }
    /// Listen for local IPC streams. Unix socket path removal belongs to the host.
    pub fn pipe_listen(&mut self, name: &PipeName, opts: &ListenOpts) -> Result<Handle> {
        self.open(Open::PipeListener {
            name: name.clone(),
            opts: *opts,
        })
    }
    /// Connect a local stream, completing with Connected or an error. Busy named
    /// pipes wait asynchronously until available or cancelled by closing the handle.
    pub fn pipe_connect(&mut self, name: &PipeName, token: Token) -> Result<Handle> {
        self.pipe_connect_inner(name, None, token)
    }
    /// Connect a local stream by an absolute loop-clock deadline. Expiry cancels
    /// native I/O and reports TimedOut only after its cancellation acknowledgement.
    /// The deadline covers connection completion, not subsequent stream I/O.
    pub fn pipe_connect_until(
        &mut self,
        name: &PipeName,
        deadline: Instant,
        token: Token,
    ) -> Result<Handle> {
        self.pipe_connect_inner(name, Some(deadline), token)
    }
    fn pipe_connect_inner(
        &mut self,
        name: &PipeName,
        deadline: Option<Instant>,
        token: Token,
    ) -> Result<Handle> {
        let h = self.open(Open::Pipe(name.clone()))?;
        match self.submit(h, Operation::Connect, token) {
            Ok(op) => {
                if let Some(at) = deadline {
                    self.connect_deadlines.insert(op.key, at);
                    self.backend.deadline_changed(self.next_deadline());
                }
                Ok(h)
            }
            Err(e) => {
                self.backend.release(h);
                self.handles.remove(h.key);
                self.refs -= 1;
                Err(e)
            }
        }
    }
    fn expire_connects(&mut self, now: Instant) -> Result<()> {
        while let Some((key, at)) = self.connect_deadlines.pop_expired(now) {
            let op = OpId {
                owner: self.owner,
                key,
            };
            if let Err(error) = self.backend.cancel(op) {
                // Preserve both the deadline and original cancellation error;
                // no premature timeout may release in-flight kernel storage.
                self.connect_deadlines.insert(key, at);
                return Err(error);
            }
            let op = self.ops.get_mut(key).expect("cancelled connect");
            op.cancel = true;
            op.timed_out = true;
        }
        Ok(())
    }
    /// Duplicate standard input, output or error, classifying pipe/file/terminal.
    pub fn open_stdio(&mut self, which: Stdio) -> Result<Handle> {
        self.open(Open::Stdio(which))
    }
    /// Pass an independent reference to a socket over local IPC. The source stays
    /// owned by this loop; close or detach it explicitly when migration is desired.
    pub fn send_handle(&mut self, pipe: Handle, h: Handle, token: Token) -> Result<OpId> {
        if self.resource(h)?.closing.is_some() {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        self.submit(pipe, Operation::SendHandle(h), token)
    }
    /// Receive and attach one passed socket. Use a dedicated IPC control stream;
    /// ordinary reads must not consume the handle-transfer framing byte.
    pub fn recv_handle(&mut self, pipe: Handle, token: Token) -> Result<OpId> {
        self.submit(pipe, Operation::RecvHandle, token)
    }
    /// Spawn a child and submit its exactly-once exit operation. Closing a live
    /// child initiates termination, then waits through ordinary turns for reaping
    /// before Cancelled and Closed; a new process group enables `kill_group`.
    pub fn spawn(&mut self, spec: &ProcessSpec, token: Token) -> Result<Process> {
        let h = self.new_handle(Kind::Socket)?;
        let mut pipes = [None; 3];
        let result = (|| {
            for (i, stdio) in spec.stdio.iter().enumerate() {
                if let ProcessStdio::Handle(source) = stdio {
                    self.resource(*source)?;
                }
                if *stdio == ProcessStdio::Pipe {
                    pipes[i] = Some(self.new_handle(Kind::Socket)?);
                }
            }
            // Reserve the terminal completion before creating an OS child.
            let op = self.new_op(Some(h), token)?;
            let result = self.backend.spawn(h, pipes, spec).and_then(|pid| {
                self.backend.submit(Request {
                    op,
                    handle: h,
                    operation: Operation::ProcessExit,
                })?;
                Ok(pid)
            });
            if result.is_err() {
                self.retire(op);
                self.outstanding -= 1;
            }
            result
        })();
        match result {
            Ok(pid) => Ok(Process {
                handle: h,
                pid,
                stdin: pipes[0],
                stdout: pipes[1],
                stderr: pipes[2],
            }),
            Err(e) => {
                for handle in std::iter::once(h).chain(pipes.into_iter().flatten()) {
                    self.backend.release(handle);
                    if self.handles.remove(handle.key).is_some() {
                        self.refs -= 1;
                    }
                }
                Err(e)
            }
        }
    }
    /// Signal a child still owned by this loop.
    pub fn kill(&mut self, process: Handle, signal: Signal) -> Result<()> {
        self.resource(process)?;
        self.backend.kill(process, signal, false)
    }
    /// Signal an isolated process group, including grandchildren.
    pub fn kill_group(&mut self, process: Handle, signal: Signal) -> Result<()> {
        self.resource(process)?;
        self.backend.kill(process, signal, true)
    }
    /// Subscribe this loop to a process-wide signal. Repeated deliveries may
    /// coalesce; every subscribed loop gets its own completion.
    pub fn signal_start(&mut self, signal: Signal, token: Token) -> Result<Handle> {
        let h = self.new_handle(Kind::Socket)?;
        if let Err(e) = self
            .backend
            .signal(h, signal)
            .and_then(|()| self.submit(h, Operation::WatchSignal, token).map(|_| ()))
        {
            self.backend.release(h);
            self.handles.remove(h.key);
            self.refs -= 1;
            return Err(e);
        }
        Ok(h)
    }
    /// Stop a signal subscription, delivering Stopped before the final Closed.
    pub fn signal_stop(&mut self, h: Handle, token: Token) -> Result<()> {
        let mut next = self.resource(h)?.head;
        while let Some(op) = next {
            next = self.ops.get(op.key).and_then(|op| op.next);
            self.stop(op);
        }
        self.close(h, token)
    }
    /// Set terminal mode; its original settings are restored on close or drop.
    pub fn tty_set_mode(&mut self, h: Handle, mode: TtyMode) -> Result<()> {
        self.resource(h)?;
        self.backend.tty_set_mode(h, mode)
    }
    /// Query terminal rows and columns.
    pub fn tty_window_size(&self, h: Handle) -> Result<WindowSize> {
        self.resource(h)?;
        self.backend.tty_window_size(h)
    }
    /// Subscribe to resize notifications after validating the terminal. On a
    /// Signal(WinCh) completion, query `tty_window_size` for its current size.
    pub fn tty_resize_start(&mut self, h: Handle, token: Token) -> Result<Handle> {
        self.tty_window_size(h)?;
        self.signal_start(Signal::WinCh, token)
    }
    /// Bind a UDP socket; port zero chooses an ephemeral port.
    pub fn udp_bind(&mut self, addr: SocketAddr, opts: &UdpOpts) -> Result<Handle> {
        self.open(Open::Udp { addr, opts: *opts })
    }
    /// Create a TCP socket and submit its connection operation with the supplied token.
    pub fn tcp_connect(
        &mut self,
        addr: SocketAddr,
        opts: &TcpOpts,
        token: Token,
    ) -> Result<Handle> {
        let h = self.open(Open::Tcp { addr, opts: *opts })?;
        if let Err(e) = self.submit(h, Operation::Connect, token) {
            self.backend.release(h);
            self.handles.remove(h.key);
            self.refs -= 1;
            return Err(e);
        }
        Ok(h)
    }
    /// Return the local IP endpoint of a bound socket.
    pub fn local_addr(&self, h: Handle) -> Result<SocketAddr> {
        self.resource(h)?;
        self.backend.local_addr(h)
    }
    fn submit(&mut self, h: Handle, operation: Operation, token: Token) -> Result<OpId> {
        let r = self.resource(h)?;
        if r.closing.is_some() || !matches!(r.kind, Kind::Socket) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let op = self.new_op(Some(h), token)?;
        if let Err(e) = self.backend.submit(Request {
            op,
            handle: h,
            operation,
        }) {
            self.retire(op);
            self.outstanding -= 1;
            return Err(e);
        }
        Ok(op)
    }
    /// Accept one incoming TCP or local connection.
    pub fn accept(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Accept { multishot: false }, token)
    }
    /// Continuously accept connections until stopped or cancelled.
    pub fn accept_start(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Accept { multishot: true }, token)
    }
    /// Read once into stable provided memory or a pooled buffer.
    pub fn read(&mut self, h: Handle, buf: ReadBuf, token: Token) -> Result<OpId> {
        self.submit(
            h,
            Operation::Read {
                buf,
                multishot: false,
            },
            token,
        )
    }
    /// Read repeatedly into pooled leases until EOF, stop, cancellation or error.
    pub fn read_start(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(
            h,
            Operation::Read {
                buf: ReadBuf::Pooled,
                multishot: true,
            },
            token,
        )
    }
    /// Write the complete buffer; internal partial writes preserve operation ordering.
    pub fn write(&mut self, h: Handle, buf: WriteBuf, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Write(buf), token)
    }
    /// Write all inline segments in order without allocating descriptor storage.
    pub fn writev(&mut self, h: Handle, bufs: WriteVectored, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Writev(bufs), token)
    }
    /// Send the buffer as one UDP datagram to the specified endpoint.
    pub fn send_to(
        &mut self,
        h: Handle,
        buf: WriteBuf,
        to: SocketAddr,
        token: Token,
    ) -> Result<OpId> {
        self.submit(h, Operation::SendTo { buf, to }, token)
    }
    /// Receive one UDP datagram into provided or pooled storage.
    pub fn recv(&mut self, h: Handle, buf: ReadBuf, token: Token) -> Result<OpId> {
        self.submit(h, Operation::RecvFrom(buf), token)
    }
    /// Queue stream write-side shutdown after earlier writes.
    pub fn shutdown(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Shutdown, token)
    }
    /// Request cancellation; terminal acknowledgement must precede memory reuse.
    pub fn cancel(&mut self, id: OpId) -> bool {
        self.cancel_inner(id, false)
    }
    /// Stop a multishot operation with a final Stopped acknowledgement.
    pub fn stop(&mut self, id: OpId) -> bool {
        self.cancel_inner(id, true)
    }
    fn cancel_inner(&mut self, id: OpId, stop: bool) -> bool {
        if id.owner != self.owner {
            return false;
        }
        let Some(op) = self.ops.get(id.key).cloned() else {
            return false;
        };
        if op.cancel {
            return false;
        }
        let timer = op.handle.is_some_and(|h| {
            self.handles
                .get(h.key)
                .is_some_and(|r| matches!(r.kind, Kind::Timer { .. }))
        });
        if timer {
            self.timers.cancel(op.handle.expect("timer handle").key);
            self.backend.deadline_changed(self.next_deadline());
            self.finish(
                id,
                if stop {
                    OpResult::Stopped
                } else {
                    OpResult::Cancelled
                },
                true,
            );
        } else {
            if op.external_wait {
                crate::external_wait::cancel(id);
                #[cfg(target_arch = "wasm32")]
                self.backend.deadline_changed(self.next_deadline());
            } else if let Some(cancel) = op.job_cancel {
                cancel.store(true, Ordering::Release);
            } else if self.backend.cancel(id).is_err() {
                return false;
            }
            let op = self.ops.get_mut(id.key).expect("active op");
            op.cancel = true;
            op.stop = stop;
        }
        if self.connect_deadlines.cancel(id.key) {
            self.backend.deadline_changed(self.next_deadline());
        }
        true
    }
    /// Whether close has begun and physical release is still pending.
    pub fn is_closing(&self, h: Handle) -> bool {
        self.resource(h).is_ok_and(|r| r.closing.is_some())
    }
    /// Cancel pending operations, then deliver Closed. The resource is released
    /// only after Closed is appended to the host's output buffer.
    pub fn close(&mut self, h: Handle, token: Token) -> Result<()> {
        let r = self.resource(h)?;
        if r.closing.is_some() {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        self.backend.prepare_close(h)?;
        let r = self.handles.get_mut(h.key).expect("validated");
        r.closing = Some(token);
        // Socket handles already carry a reference. Inactive timer handles do
        // not, so closing adds one through delivery of the final Closed result.
        if matches!(r.kind, Kind::Timer { .. }) && r.referenced {
            self.refs += 1;
        }
        let mut next = self.resource(h)?.head;
        while let Some(id) = next {
            next = self.ops.get(id.key).and_then(|op| op.next);
            self.cancel(id);
        }
        self.maybe_closed(h);
        Ok(())
    }
    /// Cancel pending operations and detach once acknowledgements are delivered; WouldBlock means turn and retry.
    pub fn detach(&mut self, h: Handle) -> Result<B::Detached> {
        let r = self.resource(h)?;
        if r.closing.is_some() || !matches!(r.kind, Kind::Socket) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let mut next = self.resource(h)?.head;
        while let Some(id) = next {
            next = self.ops.get(id.key).and_then(|op| op.next);
            self.cancel(id);
        }
        if self.resource(h)?.pending != 0 || self.queued.iter().any(|c| c.handle == Some(h)) {
            return Err(Error::new(ErrorKind::WouldBlock));
        }
        let d = self.backend.detach(h)?;
        let r = self.handles.remove(h.key).expect("validated");
        if r.referenced {
            self.refs -= 1;
        }
        Ok(d)
    }
    /// Register an owning transport on this loop; failure drops the rejected transport.
    pub fn attach(&mut self, d: B::Detached, _token: Token) -> Result<Handle> {
        let h = self.new_handle(Kind::Socket)?;
        if let Err(e) = self.backend.attach(h, d) {
            self.handles.remove(h.key);
            self.refs -= 1;
            return Err(e);
        }
        Ok(h)
    }
    /// Register a host condition with the native helper or single-agent service. Registrations use
    /// preallocated storage; cancellation and completion follow normal OpId rules.
    pub fn external_wait(
        &mut self,
        condition: &WaitCondition,
        expected: u64,
        deadline: Option<Instant>,
        token: Token,
    ) -> Result<OpId> {
        let op = self.new_op(None, token)?;
        self.ops.get_mut(op.key).expect("new wait").external_wait = true;
        if let Err(e) =
            crate::external_wait::submit(op, self.work_port.clone(), condition, expected, deadline)
        {
            self.retire(op);
            self.outstanding -= 1;
            return Err(e);
        }
        #[cfg(target_arch = "wasm32")]
        self.backend.deadline_changed(self.next_deadline());
        Ok(op)
    }
    /// Submit an owned Send closure to the bounded shared blocking pool.
    pub fn blocking<F: FnOnce() -> BlockingResult + Send + 'static>(
        &mut self,
        f: F,
        token: Token,
    ) -> Result<OpId> {
        self.submit_work(crate::blocking::blocking(f), token)
    }
    /// Resolve through a host resolver, or the shared native blocking pool.
    pub fn resolve(&mut self, request: crate::DnsRequest, token: Token) -> Result<OpId> {
        let op = self.new_op(None, token)?;
        match self.backend.resolve(op, &request) {
            Ok(()) => Ok(op),
            Err(e) => {
                self.retire(op);
                self.outstanding -= 1;
                if e.kind == ErrorKind::Unsupported {
                    self.submit_work(crate::blocking::resolve(request), token)
                } else {
                    Err(e)
                }
            }
        }
    }
    fn submit_work(
        &mut self,
        f: Box<dyn FnOnce() -> Result<crate::blocking::WorkOutput> + Send>,
        token: Token,
    ) -> Result<OpId> {
        let op = self.new_op(None, token)?;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.ops.get_mut(op.key).expect("new op").job_cancel = Some(cancel.clone());
        if let Err(e) = crate::blocking::submit(
            self.config.blocking_pool,
            op,
            cancel,
            self.work_port.clone(),
            f,
        ) {
            self.retire(op);
            self.outstanding -= 1;
            return Err(e);
        }
        Ok(op)
    }
    /// Clone the thread-safe wake endpoint for this loop.
    pub fn notifier(&self) -> Notifier {
        self.notifier.clone()
    }
    /// Clone the bounded posting endpoint routed to this loop only.
    pub fn poster(&self) -> Poster {
        self.poster.clone()
    }
    /// Opt into external host waiting and return the borrowed integration primitive.
    pub fn integration(&mut self) -> Result<Integration> {
        let integration = self.backend.integration()?;
        self.external = true;
        self.notifier.external_park(
            !self.queued.is_empty()
                || !self.poster.is_empty()
                || !self.work_port.is_empty()
                || self.backend.has_work(),
        )?;
        Ok(integration)
    }
    fn drain(&mut self, out: &mut Completions) {
        while out.len() < out.capacity() {
            let Some(Queued {
                completion: c,
                referenced,
                event_class,
            }) = self.queued.pop_front()
            else {
                break;
            };
            let closed = matches!(c.result, OpResult::Closed)
                .then_some(c.handle)
                .flatten();
            self.refs -= usize::from(referenced);
            if let Some(class) = event_class {
                self.buffered[class] -= 1;
            }
            if c.terminal && c.op.is_some() {
                self.outstanding -= 1;
            }
            out.entries.push(c);
            if let Some(h) = closed
                && let Some(r) = self.handles.remove(h.key)
            {
                if matches!(r.kind, Kind::Socket) {
                    self.backend.release(h);
                }
                if r.referenced {
                    self.refs -= 1;
                }
            }
        }
    }
    fn accept_event(&mut self, e: Event<B::Detached>) {
        if e.op.owner != self.owner {
            return;
        }
        let Some(op) = self.ops.get(e.op.key).cloned() else {
            return;
        };
        let result = if op.cancel {
            if !e.terminal {
                return;
            }
            if op.timed_out {
                OpResult::Err(Error::new(ErrorKind::TimedOut))
            } else if op.stop {
                OpResult::Stopped
            } else {
                OpResult::Cancelled
            }
        } else {
            match e.result {
                Ok(Outcome::Resolved(addresses)) => OpResult::Resolved(addresses),
                Ok(Outcome::Exited(status)) => OpResult::Exited(status),
                Ok(Outcome::Signal(signal)) => OpResult::Signal(signal),
                Ok(Outcome::PipeAccepted(d)) => match self.attach(d, op.token) {
                    Ok(conn) => OpResult::PipeAccepted { conn },
                    Err(e) => OpResult::Err(e),
                },
                Ok(Outcome::HandleReceived(d)) => match self.attach(d, op.token) {
                    Ok(handle) => OpResult::HandleReceived { handle },
                    Err(e) => OpResult::Err(e),
                },
                Ok(Outcome::HandleSent) => OpResult::HandleSent,
                Err(e) => OpResult::Err(e),
                Ok(Outcome::Connected) => OpResult::Connected,
                Ok(Outcome::Accepted { transport, peer }) => match self.attach(transport, op.token)
                {
                    Ok(conn) => OpResult::Accepted { conn, peer },
                    Err(e) => OpResult::Err(e),
                },
                Ok(Outcome::Read { n, lease }) => OpResult::Read { n, lease },
                Ok(Outcome::Eof) => OpResult::Eof,
                Ok(Outcome::Wrote(n)) => OpResult::Wrote(n),
                Ok(Outcome::RecvFrom { n, from, lease }) => OpResult::RecvFrom { n, from, lease },
                Ok(Outcome::Shutdown) => OpResult::Shutdown,
                Ok(Outcome::Cancelled) => OpResult::Cancelled,
            }
        };
        self.finish(e.op, result, e.terminal);
    }
    /// Collect bounded completions with at most one OS wait, never invoking host callbacks.
    pub fn turn(&mut self, timeout: Timeout, out: &mut Completions) -> Result<TurnInfo> {
        self.assert_owner();
        self.backend.validate_timeout(timeout)?;
        out.clear();
        let notified = self.notifier.begin();
        let start = self.backend.now();
        self.expire_connects(start)?;
        #[cfg(target_arch = "wasm32")]
        crate::external_wait::poll(self.owner, start);
        let deadline = match (timeout.deadline(start), self.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let queued =
            !self.queued.is_empty() || !self.poster.is_empty() || !self.work_port.is_empty();
        let mut waits = 0;
        let mut discovery_polls = 0;
        let mut zero_event_waits = 0;
        if self.buffered[NATIVE_EVENTS] == 0 && (!queued || self.native_pending != 0) {
            let mut timeout = deadline.map(|d| d.saturating_duration_since(start));
            if timeout == Some(Duration::ZERO) || queued || notified || !self.notifier.park() {
                timeout = Some(Duration::ZERO);
            }
            let poll = self.backend.poll(timeout, &mut self.events);
            self.notifier.running();
            let poll = poll?;
            waits = poll.waits;
            discovery_polls = poll.discovery_polls;
            zero_event_waits = poll.zero_event_waits;
            // Taking/replacing the preallocated vector preserves storage and allows
            // completion handling to mutate the backend when accepting a socket.
            let mut events = std::mem::take(&mut self.events);
            for e in events.drain(..) {
                self.accept_event(e);
            }
            self.events = events;
        }
        let now = self.backend.now();
        self.expire_connects(now)?;
        #[cfg(target_arch = "wasm32")]
        crate::external_wait::poll(self.owner, now);
        for _ in 0..self.config.events_per_turn {
            if self.buffered[TIMER_EVENTS] == self.config.events_per_turn {
                break;
            }
            let Some((key, _at)) = self.timers.pop_expired(now) else {
                break;
            };
            let h = Handle {
                owner: self.owner,
                key,
            };
            let Some(r) = self.handles.get(key) else {
                continue;
            };
            if let Kind::Timer {
                op: Some(op),
                repeat,
            } = r.kind
            {
                if let Some(repeat) = repeat {
                    if let Some(at) = now.checked_add(repeat) {
                        self.timers.insert(h.key, at);
                        self.finish(op, OpResult::Timer, false);
                    } else {
                        self.finish(op, OpResult::Err(Error::new(ErrorKind::InvalidInput)), true);
                    }
                } else {
                    self.finish(op, OpResult::Timer, true);
                }
            }
        }
        self.backend.deadline_changed(self.next_deadline());
        for _ in 0..self.config.events_per_turn {
            let Some(work) = self.work_port.pop() else {
                break;
            };
            let Some(op) = self.ops.get(work.op.key) else {
                continue;
            };
            let result = if op.cancel {
                if op.stop {
                    OpResult::Stopped
                } else {
                    OpResult::Cancelled
                }
            } else {
                match work.result {
                    Ok(crate::blocking::WorkOutput::ExternalWait(r)) => OpResult::ExternalWait(r),
                    Ok(crate::blocking::WorkOutput::Blocking(p)) => OpResult::Blocking(p),
                    Ok(crate::blocking::WorkOutput::Resolved(a)) => OpResult::Resolved(a),
                    Err(e) => OpResult::Err(e),
                }
            };
            self.finish(work.op, result, true);
        }
        // The independent post reserve guarantees progress through a persistent
        // timer/I/O backlog without consuming terminal operation/Closed credits.
        for _ in 0..self.config.events_per_turn {
            if self.buffered[POST_EVENTS] == self.config.events_per_turn {
                break;
            }
            let Some(p) = self.poster.pop() else {
                break;
            };
            self.enqueue(
                Completion {
                    token: p.token,
                    op: None,
                    handle: None,
                    terminal: true,
                    result: OpResult::Posted(p.payload),
                },
                false,
            );
        }
        self.drain(out);
        if self.external {
            self.notifier.external_park(
                !self.queued.is_empty()
                    || !self.poster.is_empty()
                    || !self.work_port.is_empty()
                    || self.backend.has_work(),
            )?;
        }
        Ok(TurnInfo {
            completions: out.len(),
            waited: self.backend.now().saturating_duration_since(start),
            alive: self.alive(),
            os_waits: waits,
            discovery_polls,
            zero_event_waits,
        })
    }
}

impl<B: Backend> Drop for Driver<B> {
    fn drop(&mut self) {
        crate::external_wait::close(self.owner);
        self.work_port.close();
        self.poster.close();
        self.notifier.close();
    }
}

#[cfg(all(test, not(loom), not(turnloop_backend = "web")))]
mod clock_contract {
    use super::*;
    use crate::backend::{PollInfo, Wake};
    use std::sync::Arc;
    struct NoWake;
    impl Wake for NoWake {
        fn wake(&self) -> Result<()> {
            panic!("nonblocking test must not wake")
        }
        fn syscall_count(&self) -> u64 {
            0
        }
    }
    struct Host {
        now: Instant,
        deadline: Option<Instant>,
        changes: usize,
        polls: usize,
        cached_work: bool,
        accept_connect: bool,
        pending: Option<Request>,
        cancellation: Option<OpId>,
        cancel_error: Option<Error>,
        cancel_calls: usize,
        acknowledge: bool,
    }
    // SAFETY: this test backend accepts only buffer-free synthetic connects;
    // it performs no native I/O and owns no user buffers.
    unsafe impl Backend for Host {
        type Wake = NoWake;
        type Detached = ();
        fn new(_: &Config, _: BufferPool) -> Result<Self> {
            Ok(Self {
                now: Instant::now(),
                deadline: None,
                changes: 0,
                polls: 0,
                cached_work: false,
                accept_connect: false,
                pending: None,
                cancellation: None,
                cancel_error: None,
                cancel_calls: 0,
                acknowledge: false,
            })
        }
        fn now(&self) -> Instant {
            self.now
        }
        fn waker(&self) -> Arc<NoWake> {
            Arc::new(NoWake)
        }
        fn validate_timeout(&self, timeout: Timeout) -> Result<()> {
            if matches!(timeout, Timeout::Now) {
                Ok(())
            } else {
                Err(Error::new(ErrorKind::Unsupported))
            }
        }
        fn deadline_changed(&mut self, deadline: Option<Instant>) {
            self.deadline = deadline;
            self.changes += 1;
        }
        fn open(&mut self, _: Handle, spec: Open) -> Result<()> {
            if self.accept_connect && matches!(spec, Open::Pipe(_)) {
                Ok(())
            } else {
                Err(Error::new(ErrorKind::Unsupported))
            }
        }
        fn local_addr(&self, _: Handle) -> Result<SocketAddr> {
            Err(Error::new(ErrorKind::Unsupported))
        }
        fn submit(&mut self, request: Request) -> Result<()> {
            if self.accept_connect && matches!(request.operation, Operation::Connect) {
                assert!(self.pending.replace(request).is_none());
                Ok(())
            } else {
                Err(Error::new(ErrorKind::Unsupported))
            }
        }
        fn cancel(&mut self, op: OpId) -> Result<()> {
            self.cancel_calls += 1;
            if let Some(error) = self.cancel_error {
                return Err(error);
            }
            if self
                .pending
                .as_ref()
                .is_some_and(|request| request.op == op)
            {
                self.cancellation = Some(op);
                Ok(())
            } else {
                Err(Error::new(ErrorKind::NotFound))
            }
        }
        fn has_work(&self) -> bool {
            self.cached_work
        }
        fn poll(
            &mut self,
            timeout: Option<Duration>,
            events: &mut Vec<Event<()>>,
        ) -> Result<PollInfo> {
            assert_eq!(timeout, Some(Duration::ZERO));
            self.polls += 1;
            if self.acknowledge
                && let Some(op) = self.cancellation.take()
            {
                assert_eq!(self.pending.take().expect("pending connect").op, op);
                events.push(Event {
                    op,
                    terminal: true,
                    result: Ok(Outcome::Cancelled),
                });
            }
            Ok(PollInfo::default())
        }
        fn release(&mut self, _: Handle) {}
        fn detach(&mut self, _: Handle) -> Result<()> {
            Err(Error::new(ErrorKind::Unsupported))
        }
        fn attach(&mut self, _: Handle, _: ()) -> Result<()> {
            Err(Error::new(ErrorKind::Unsupported))
        }
        fn integration(&mut self) -> Result<Integration> {
            Ok(Integration::HostCallback)
        }
    }
    #[test]
    fn queued_core_work_skips_backend_even_with_cached_work() {
        let mut driver = Driver::<Host>::new(Config::default()).expect("host loop");
        driver.backend.cached_work = true;
        let mut out = Completions::with_capacity(1);
        let mut delivered = 0;
        for _ in 0..64 {
            driver
                .poster()
                .post(Token(1), Payload::U64(42))
                .expect("post");
            let info = driver.turn(Timeout::Now, &mut out).expect("queued turn");
            assert_eq!(info.completions, 1);
            assert_eq!(out[0].token, Token(1));
            assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(42))));
            assert_eq!((info.os_waits, info.discovery_polls), (0, 0));
            delivered += 1;
        }
        assert_eq!(delivered, 64);
        assert_eq!(driver.backend.polls, 0, "backend must not be called");
    }

    #[test]
    fn connection_deadline_waits_for_acknowledgement_and_retains_cancellation_errors() {
        let mut driver = Driver::<Host>::new(Config::default()).expect("host loop");
        driver.backend.accept_connect = true;
        let at = driver.now() + Duration::from_millis(10);
        let h = driver
            .pipe_connect_until(&PipeName("synthetic".into()), at, Token(1))
            .expect("connect");
        let op = driver
            .backend
            .pending
            .as_ref()
            .expect("submitted connect")
            .op;
        let mut out = Completions::with_capacity(1);
        driver
            .turn(Timeout::Now, &mut out)
            .expect("before deadline");
        assert!(out.is_empty());
        assert_eq!(driver.backend.cancel_calls, 0);
        assert_eq!(driver.next_deadline(), Some(at));
        let failure = Error {
            kind: ErrorKind::Other,
            os: Some(12345),
        };
        driver.backend.cancel_error = Some(failure);
        driver.backend.now = at;
        assert_eq!(
            driver
                .turn(Timeout::Now, &mut out)
                .expect_err("injected cancellation error"),
            failure
        );
        assert_eq!(driver.backend.cancel_calls, 1);
        assert_eq!(driver.next_deadline(), Some(at));
        assert!(out.is_empty() && driver.backend.pending.is_some());
        driver.backend.cancel_error = None;
        driver
            .turn(Timeout::Now, &mut out)
            .expect("retry cancellation");
        assert_eq!(driver.backend.cancel_calls, 2);
        assert_eq!(driver.next_deadline(), None);
        assert!(
            out.is_empty() && driver.backend.pending.is_some(),
            "timeout cannot retire before native acknowledgement"
        );
        driver.backend.acknowledge = true;
        driver
            .turn(Timeout::Now, &mut out)
            .expect("acknowledge timeout");
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].handle, out[0].op, out[0].token, out[0].terminal),
            (Some(h), Some(op), Token(1), true)
        );
        assert!(matches!(
            out[0].result,
            OpResult::Err(Error {
                kind: ErrorKind::TimedOut,
                ..
            })
        ));
        assert!(driver.backend.pending.is_none());
        assert_eq!(driver.backend.cancel_calls, 2);
        driver.close(h, Token(2)).expect("close timed-out stream");
        driver.turn(Timeout::Now, &mut out).expect("closed");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Closed));
        driver
            .turn(Timeout::Now, &mut out)
            .expect("no duplicate timeout");
        assert!(out.is_empty() && !driver.alive());
    }
    #[test]
    fn optional_native_capabilities_reject_without_leaking_core_reservations() {
        let mut l = Driver::<Host>::new(Config::default()).expect("host loop");
        let mut spec = ProcessSpec::new("unused-native-program");
        spec.stdio = [ProcessStdio::Pipe; 3];
        assert_eq!(
            l.spawn(&spec, Token(1))
                .expect_err("unsupported spawn")
                .kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.signal_start(Signal::Int, Token(2))
                .expect_err("unsupported signal")
                .kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.open_stdio(Stdio::Stdin)
                .expect_err("unsupported stdio")
                .kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.pipe_listen(&PipeName("unused".into()), &ListenOpts::default())
                .expect_err("unsupported pipe")
                .kind,
            ErrorKind::Unsupported
        );
        assert!(
            !l.alive(),
            "failed native setup must roll back every reservation"
        );
        let h = l
            .timer(l.now(), None, Token(3))
            .expect("core timer still works");
        assert_eq!(
            l.kill(h, Signal::Kill).expect_err("unsupported kill").kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.tty_set_mode(h, TtyMode::Raw)
                .expect_err("unsupported TTY")
                .kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.tty_window_size(h)
                .expect_err("unsupported dimensions")
                .kind,
            ErrorKind::Unsupported
        );
        let mut out = Completions::default();
        l.turn(Timeout::Now, &mut out).expect("core turn");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Timer));
        l.close(h, Token(4)).expect("timer close");
        l.turn(Timeout::Now, &mut out).expect("closed turn");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Closed));
        assert!(!l.alive());
    }
    #[test]
    fn host_clock_arms_deadline_and_validates_queued_turns() {
        let mut l = Driver::<Host>::new(Config::default()).expect("host loop");
        let at = l.now() + Duration::from_micros(50);
        let h = l.timer(at, None, Token(1)).expect("timer");
        assert_eq!(l.backend.deadline, Some(at));
        assert!(l.backend.changes > 0);
        let mut out = Completions::default();
        l.turn(Timeout::Now, &mut out).expect("early turn");
        assert!(out.is_empty());
        assert_eq!(l.backend.polls, 1);
        l.backend.now = at;
        l.turn(Timeout::Now, &mut out).expect("deadline turn");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Timer));
        assert_eq!(l.backend.deadline, None);
        l.close(h, Token(2)).expect("close");
        assert!(
            matches!(
                l.turn(Timeout::Forever, &mut out),
                Err(Error {
                    kind: ErrorKind::Unsupported,
                    ..
                })
            ),
            "queued completions do not bypass timeout validation"
        );
        l.turn(Timeout::Now, &mut out).expect("queued close");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Closed));
        let h = l
            .timer(at + Duration::from_secs(1), None, Token(3))
            .expect("timer");
        assert!(l.cancel(l.timer_op(h).expect("op")));
        assert_eq!(l.backend.deadline, None);
    }
}

#[cfg(turnloop_backend = "web")]
impl Driver<crate::backend::web::Web> {
    /// Configure the host dispatcher. It is scheduled asynchronously and should
    /// call turn(Now); no user callback runs inside a turnloop method.
    pub fn set_schedule_turn(&mut self, schedule: &js_sys::Function) -> Result<()> {
        self.backend.configure(schedule)?;
        self.integration()?;
        Ok(())
    }
    /// Number of coalesced host turn requests.
    pub fn schedule_count(&self) -> u32 {
        self.backend.schedule_count()
    }
    /// Attach a bounded Worker Poster. Returns a JS descriptor with `buffer`
    /// (SharedArrayBuffer), `capacity`, and `producerSource` (SharedPoster class).
    /// Construct that class in a Worker with the transferred descriptor; post
    /// returns false on contention/full/closed, retaining the caller's values.
    /// Browsers require COOP/COEP isolation and Atomics.waitAsync; see docs/wasm.md.
    #[cfg(feature = "web-worker")]
    pub fn worker_poster(&mut self, capacity: u32) -> Result<wasm_bindgen::JsValue> {
        self.backend.worker_poster(capacity, self.poster())
    }
    /// Expose a condition to Workers through a bounded Atomics-backed queue.
    /// The descriptor's producerSource class offers store(u64) and notify(). Both
    /// return false on contention/full/closed; retry without losing the update.
    /// Accepted updates apply on the owning agent in queue order. A descriptor
    /// lives until this driver drops; the condition can notify waits on any local loop.
    #[cfg(feature = "web-worker")]
    pub fn worker_wait_condition(
        &mut self,
        condition: &WaitCondition,
        capacity: u32,
    ) -> Result<wasm_bindgen::JsValue> {
        self.backend.worker_condition(condition.clone(), capacity)
    }
    /// Fetch one complete response into a provided or pooled buffer. Responses
    /// larger than that buffer complete with ResourceLimit, never truncated data.
    pub fn fetch(&mut self, url: &str, buf: ReadBuf, token: Token) -> Result<(Handle, OpId)> {
        let h = self.open(Open::Fetch { url: url.into() })?;
        match self.read(h, buf, token) {
            Ok(op) => Ok((h, op)),
            Err(e) => {
                self.backend.release(h);
                self.handles.remove(h.key);
                self.refs -= 1;
                Err(e)
            }
        }
    }
    /// Connect a browser WebSocket; subsequent read/write operations exchange
    /// whole binary messages. Oversize messages fail with ResourceLimit.
    pub fn websocket(&mut self, url: &str, token: Token) -> Result<Handle> {
        let h = self.open(Open::WebSocket { url: url.into() })?;
        if let Err(e) = self.submit(h, Operation::Connect, token) {
            self.backend.release(h);
            self.handles.remove(h.key);
            self.refs -= 1;
            return Err(e);
        }
        Ok(h)
    }
}
