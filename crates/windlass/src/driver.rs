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
    stop: bool,
    job_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    previous: Option<OpId>,
    next: Option<OpId>,
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
    queued: VecDeque<Completion>,
    events: Vec<Event<B::Detached>>,
    refs: usize,
    outstanding: usize,
    config: Config,
    _local: PhantomData<Rc<()>>,
}
impl<B: Backend> Driver<B> {
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
        let backend = B::new(
            &config,
            BufferPool::new(config.pooled_buffers, config.pooled_buffer_size),
        )?;
        let notifier = Notifier::new(backend.waker());
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
            queued: VecDeque::with_capacity(
                config.max_operations + config.max_handles + config.events_per_turn,
            ),
            events: Vec::with_capacity(config.events_per_turn),
            refs: 0,
            outstanding: 0,
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
                stop: false,
                job_cancel: None,
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
            self.queued.push_back(Completion {
                token,
                op: None,
                handle: Some(h),
                terminal: true,
                result: OpResult::Closed,
            });
        }
    }
    fn finish(&mut self, id: OpId, result: OpResult, terminal: bool) {
        let Some(op) = self.ops.get(id.key).cloned() else {
            return;
        };
        self.queued.push_back(Completion {
            token: op.token,
            op: Some(id),
            handle: op.handle,
            terminal,
            result,
        });
        if terminal {
            self.retire(id);
            if let Some(h) = op.handle {
                self.maybe_closed(h);
            }
        }
    }
    pub fn alive(&self) -> bool {
        self.refs != 0
    }
    /// The backend monotonic clock; portable deadline construction starts here.
    pub fn now(&self) -> Instant {
        self.backend.now()
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.timers.next_deadline()
    }
    pub fn set_ref(&mut self, h: Handle, referenced: bool) -> Result<()> {
        let r = self.resource(h)?;
        let weight = r.pending + usize::from(matches!(r.kind, Kind::Socket));
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
        }
        Ok(())
    }
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
        self.backend.deadline_changed(self.timers.next_deadline());
        Ok(h)
    }
    pub fn timer_reset(&mut self, h: Handle, at: Instant) -> bool {
        let Ok(r) = self.resource(h) else {
            return false;
        };
        if r.closing.is_some() || !matches!(r.kind, Kind::Timer { op: Some(_), .. }) {
            return false;
        }
        self.timers.cancel(h.key);
        self.timers.insert(h.key, at);
        self.backend.deadline_changed(self.timers.next_deadline());
        true
    }
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
    pub fn tcp_listen(&mut self, addr: SocketAddr, opts: &ListenOpts) -> Result<Handle> {
        self.open(Open::Listener { addr, opts: *opts })
    }
    pub fn udp_bind(&mut self, addr: SocketAddr, opts: &UdpOpts) -> Result<Handle> {
        self.open(Open::Udp { addr, opts: *opts })
    }
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
    pub fn accept(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Accept { multishot: false }, token)
    }
    pub fn accept_start(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Accept { multishot: true }, token)
    }
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
    pub fn write(&mut self, h: Handle, buf: WriteBuf, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Write(buf), token)
    }
    pub fn writev(&mut self, h: Handle, bufs: WriteVectored, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Writev(bufs), token)
    }
    pub fn send_to(
        &mut self,
        h: Handle,
        buf: WriteBuf,
        to: SocketAddr,
        token: Token,
    ) -> Result<OpId> {
        self.submit(h, Operation::SendTo { buf, to }, token)
    }
    pub fn recv(&mut self, h: Handle, buf: ReadBuf, token: Token) -> Result<OpId> {
        self.submit(h, Operation::RecvFrom(buf), token)
    }
    pub fn shutdown(&mut self, h: Handle, token: Token) -> Result<OpId> {
        self.submit(h, Operation::Shutdown, token)
    }
    pub fn cancel(&mut self, id: OpId) -> bool {
        self.cancel_inner(id, false)
    }
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
            self.backend.deadline_changed(self.timers.next_deadline());
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
            if let Some(cancel) = op.job_cancel {
                cancel.store(true, Ordering::Release);
            } else if self.backend.cancel(id).is_err() {
                return false;
            }
            let op = self.ops.get_mut(id.key).expect("active op");
            op.cancel = true;
            op.stop = stop;
        }
        true
    }
    /// Cancel pending operations, then deliver Closed. The resource is released
    /// only after Closed is appended to the host's output buffer.
    pub fn close(&mut self, h: Handle, token: Token) -> Result<()> {
        let r = self.resource(h)?;
        if r.closing.is_some() {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        self.handles.get_mut(h.key).expect("validated").closing = Some(token);
        let mut next = self.resource(h)?.head;
        while let Some(id) = next {
            next = self.ops.get(id.key).and_then(|op| op.next);
            self.cancel(id);
        }
        self.maybe_closed(h);
        Ok(())
    }
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
    pub fn attach(&mut self, d: B::Detached, _token: Token) -> Result<Handle> {
        let h = self.new_handle(Kind::Socket)?;
        if let Err(e) = self.backend.attach(h, d) {
            self.handles.remove(h.key);
            self.refs -= 1;
            return Err(e);
        }
        Ok(h)
    }
    pub fn blocking<F: FnOnce() -> BlockingResult + Send + 'static>(
        &mut self,
        f: F,
        token: Token,
    ) -> Result<OpId> {
        self.submit_work(crate::blocking::blocking(f), token)
    }
    pub fn resolve(&mut self, request: crate::DnsRequest, token: Token) -> Result<OpId> {
        self.submit_work(crate::blocking::resolve(request), token)
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
    pub fn notifier(&self) -> Notifier {
        self.notifier.clone()
    }
    pub fn poster(&self) -> Poster {
        self.poster.clone()
    }
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
            let Some(c) = self.queued.pop_front() else {
                break;
            };
            let closed = matches!(c.result, OpResult::Closed)
                .then_some(c.handle)
                .flatten();
            if c.terminal && c.op.is_some() {
                self.outstanding -= 1;
            }
            out.entries.push(c);
            if let Some(h) = closed
                && let Some(r) = self.handles.remove(h.key)
                && matches!(r.kind, Kind::Socket)
            {
                self.backend.release(h);
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
            if op.stop {
                OpResult::Stopped
            } else {
                OpResult::Cancelled
            }
        } else {
            match e.result {
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
    pub fn turn(&mut self, timeout: Timeout, out: &mut Completions) -> Result<TurnInfo> {
        self.assert_owner();
        self.backend.validate_timeout(timeout)?;
        out.clear();
        let notified = self.notifier.begin();
        let start = self.backend.now();
        let deadline = match (timeout.deadline(start), self.next_deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let queued =
            !self.queued.is_empty() || !self.poster.is_empty() || !self.work_port.is_empty();
        let mut waits = 0;
        if !queued {
            let mut timeout = deadline.map(|d| d.saturating_duration_since(start));
            if timeout == Some(Duration::ZERO)
                || notified
                || self.backend.has_work()
                || !self.notifier.park()
            {
                timeout = Some(Duration::ZERO);
            }
            let poll = self.backend.poll(timeout, &mut self.events);
            self.notifier.running();
            waits = poll?.waits;
            // Taking/replacing the preallocated vector preserves storage and allows
            // completion handling to mutate the backend when accepting a socket.
            let mut events = std::mem::take(&mut self.events);
            for e in events.drain(..) {
                self.accept_event(e);
            }
            self.events = events;
        }
        let now = self.backend.now();
        for _ in 0..self.config.events_per_turn {
            if self.queued.len() >= self.config.max_operations + self.config.max_handles {
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
        self.backend.deadline_changed(self.timers.next_deadline());
        for _ in 0..self.config.events_per_turn {
            if self.queued.len() >= self.config.max_operations + self.config.max_handles {
                break;
            }
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
                    Ok(crate::blocking::WorkOutput::Blocking(p)) => OpResult::Blocking(p),
                    Ok(crate::blocking::WorkOutput::Resolved(a)) => OpResult::Resolved(a),
                    Err(e) => OpResult::Err(e),
                }
            };
            self.finish(work.op, result, true);
        }
        self.drain(out);
        while out.len() < out.capacity() {
            let Some(p) = self.poster.pop() else {
                break;
            };
            out.entries.push(Completion {
                token: p.token,
                op: None,
                handle: None,
                terminal: true,
                result: OpResult::Posted(p.payload),
            });
        }
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
        })
    }
}

impl<B: Backend> Drop for Driver<B> {
    fn drop(&mut self) {
        self.work_port.close();
        self.poster.close();
        self.notifier.close();
    }
}

#[cfg(all(test, not(loom), not(windlass_backend = "web")))]
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
    }
    // SAFETY: this test backend accepts no native I/O and owns no user buffers.
    unsafe impl Backend for Host {
        type Wake = NoWake;
        type Detached = ();
        fn new(_: &Config, _: BufferPool) -> Result<Self> {
            Ok(Self {
                now: Instant::now(),
                deadline: None,
                changes: 0,
                polls: 0,
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
        fn open(&mut self, _: Handle, _: Open) -> Result<()> {
            Err(Error::new(ErrorKind::Unsupported))
        }
        fn local_addr(&self, _: Handle) -> Result<SocketAddr> {
            Err(Error::new(ErrorKind::Unsupported))
        }
        fn submit(&mut self, _: Request) -> Result<()> {
            Err(Error::new(ErrorKind::Unsupported))
        }
        fn cancel(&mut self, _: OpId) -> Result<()> {
            Err(Error::new(ErrorKind::NotFound))
        }
        fn has_work(&self) -> bool {
            false
        }
        fn poll(&mut self, timeout: Option<Duration>, _: &mut Vec<Event<()>>) -> Result<PollInfo> {
            assert_eq!(timeout, Some(Duration::ZERO));
            self.polls += 1;
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
