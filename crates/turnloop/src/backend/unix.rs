//! Shared completion engine for kqueue and epoll. Each direction has an intrusive
//! FIFO of operations; readiness remains cached until an actual EAGAIN.
#[cfg(turnloop_backend = "epoll")]
use super::epoll::Epoll as SystemPoller;
#[cfg(turnloop_backend = "kqueue")]
use super::kqueue::Kqueue as SystemPoller;
use super::{
    poller::{Poller, Ready, last_error},
    socket::{self, Addr},
};
use crate::{
    backend::{Backend, Event, Operation, Outcome, PollInfo, Request},
    *,
};
use std::{
    collections::VecDeque,
    net::SocketAddr,
    os::fd::{AsRawFd, OwnedFd},
    sync::Arc,
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    Tcp,
    Pipe,
    PipeListener,
    Stream,
    File,
    Listener,
    Udp,
}
/// Owns an unregistered socket and can be sent to another loop/thread.
#[derive(Debug)]
pub struct Detached {
    pub(super) fd: OwnedFd,
    pub(super) kind: Kind,
}
struct Resource {
    handle: Handle,
    transport: Detached,
    connect: Option<Addr>,
    connecting: bool,
    ready: [bool; 2],
    heads: [Option<usize>; 2],
    tails: [Option<usize>; 2],
    queued: bool,
}
struct Pending {
    request: Request,
    next: Option<usize>,
    offset: usize,
    passed: Option<Detached>,
}
pub struct Unix {
    poller: SystemPoller,
    resources: Vec<Option<Resource>>,
    ops: Vec<Option<Pending>>,
    ready: VecDeque<Handle>,
    cancelled: VecDeque<OpId>,
    polled: Vec<Ready>,
    pool: BufferPool,
}
fn direction(op: &Operation) -> usize {
    usize::from(!matches!(
        op,
        Operation::Accept { .. } | Operation::Read { .. } | Operation::RecvFrom(_) | Operation::RecvHandle
    ))
}
impl Unix {
    fn get(&self, h: Handle) -> Result<&Resource> {
        self.resources
            .get(h.index())
            .and_then(Option::as_ref)
            .filter(|r| r.handle == h)
            .ok_or(Error::new(ErrorKind::NotFound))
    }
    fn install(
        &mut self,
        h: Handle,
        transport: Detached,
        connect: Option<Addr>,
    ) -> Result<()> {
        if self.resources.get(h.index()).is_none_or(Option::is_some) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        if transport.kind != Kind::File {
            self.poller.register(transport.fd.as_raw_fd(), h.key())?;
        }
        self.resources[h.index()] = Some(Resource {
            handle: h,
            transport,
            connect,
            connecting: false,
            ready: [true; 2],
            heads: [None; 2],
            tails: [None; 2],
            queued: false,
        });
        Ok(())
    }
    fn schedule(&mut self, h: Handle) {
        let Some(r) = self.resources.get_mut(h.index()).and_then(Option::as_mut) else {
            return;
        };
        if r.handle == h && !r.queued && (0..2).any(|d| r.ready[d] && r.heads[d].is_some()) {
            r.queued = true;
            self.ready.push_back(h);
        }
    }
    fn unlink(&mut self, h: Handle, i: usize, d: usize) {
        let r = self.resources[h.index()]
            .as_mut()
            .expect("registered resource");
        let next = self.ops[i].as_ref().expect("pending op").next;
        if r.heads[d] == Some(i) {
            r.heads[d] = next;
        } else {
            let mut at = r.heads[d];
            while let Some(p) = at {
                let previous = self.ops[p].as_mut().expect("queued op");
                if previous.next == Some(i) {
                    previous.next = next;
                    if r.tails[d] == Some(i) {
                        r.tails[d] = Some(p);
                    }
                    break;
                }
                at = previous.next;
            }
        }
        if r.tails[d] == Some(i) {
            r.tails[d] = None;
        }
    }
    fn run_ready(&mut self, events: &mut Vec<Event<Detached>>) {
        let budget = events.capacity().saturating_sub(events.len());
        for _ in 0..budget {
            if events.len() == events.capacity() {
                break;
            }
            let Some(h) = self.ready.pop_front() else {
                break;
            };
            let Some(r) = self.resources.get_mut(h.index()).and_then(Option::as_mut) else {
                continue;
            };
            if r.handle != h {
                continue;
            }
            r.queued = false;
            for d in 0..2 {
                if events.len() == events.capacity() {
                    break;
                }
                let r = self.resources[h.index()].as_mut().expect("resource");
                if !r.ready[d] {
                    continue;
                }
                let Some(i) = r.heads[d] else {
                    continue;
                };
                let p = self.ops[i].as_mut().expect("queued op");
                let result = execute(r, p, &self.pool);
                let event = match result {
                    Ok(Some((result, terminal))) => Some(Event {
                        op: p.request.op,
                        result: Ok(result),
                        terminal,
                    }),
                    Ok(None) => None,
                    Err(e) if e.kind == ErrorKind::WouldBlock => {
                        r.ready[d] = false;
                        None
                    }
                    Err(e) if e.os == Some(libc::EINTR) => None,
                    Err(e) => Some(Event {
                        op: p.request.op,
                        result: Err(e),
                        terminal: true,
                    }),
                };
                if let Some(e) = event {
                    if e.terminal {
                        self.unlink(h, i, d);
                        self.ops[i] = None;
                    }
                    events.push(e);
                }
            }
            self.schedule(h);
        }
    }
}
// SAFETY: all I/O executes synchronously in poll; a terminal event removes its
// request, and owned descriptors/requests are dropped without outstanding native
// buffer access. Readiness events contain generation keys, never buffer pointers.
unsafe impl Backend for Unix {
    #[cfg(turnloop_backend = "kqueue")]
    type Wake = super::kqueue::KqueueWake;
    #[cfg(turnloop_backend = "epoll")]
    type Wake = super::epoll::EpollWake;
    type Detached = Detached;
    fn new(config: &Config, pool: BufferPool) -> Result<Self> {
        Ok(Self {
            poller: SystemPoller::new(config.events_per_turn)?,
            resources: (0..config.max_handles).map(|_| None).collect(),
            ops: (0..config.max_operations).map(|_| None).collect(),
            ready: VecDeque::with_capacity(config.max_handles),
            cancelled: VecDeque::with_capacity(config.max_operations),
            polled: Vec::with_capacity(config.events_per_turn),
            pool,
        })
    }
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn waker(&self) -> Arc<Self::Wake> {
        self.poller.waker()
    }
    fn open(&mut self, h: Handle, spec: Open) -> Result<()> {
        let spec = match spec {
            Open::Pipe(name) => {
                let (transport, addr) = super::ipc::open(&name, None)?;
                return self.install(h, transport, Some(addr));
            }
            Open::PipeListener { name, opts } => {
                let (transport, _) = super::ipc::open(&name, Some(opts))?;
                return self.install(h, transport, None);
            }
            Open::Stdio(which) => {
                let fd = match which { Stdio::Stdin => 0, Stdio::Stdout => 1, Stdio::Stderr => 2 };
                let transport = super::ipc::stdio(fd)?;
                return self.install(h, transport, None);
            }
            other => other,
        };
        let (addr, kind, reuse, backlog, nodelay) = match spec {
            Open::Tcp { addr, opts } => (addr, Kind::Tcp, false, 0, opts.nodelay),
            Open::Listener { addr, opts } => {
                (addr, Kind::Listener, opts.reuse_port, opts.backlog, false)
            }
            Open::Udp { addr, opts } => (addr, Kind::Udp, opts.reuse_port, 0, false),
            _ => unreachable!("native open handled above"),
        };
        if backlog > i32::MAX as u32 {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let fd = socket::create(addr, kind == Kind::Udp)?;
        if kind != Kind::Tcp {
            socket::option(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEADDR, 1)?;
            if reuse {
                socket::option(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_REUSEPORT, 1)?;
            }
            let a = Addr::new(addr);
            // SAFETY: sockaddr pointer and length refer to initialized storage.
            if unsafe { libc::bind(fd.as_raw_fd(), a.ptr(), a.len) } < 0 {
                return Err(last_error());
            }
            if kind == Kind::Listener {
                // SAFETY: this fd is a bound stream socket and backlog is checked.
                if unsafe { libc::listen(fd.as_raw_fd(), backlog as i32) } < 0 {
                    return Err(last_error());
                }
            }
        }
        if nodelay {
            socket::option(fd.as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_NODELAY, 1)?;
        }
        self.install(
            h,
            Detached { fd, kind },
            (kind == Kind::Tcp).then(|| Addr::new(addr)),
        )
    }
    fn local_addr(&self, h: Handle) -> Result<SocketAddr> {
        socket::local_addr(self.get(h)?.transport.fd.as_raw_fd())
    }
    fn submit(&mut self, request: Request) -> Result<()> {
        let h = request.handle;
        let r = self.get(h)?;
        let valid = match &request.operation {
            Operation::Accept { .. } => matches!(r.transport.kind, Kind::Listener | Kind::PipeListener),
            Operation::SendHandle(_) | Operation::RecvHandle => r.transport.kind == Kind::Pipe,
            Operation::RecvFrom(_) | Operation::SendTo { .. } => r.transport.kind == Kind::Udp,
            Operation::Connect => matches!(r.transport.kind, Kind::Tcp | Kind::Pipe) && r.connect.is_some(),
            _ => matches!(r.transport.kind, Kind::Tcp | Kind::Pipe | Kind::Stream | Kind::File),
        };
        if !valid || self.ops.get(request.op.index()).is_none_or(Option::is_some) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        if matches!(&request.operation, Operation::Read { buf: ReadBuf::Provided(b), .. } if b.is_empty())
        {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let passed = if let Operation::SendHandle(source) = request.operation {
            let source = self.get(source)?;
            if !matches!(source.transport.kind, Kind::Tcp | Kind::Listener | Kind::Udp | Kind::Pipe | Kind::PipeListener) {
                return Err(Error::new(ErrorKind::Unsupported));
            }
            Some(Detached { fd: source.transport.fd.try_clone().map_err(Error::from)?, kind: source.transport.kind })
        } else { None };
        let d = direction(&request.operation);
        let i = request.op.index();
        let r = self.resources[h.index()].as_mut().expect("validated");
        if let Some(tail) = r.tails[d] {
            self.ops[tail].as_mut().expect("tail").next = Some(i);
        } else {
            r.heads[d] = Some(i);
        }
        r.tails[d] = Some(i);
        self.ops[i] = Some(Pending {
            request,
            next: None,
            offset: 0,
            passed,
        });
        self.schedule(h);
        Ok(())
    }
    fn cancel(&mut self, op: OpId) -> Result<()> {
        let p = self
            .ops
            .get(op.index())
            .and_then(Option::as_ref)
            .filter(|p| p.request.op == op)
            .ok_or(Error::new(ErrorKind::NotFound))?;
        let h = p.request.handle;
        let d = direction(&p.request.operation);
        self.unlink(h, op.index(), d);
        self.cancelled.push_back(op);
        self.schedule(h);
        Ok(())
    }
    fn has_work(&self) -> bool {
        !self.ready.is_empty() || !self.cancelled.is_empty()
    }
    fn poll(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<Event<Detached>>,
    ) -> Result<PollInfo> {
        while events.len() < events.capacity() {
            let Some(op) = self.cancelled.pop_front() else {
                break;
            };
            self.ops[op.index()] = None;
            events.push(Event {
                op,
                terminal: true,
                result: Ok(Outcome::Cancelled),
            });
        }
        self.run_ready(events);
        // Cached readiness can end in EAGAIN without producing a completion.
        // In that case use this turn's single OS wait with its exact timeout;
        // returning now would turn an idle socket into a timer polling loop.
        if self.has_work() || !events.is_empty() || events.len() == events.capacity() {
            return Ok(PollInfo::default());
        }
        self.polled.clear();
        let info = self.poller.wait(timeout, &mut self.polled)?;
        for i in 0..self.polled.len() {
            let e = self.polled[i];
            let Some(r) = self
                .resources
                .get_mut(e.key as u32 as usize)
                .and_then(Option::as_mut)
            else {
                continue;
            };
            if r.handle.key() != e.key {
                continue;
            }
            r.ready[0] |= e.read;
            r.ready[1] |= e.write;
            let h = r.handle;
            self.schedule(h);
        }
        self.run_ready(events);
        Ok(info)
    }
    fn release(&mut self, h: Handle) {
        if let Ok(r) = self.get(h) {
            if r.transport.kind != Kind::File {
                let fd = r.transport.fd.as_raw_fd();
                let _ = self.poller.deregister(fd);
            }
            self.ready.retain(|&at| at != h);
            self.resources[h.index()] = None;
        }
        // Closing the final descriptor removes its registration from epoll/kqueue.
    }
    fn detach(&mut self, h: Handle) -> Result<Detached> {
        let r = self.get(h)?;
        if r.heads.iter().any(Option::is_some) {
            return Err(Error::new(ErrorKind::WouldBlock));
        }
        if r.transport.kind != Kind::File {
            self.poller.deregister(r.transport.fd.as_raw_fd())?;
        }
        // Remove a stale scheduling entry before the slot can be reused.
        self.ready.retain(|&at| at != h);
        Ok(self.resources[h.index()]
            .take()
            .expect("validated")
            .transport)
    }
    fn attach(&mut self, h: Handle, transport: Detached) -> Result<()> {
        self.install(h, transport, None)
    }
    fn integration(&mut self) -> Result<Integration> {
        Ok(Integration::Fd(self.poller.fd()))
    }
}

fn execute(
    r: &mut Resource,
    p: &mut Pending,
    pool: &BufferPool,
) -> Result<Option<(Outcome<Detached>, bool)>> {
    let fd = r.transport.fd.as_raw_fd();
    match &mut p.request.operation {
        Operation::Connect => {
            if !r.connecting {
                let a = r.connect.as_ref().ok_or(Error::new(ErrorKind::InvalidInput))?;
                // SAFETY: nonblocking socket and live initialized sockaddr.
                let n = unsafe { libc::connect(fd, a.ptr(), a.len) };
                if n < 0 {
                    let e = last_error();
                    if e.os == Some(libc::EINPROGRESS) {
                        r.connecting = true;
                        r.ready[1] = false;
                        return Ok(None);
                    }
                    return Err(e);
                }
            } else {
                let mut error: i32 = 0;
                let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
                // SAFETY: initialized integer output and length match SO_ERROR's ABI.
                if unsafe {
                    libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_ERROR,
                        (&mut error as *mut i32).cast(),
                        &mut len,
                    )
                } < 0
                {
                    return Err(last_error());
                }
                if error != 0 {
                    return Err(std::io::Error::from_raw_os_error(error).into());
                }
            }
            r.connect = None;
            Ok(Some((Outcome::Connected, true)))
        }
        Operation::SendHandle(_) => {
            super::ipc::send(fd, p.passed.as_ref().ok_or(Error::new(ErrorKind::InvalidInput))?.fd.as_raw_fd())?;
            Ok(Some((Outcome::HandleSent, true)))
        }
        Operation::RecvHandle => Ok(Some((Outcome::HandleReceived(super::ipc::receive(fd)?), true))),
        Operation::Accept { multishot } => {
            if r.transport.kind == Kind::PipeListener {
                return Ok(Some((Outcome::PipeAccepted(super::ipc::accept(fd)?), !*multishot)));
            }
            let (fd, peer) = socket::accept(fd)?;
            Ok(Some((
                Outcome::Accepted {
                    transport: Detached {
                        fd,
                        kind: Kind::Tcp,
                    },
                    peer,
                },
                !*multishot,
            )))
        }
        Operation::Read { buf, multishot } => receive(fd, buf, *multishot, false, pool),
        Operation::RecvFrom(buf) => receive(fd, buf, false, true, pool),
        Operation::Write(buf) => {
            let bytes = buf.as_slice();
            if p.offset == bytes.len() {
                return Ok(Some((Outcome::Wrote(p.offset), true)));
            }
            // SAFETY: WriteBuf guarantees stable initialized bytes until completion.
            let n = unsafe {
                if matches!(r.transport.kind, Kind::Stream | Kind::File) {
                    super::ipc::write(fd, bytes[p.offset..].as_ptr().cast(), bytes.len() - p.offset)
                } else {
                    libc::send(fd, bytes[p.offset..].as_ptr().cast(), bytes.len() - p.offset, send_flags())
                }
            };
            if n < 0 {
                return Err(last_error());
            }
            if n == 0 {
                return Err(Error::new(ErrorKind::BrokenPipe));
            }
            p.offset += n as usize;
            Ok((p.offset == bytes.len()).then_some((Outcome::Wrote(p.offset), true)))
        }
        Operation::Writev(bufs) => {
            let mut iov = [libc::iovec {
                iov_base: std::ptr::null_mut(),
                iov_len: 0,
            }; MAX_IOV];
            let mut skip = p.offset;
            let mut count = 0;
            for b in bufs.bufs.iter().flatten() {
                let bytes = b.as_slice();
                if skip >= bytes.len() {
                    skip -= bytes.len();
                    continue;
                }
                iov[count] = libc::iovec {
                    iov_base: bytes[skip..].as_ptr().cast_mut().cast(),
                    iov_len: bytes.len() - skip,
                };
                count += 1;
                skip = 0;
            }
            if count == 0 {
                return Ok(Some((Outcome::Wrote(p.offset), true)));
            }
            // SAFETY: all-zero msghdr is valid before assigning the iovec fields.
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = iov.as_mut_ptr();
            msg.msg_iovlen = count as _;
            // SAFETY: msghdr references initialized iovecs and stable buffer regions.
            let n = unsafe {
                if matches!(r.transport.kind, Kind::Stream | Kind::File) {
                    super::ipc::writev(fd, iov.as_ptr(), count as i32)
                } else { libc::sendmsg(fd, &msg, send_flags()) }
            };
            if n < 0 {
                return Err(last_error());
            }
            if n == 0 {
                return Err(Error::new(ErrorKind::BrokenPipe));
            }
            p.offset += n as usize;
            let len: usize = bufs.bufs.iter().flatten().map(|b| b.as_slice().len()).sum();
            Ok((p.offset == len).then_some((Outcome::Wrote(p.offset), true)))
        }
        Operation::SendTo { buf, to } => {
            let a = Addr::new(*to);
            let bytes = buf.as_slice();
            // SAFETY: initialized address and stable readable datagram payload.
            let n = unsafe {
                libc::sendto(
                    fd,
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    send_flags(),
                    a.ptr(),
                    a.len,
                )
            };
            if n < 0 {
                return Err(last_error());
            }
            Ok(Some((Outcome::Wrote(n as usize), true)))
        }
        Operation::Shutdown => {
            // SAFETY: fd is a live TCP socket; SHUT_WR is a valid shutdown direction.
            if unsafe { libc::shutdown(fd, libc::SHUT_WR) } < 0 {
                return Err(last_error());
            }
            Ok(Some((Outcome::Shutdown, true)))
        }
    }
}
fn send_flags() -> i32 {
    #[cfg(turnloop_backend = "epoll")]
    {
        libc::MSG_NOSIGNAL
    }
    #[cfg(turnloop_backend = "kqueue")]
    {
        0
    }
}
fn receive(
    fd: i32,
    buf: &mut ReadBuf,
    multishot: bool,
    udp: bool,
    pool: &BufferPool,
) -> Result<Option<(Outcome<Detached>, bool)>> {
    let mut lease = None;
    let (ptr, len) = match buf {
        ReadBuf::Provided(b) => (b.as_mut_ptr(), b.len()),
        ReadBuf::Pooled => {
            let Some(b) = pool.acquire() else {
                return Ok(None);
            };
            let b = lease.insert(b);
            let bytes = b.writable();
            (bytes.as_mut_ptr(), bytes.len())
        }
    };
    let mut a = Addr::empty();
    // SAFETY: provided memory obeys the exclusive writable-region contract, or is
    // exclusively held by this pool lease. Address output storage is valid.
    let n = unsafe {
        if udp {
            libc::recvfrom(fd, ptr.cast(), len, 0, a.mut_ptr(), &mut a.len)
        } else {
            libc::read(fd, ptr.cast(), len)
        }
    };
    if n < 0 {
        return Err(last_error());
    }
    let n = n as usize;
    if let Some(b) = &mut lease {
        b.set_len(n);
    }
    if udp {
        Ok(Some((
            Outcome::RecvFrom {
                n,
                from: a.decode()?,
                lease,
            },
            true,
        )))
    } else if n == 0 {
        Ok(Some((Outcome::Eof, true)))
    } else {
        Ok(Some((Outcome::Read { n, lease }, !multishot)))
    }
}
