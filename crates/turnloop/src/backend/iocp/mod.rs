//! Native IOCP completion backend. Kernel storage is pinned separately from Rust
//! operation metadata and survives cancellation until the OS acknowledgement.
mod bridge;
mod integration;
mod ipc;
mod pipes;
mod port;
mod process;
mod services;
mod signals;
mod socket;
mod sync_io;
mod timer;

use crate::{
    backend::{Backend, Event, Operation, Outcome, PollInfo, Request},
    *,
};
use integration::EventIntegration;
use port::{Entry, Port, TIMER, WAKE, Wait};
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    mem::size_of,
    net::SocketAddr,
    os::windows::io::{AsRawHandle, AsRawSocket, OwnedHandle, OwnedSocket},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    Networking::WinSock::*,
    Storage::FileSystem::{ReadFile, SetFileCompletionNotificationModes, WriteFile},
    System::{
        IO::{CancelIoEx, OVERLAPPED},
        Pipes::ConnectNamedPipe,
    },
};

fn invalid() -> Error {
    Error::new(ErrorKind::InvalidInput)
}
fn unsupported() -> Error {
    Error::new(ErrorKind::Unsupported)
}
fn os_error() -> Error {
    std::io::Error::last_os_error().into()
}
fn bool_result(value: i32) -> Result<()> {
    if value == 0 { Err(os_error()) } else { Ok(()) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Tcp,
    Listener,
    Udp,
    Pipe,
    PipeListener,
    Sync,
}
#[derive(Debug)]
enum Native {
    Socket(OwnedSocket),
    Handle(OwnedHandle),
}
impl Native {
    fn raw(&self) -> HANDLE {
        match self {
            Self::Socket(s) => s.as_raw_socket() as HANDLE,
            Self::Handle(h) => h.as_raw_handle(),
        }
    }
}
/// Owning, quiescent Windows transport. Transfer preserves independent ownership;
/// a prior IOCP association is routed through overlapped events on the next loop.
#[derive(Debug)]
pub struct Detached {
    native: Native,
    kind: Kind,
    routed: bool,
    mode: Option<u32>,
    console_input: bool,
}
impl Detached {
    fn new(native: Native, kind: Kind, routed: bool) -> Self {
        Self {
            native,
            kind,
            routed,
            mode: None,
            console_input: false,
        }
    }
}
impl Drop for Detached {
    fn drop(&mut self) {
        if let Some(mode) = self.mode {
            // SAFETY: owned console handle is still live; mode was captured on adoption.
            unsafe {
                windows_sys::Win32::System::Console::SetConsoleMode(self.native.raw(), mode);
            }
        }
    }
}
struct Resource {
    handle: Handle,
    transport: Detached,
    connect: Option<SocketAddr>,
    name: Option<Vec<u16>>,
    skip: bool,
    heads: [Option<usize>; 2],
    tails: [Option<usize>; 2],
}
#[repr(C)]
struct Kernel {
    overlapped: OVERLAPPED,
    addr: SOCKADDR_STORAGE,
    addr_len: i32,
    flags: u32,
    accept_addr: [u8; 288],
    wire: [u32; 160],
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Start,
    Io,
    IdleRead,
}
struct Pending {
    request: Request,
    next: Option<usize>,
    stage: Stage,
    waiting: bool,
    queued: bool,
    pool_wait: bool,
    cancelled: bool,
    completion: Option<Result<u32>>,
    offset: usize,
    lease: Option<BufLease>,
    accepted: Option<Detached>,
    passed: Option<OwnedSocket>,
    direction: usize,
}
fn direction(operation: &Operation) -> usize {
    usize::from(!matches!(
        operation,
        Operation::Read { .. }
            | Operation::RecvFrom(_)
            | Operation::Accept { .. }
            | Operation::RecvHandle
    ))
}
/// Lifetime-safe, parking-aware IOCP wake endpoint.
pub struct IocpWake {
    port: Arc<Port>,
    calls: AtomicU64,
}
impl crate::backend::Wake for IocpWake {
    fn wake(&self) -> Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.port.post(WAKE, 0).map_err(Into::into)
    }
    fn syscall_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}
/// One IOCP per host-driven loop, with fixed operation storage and an opt-in GUI helper.
pub struct Iocp {
    resources: Vec<Option<Resource>>,
    ops: Vec<Option<Pending>>,
    kernel: Box<[UnsafeCell<Kernel>]>,
    bridges: Vec<Option<bridge::Bridge>>,
    ready: VecDeque<usize>,
    pool_waiting: VecDeque<OpId>,
    pool: BufferPool,
    port: Arc<Port>,
    wake: Arc<IocpWake>,
    timer: timer::PacketTimer,
    event: Option<EventIntegration>,
    notifier: Option<Notifier>,
    services: services::Services,
    workers: Vec<Option<sync_io::Worker>>,
    deadline: Option<Instant>,
    failure: Option<Error>,
}
impl Iocp {
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
        connect: Option<SocketAddr>,
        name: Option<Vec<u16>>,
    ) -> Result<()> {
        if self.resources.get(h.index()).is_none_or(Option::is_some) {
            return Err(invalid());
        }
        let raw = transport.native.raw();
        let mut skip = false;
        if let Native::Socket(_) = transport.native {
            socket::nonblocking(raw as usize)?;
        }
        if transport.kind != Kind::Sync && !transport.routed {
            // SAFETY: freshly created, exclusively owned overlapped resource.
            unsafe { self.port.associate(raw, 1) }.map_err(Error::from)?;
            let ifs = if matches!(transport.native, Native::Socket(_)) {
                socket::ifs(raw as usize)?
            } else {
                true
            };
            if ifs {
                // SAFETY: live overlapped IFS handle. Unsupported skip mode retains
                // normal completion packets, including synchronous successes.
                skip = unsafe { SetFileCompletionNotificationModes(raw, 1) } != 0;
            }
        }
        if transport.kind == Kind::Sync {
            // Adoption is resource setup. Reserve the worker here so even the
            // first read after pooled-buffer backpressure needs no allocation.
            self.workers[h.index()] = Some(sync_io::Worker::new(raw, Arc::clone(&self.port))?);
        }
        self.resources[h.index()] = Some(Resource {
            handle: h,
            transport,
            connect,
            name,
            skip,
            heads: [None; 2],
            tails: [None; 2],
        });
        Ok(())
    }
    fn schedule(&mut self, i: usize) {
        if let Some(p) = &mut self.ops[i]
            && !p.queued
            && !p.waiting
        {
            p.queued = true;
            self.ready.push_back(i);
        }
    }
    fn unlink(&mut self, i: usize, p: &Pending) {
        let h = p.request.handle;
        let d = p.direction;
        let r = self.resources[h.index()].as_mut().expect("live resource");
        if r.heads[d] == Some(i) {
            r.heads[d] = p.next;
            if r.tails[d] == Some(i) {
                r.tails[d] = None;
            }
        } else {
            let mut at = r.heads[d];
            while let Some(j) = at {
                let previous = self.ops[j].as_mut().expect("queued operation");
                if previous.next == Some(i) {
                    previous.next = p.next;
                    if r.tails[d] == Some(i) {
                        r.tails[d] = Some(j);
                    }
                    break;
                }
                at = previous.next;
            }
        }
        if let Some(next) = r.heads[d] {
            self.schedule(next);
        }
    }
    fn run_ready(&mut self, events: &mut Vec<Event<Detached>>) {
        if self.pool.available() {
            while let Some(op) = self.pool_waiting.pop_front() {
                self.ops[op.index()]
                    .as_mut()
                    .expect("pool waiter")
                    .pool_wait = false;
                self.schedule(op.index());
            }
        }
        let budget = self.ready.len();
        for _ in 0..budget {
            if events.len() == events.capacity() {
                break;
            }
            let i = self.ready.pop_front().expect("ready operation");
            let Some(mut p) = self.ops[i].take() else {
                continue;
            };
            p.queued = false;
            let result = if p.cancelled {
                Ok(Some((Outcome::Cancelled, true)))
            } else {
                self.execute(i, &mut p)
            };
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    self.unlink(i, &p);
                    events.push(Event {
                        op: p.request.op,
                        terminal: true,
                        result: Err(error),
                    });
                    continue;
                }
            };
            if let Some((result, terminal)) = result {
                if terminal {
                    self.unlink(i, &p);
                }
                events.push(Event {
                    op: p.request.op,
                    terminal,
                    result: Ok(result),
                });
                if terminal {
                    continue;
                }
                p.stage = Stage::Start;
                p.completion = None;
            }
            let runnable = !p.waiting && !p.pool_wait;
            if p.pool_wait {
                self.pool_waiting.push_back(p.request.op);
            }
            self.ops[i] = Some(p);
            if runnable {
                self.schedule(i);
            }
        }
    }
    fn kernel_ptr(&self, i: usize) -> *mut OVERLAPPED {
        self.kernel[i].get().cast()
    }
    fn io_raw(&self, p: &Pending) -> HANDLE {
        if matches!(p.request.operation, Operation::Accept { .. })
            && let Some(accepted) = &p.accepted
            && accepted.kind == Kind::Pipe
        {
            return accepted.native.raw();
        }
        self.resources[p.request.handle.index()]
            .as_ref()
            .expect("resource")
            .transport
            .native
            .raw()
    }
    fn prepare(&mut self, i: usize, p: &Pending) -> Result<*mut Kernel> {
        let raw = self.kernel[i].get();
        // SAFETY: caller only prepares after the previous native completion and
        // registered-wait callback have both quiesced. Slab storage never moves.
        unsafe {
            ptr::write(raw, std::mem::zeroed());
        }
        let resource = self.get(p.request.handle)?;
        if resource.transport.routed {
            if self.bridges[i].is_none() {
                self.bridges[i] = Some(bridge::Bridge::new(
                    Arc::clone(&self.port),
                    self.kernel_ptr(i),
                )?);
            }
            let handle = self.io_raw(p);
            let event = self.bridges[i].as_mut().expect("bridge").prepare(handle)?;
            // SAFETY: inactive kernel slot. Low event bit suppresses the old IOCP
            // association, including when socket duplication retained that association.
            unsafe {
                (*raw).overlapped.hEvent = (event as usize | 1) as HANDLE;
            }
        }
        Ok(raw)
    }
    fn submitted(
        &mut self,
        i: usize,
        p: &mut Pending,
        success: bool,
        bytes: u32,
        error: Error,
        stage: Stage,
    ) -> Result<()> {
        let r = self.get(p.request.handle)?;
        p.stage = stage;
        if success && (r.skip || r.transport.routed) {
            p.completion = Some(Ok(bytes));
        } else if success || error.os == Some(ERROR_IO_PENDING as i32) {
            p.waiting = true;
            if r.transport.routed {
                // A failed wait registration cannot release an in-flight request.
                // Bridge::start cancels and synchronously drains on setup failure.
                if let Err(error) = self.bridges[i].as_mut().expect("bridge").start() {
                    p.waiting = false;
                    return Err(error);
                }
            }
        } else {
            p.completion = Some(Err(error));
        }
        Ok(())
    }
    fn execute(&mut self, i: usize, p: &mut Pending) -> Result<Option<(Outcome<Detached>, bool)>> {
        if matches!(
            p.request.operation,
            Operation::SendHandle(_) | Operation::RecvHandle
        ) {
            return self.ipc_step(i, p);
        }
        let r = self.get(p.request.handle)?;
        let raw = r.transport.native.raw();
        let kind = r.transport.kind;
        if let Some(result) = p.completion.take() {
            let bytes = match result {
                Ok(bytes) => bytes as usize,
                Err(error)
                    if matches!(p.request.operation, Operation::Read { .. })
                        && matches!(error.os, Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_HANDLE_EOF as i32) =>
                {
                    return Ok(Some((Outcome::Eof, true)));
                }
                Err(error) => return Err(error),
            };
            if p.stage == Stage::IdleRead {
                return self.read_ready(p, raw as usize);
            }
            match &p.request.operation {
                Operation::Connect => {
                    if kind == Kind::Tcp {
                        // SAFETY: successful ConnectEx requires context update before use.
                        socket::check(unsafe {
                            setsockopt(
                                raw as usize,
                                SOL_SOCKET,
                                SO_UPDATE_CONNECT_CONTEXT,
                                ptr::null(),
                                0,
                            )
                        })?;
                    }
                    return Ok(Some((Outcome::Connected, true)));
                }
                Operation::Accept { multishot } => {
                    let transport = p.accepted.take().expect("accept transport");
                    let outcome = if kind == Kind::Listener {
                        let listener = raw as usize;
                        // SAFETY: AcceptEx completed; option consumes the live listener value.
                        socket::check(unsafe {
                            setsockopt(
                                transport.native.raw() as usize,
                                SOL_SOCKET,
                                SO_UPDATE_ACCEPT_CONTEXT,
                                ptr::from_ref(&listener).cast(),
                                size_of::<usize>() as i32,
                            )
                        })?;
                        let peer = socket::address(transport.native.raw() as usize, true)?;
                        Outcome::Accepted { transport, peer }
                    } else {
                        Outcome::PipeAccepted(transport)
                    };
                    return Ok(Some((outcome, !multishot)));
                }
                Operation::Read { multishot, .. } => {
                    if bytes == 0 {
                        return Ok(Some((Outcome::Eof, true)));
                    }
                    let mut lease = p.lease.take();
                    if let Some(lease) = &mut lease {
                        lease.set_len(bytes);
                    }
                    return Ok(Some((Outcome::Read { n: bytes, lease }, !multishot)));
                }
                Operation::RecvFrom(_) => {
                    // SAFETY: native receive has completed, so the address output is quiescent.
                    let k = unsafe { &*self.kernel[i].get() };
                    let from = socket::decode(&k.addr, k.addr_len)?;
                    let mut lease = p.lease.take();
                    if let Some(lease) = &mut lease {
                        lease.set_len(bytes);
                    }
                    return Ok(Some((
                        Outcome::RecvFrom {
                            n: bytes,
                            from,
                            lease,
                        },
                        true,
                    )));
                }
                Operation::SendTo { .. } => return Ok(Some((Outcome::Wrote(bytes), true))),
                Operation::Write(_) | Operation::Writev(_) => {
                    if bytes == 0 && write_length(&p.request.operation) != p.offset {
                        return Err(Error::new(ErrorKind::BrokenPipe));
                    }
                    p.offset += bytes;
                    if p.offset == write_length(&p.request.operation) {
                        return Ok(Some((Outcome::Wrote(p.offset), true)));
                    }
                    p.stage = Stage::Start;
                }
                _ => return Err(unsupported()),
            }
        }
        let r = self.get(p.request.handle)?;
        if kind == Kind::Sync {
            let write = matches!(
                p.request.operation,
                Operation::Write(_) | Operation::Writev(_)
            );
            let (buffer, len) = if write {
                let mut buffers = [WSABUF {
                    len: 0,
                    buf: ptr::null_mut(),
                }; MAX_IOV];
                if write_buffers(&p.request.operation, p.offset, &mut buffers) == 0 {
                    return Ok(Some((Outcome::Wrote(p.offset), true)));
                }
                (buffers[0].buf, buffers[0].len)
            } else {
                match read_buffer(p, &self.pool) {
                    Some(buf) => buf,
                    None => {
                        p.pool_wait = true;
                        return Ok(None);
                    }
                }
            };
            let index = p.request.handle.index();
            self.workers[index].as_ref().expect("stdio worker").start(
                buffer,
                len,
                write,
                self.kernel_ptr(i),
            );
            p.stage = Stage::Io;
            p.waiting = true;
            return Ok(None);
        }
        match &p.request.operation {
            Operation::Shutdown => {
                if matches!(kind, Kind::Tcp) {
                    // SAFETY: live stream; sends ahead of shutdown have completed in FIFO order.
                    socket::check(unsafe { shutdown(raw as usize, SD_SEND) })?;
                } else {
                    return Err(unsupported());
                }
                return Ok(Some((Outcome::Shutdown, true)));
            }
            Operation::Connect if kind == Kind::Pipe => {
                return Ok(Some((Outcome::Connected, true)));
            }
            Operation::Read {
                buf: ReadBuf::Pooled,
                ..
            } if kind == Kind::Tcp => {
                let k = self.prepare(i, p)?;
                let buf = WSABUF {
                    len: 0,
                    buf: ptr::null_mut(),
                };
                let mut bytes = 0;
                // SAFETY: pinned OVERLAPPED/flags, descriptor captured synchronously;
                // zero-byte receive retains no payload or pool lease while idle.
                let ok = unsafe {
                    WSARecv(
                        raw as usize,
                        &buf,
                        1,
                        &mut bytes,
                        ptr::addr_of_mut!((*k).flags),
                        k.cast(),
                        None,
                    )
                } == 0;
                self.submitted(i, p, ok, bytes, socket::last_error(), Stage::IdleRead)?;
            }
            Operation::Read { .. } | Operation::RecvFrom(_) => {
                let (buf, len) = match read_buffer(p, &self.pool) {
                    Some(buf) => buf,
                    None => {
                        p.pool_wait = true;
                        return Ok(None);
                    }
                };
                let k = self.prepare(i, p)?;
                let mut bytes = 0;
                let (ok, error) = if matches!(kind, Kind::Tcp | Kind::Udp) {
                    let buf = WSABUF { len, buf };
                    // SAFETY: stable provided/leased storage; kernel output lives in
                    // the pinned slab through acknowledgement. WSABUF is captured.
                    let code = unsafe {
                        if kind == Kind::Udp {
                            (*k).addr_len = size_of::<SOCKADDR_STORAGE>() as i32;
                            WSARecvFrom(
                                raw as usize,
                                &buf,
                                1,
                                &mut bytes,
                                ptr::addr_of_mut!((*k).flags),
                                ptr::addr_of_mut!((*k).addr).cast(),
                                ptr::addr_of_mut!((*k).addr_len),
                                k.cast(),
                                None,
                            )
                        } else {
                            WSARecv(
                                raw as usize,
                                &buf,
                                1,
                                &mut bytes,
                                ptr::addr_of_mut!((*k).flags),
                                k.cast(),
                                None,
                            )
                        }
                    };
                    (code == 0, socket::last_error())
                } else {
                    // SAFETY: live overlapped pipe and stable exclusive buffer until completion.
                    let ok = unsafe { ReadFile(raw, buf, len, &mut bytes, k.cast()) } != 0;
                    (ok, os_error())
                };
                self.submitted(i, p, ok, bytes, error, Stage::Io)?;
            }
            Operation::Write(_) | Operation::Writev(_) | Operation::SendTo { .. } => {
                let k = self.prepare(i, p)?;
                let mut buffers = [WSABUF {
                    len: 0,
                    buf: ptr::null_mut(),
                }; MAX_IOV];
                let n = write_buffers(&p.request.operation, p.offset, &mut buffers);
                if n == 0 && !matches!(p.request.operation, Operation::SendTo { .. }) {
                    return Ok(Some((Outcome::Wrote(p.offset), true)));
                }
                let mut bytes = 0;
                let (ok, error) = if matches!(kind, Kind::Tcp | Kind::Udp) {
                    // SAFETY: request retains immutable payload until completion;
                    // Winsock captures stack descriptors and destination synchronously.
                    let code = unsafe {
                        if let Operation::SendTo { to, .. } = p.request.operation {
                            let addr = socket::Addr::new(to);
                            WSASendTo(
                                raw as usize,
                                buffers.as_ptr(),
                                n.max(1) as u32,
                                &mut bytes,
                                0,
                                addr.as_ptr(),
                                addr.len,
                                k.cast(),
                                None,
                            )
                        } else {
                            WSASend(
                                raw as usize,
                                buffers.as_ptr(),
                                n as u32,
                                &mut bytes,
                                0,
                                k.cast(),
                                None,
                            )
                        }
                    };
                    (code == 0, socket::last_error())
                } else {
                    // SAFETY: overlapped pipe; first remaining segment retained until completion.
                    let ok = unsafe {
                        WriteFile(raw, buffers[0].buf, buffers[0].len, &mut bytes, k.cast())
                    } != 0;
                    (ok, os_error())
                };
                self.submitted(i, p, ok, bytes, error, Stage::Io)?;
            }
            Operation::Connect => {
                let addr = socket::Addr::new(r.connect.ok_or_else(invalid)?);
                // SAFETY: nullable ConnectEx pointer matches the provider GUID.
                let connect =
                    unsafe { socket::extension::<LPFN_CONNECTEX>(raw as usize, &WSAID_CONNECTEX) }?
                        .ok_or_else(unsupported)?;
                let k = self.prepare(i, p)?;
                let mut bytes = 0;
                // SAFETY: explicitly bound socket; address captured; pinned OVERLAPPED.
                let ok = unsafe {
                    connect(
                        raw as usize,
                        addr.as_ptr(),
                        addr.len,
                        ptr::null(),
                        0,
                        &mut bytes,
                        k.cast(),
                    )
                } != 0;
                self.submitted(i, p, ok, bytes, socket::last_error(), Stage::Io)?;
            }
            Operation::Accept { .. } if kind == Kind::Listener => {
                let v6 = socket::address(raw as usize, false)?.is_ipv6();
                p.accepted = Some(Detached::new(
                    Native::Socket(socket::create(v6, false)?),
                    Kind::Tcp,
                    false,
                ));
                // SAFETY: nullable AcceptEx pointer matches the provider GUID.
                let accept =
                    unsafe { socket::extension::<LPFN_ACCEPTEX>(raw as usize, &WSAID_ACCEPTEX) }?
                        .ok_or_else(unsupported)?;
                let k = self.prepare(i, p)?;
                let accepted = p.accepted.as_ref().expect("socket").native.raw() as usize;
                let mut bytes = 0;
                // SAFETY: listener and secondary socket stay owned until completion;
                // address buffer is pinned and has two sockaddr_storage + 16 regions.
                let ok = unsafe {
                    accept(
                        raw as usize,
                        accepted,
                        ptr::addr_of_mut!((*k).accept_addr).cast(),
                        0,
                        144,
                        144,
                        &mut bytes,
                        k.cast(),
                    )
                } != 0;
                self.submitted(i, p, ok, bytes, socket::last_error(), Stage::Io)?;
            }
            Operation::Accept { .. } if kind == Kind::PipeListener => {
                let next = pipes::instance(r.name.as_ref().expect("pipe name"), false)?;
                // SAFETY: newly created overlapped pipe is unassociated.
                unsafe { self.port.associate(next.native.raw(), 1) }.map_err(Error::from)?;
                // SAFETY: fresh pipe supports skip-success.
                bool_result(unsafe { SetFileCompletionNotificationModes(next.native.raw(), 1) })?;
                let r = self.resources[p.request.handle.index()]
                    .as_mut()
                    .expect("listener");
                let mut accepted = std::mem::replace(&mut r.transport, next);
                accepted.kind = Kind::Pipe;
                accepted.routed = true;
                p.accepted = Some(accepted);
                let k = self.prepare(i, p)?;
                let accept_raw = self.io_raw(p);
                // SAFETY: owned overlapped instance and pinned operation.
                let ok = unsafe { ConnectNamedPipe(accept_raw, k.cast()) } != 0;
                let error = os_error();
                if error.os == Some(ERROR_PIPE_CONNECTED as i32) && !ok {
                    p.stage = Stage::Io;
                    p.completion = Some(Ok(0));
                } else {
                    self.submitted(i, p, ok, 0, error, Stage::Io)?;
                }
            }
            _ => return Err(unsupported()),
        }
        Ok(None)
    }
    fn read_ready(
        &mut self,
        p: &mut Pending,
        raw: usize,
    ) -> Result<Option<(Outcome<Detached>, bool)>> {
        let Some(mut lease) = self.pool.acquire() else {
            p.pool_wait = true;
            p.completion = Some(Ok(0));
            return Ok(None);
        };
        let buf = lease.writable();
        // SAFETY: nonblocking socket and exclusive lease; synchronous recv retains no pointer.
        let n = unsafe {
            recv(
                raw,
                buf.as_mut_ptr(),
                buf.len().min(i32::MAX as usize) as i32,
                0,
            )
        };
        if n == SOCKET_ERROR {
            let error = socket::last_error();
            if error.kind == ErrorKind::WouldBlock {
                p.stage = Stage::Start;
                return Ok(None);
            }
            return Err(error);
        }
        if n == 0 {
            return Ok(Some((Outcome::Eof, true)));
        }
        lease.set_len(n as usize);
        let multishot = matches!(
            p.request.operation,
            Operation::Read {
                multishot: true,
                ..
            }
        );
        Ok(Some((
            Outcome::Read {
                n: n as usize,
                lease: Some(lease),
            },
            !multishot,
        )))
    }
    fn entries(&mut self, entries: &[Entry]) -> Result<()> {
        for entry in entries {
            if entry.key == WAKE {
                continue;
            }
            if entry.key == TIMER {
                // SAFETY: this port has exactly one associated deadline timer.
                unsafe {
                    self.timer.dequeued(entry.overlapped);
                }
                if self.event.is_some()
                    && self
                        .deadline
                        .is_some_and(|deadline| deadline > Instant::now())
                {
                    self.arm_event_deadline()?;
                }
                continue;
            }
            let base = self.kernel.as_ptr() as usize;
            let stride = size_of::<UnsafeCell<Kernel>>();
            let offset = entry.overlapped.checked_sub(base).ok_or_else(invalid)?;
            if offset % stride != 0 || offset / stride >= self.ops.len() {
                return Err(invalid());
            }
            let i = offset / stride;
            let bridge_result = if entry.key == sync_io::KEY {
                let p = self.ops[i].as_ref().ok_or_else(invalid)?;
                Some(
                    self.workers[p.request.handle.index()]
                        .as_ref()
                        .ok_or_else(invalid)?
                        .finish(),
                )
            } else if entry.key == bridge::KEY {
                Some(self.bridges[i].as_mut().ok_or_else(invalid)?.finish()?)
            } else {
                None
            };
            let p = self.ops[i].as_mut().ok_or_else(invalid)?;
            if !p.waiting {
                return Err(invalid());
            }
            p.waiting = false;
            p.completion = Some(bridge_result.unwrap_or_else(|| {
                if entry.status < 0 {
                    // SAFETY: pure NTSTATUS conversion, no pointers.
                    Err(socket::error(
                        unsafe { RtlNtStatusToDosError(entry.status) } as i32,
                    ))
                } else {
                    Ok(entry.bytes)
                }
            }));
            self.schedule(i);
        }
        Ok(())
    }
}
// SAFETY: accepted requests and pinned kernel storage stay owned until terminal
// acknowledgement. Cancel never frees pending storage. Drop cancels and drains
// native I/O and joins routed completion callbacks before releasing any buffers.
unsafe impl Backend for Iocp {
    type Wake = IocpWake;
    type Detached = Detached;
    fn new(config: &Config, pool: BufferPool) -> Result<Self> {
        socket::startup()?;
        let port = Arc::new(Port::new()?);
        let kernel: Box<[UnsafeCell<Kernel>]> = (0..config.max_operations)
            .map(|_| {
                // SAFETY: inactive all-zero kernel I/O storage is valid.
                UnsafeCell::new(unsafe { std::mem::zeroed() })
            })
            .collect();
        let bridges = kernel
            .iter()
            .map(|slot| bridge::Bridge::new(Arc::clone(&port), slot.get().cast()).map(Some))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            resources: (0..config.max_handles).map(|_| None).collect(),
            ops: (0..config.max_operations).map(|_| None).collect(),
            kernel,
            bridges,
            ready: VecDeque::with_capacity(config.max_operations),
            pool_waiting: VecDeque::with_capacity(config.max_operations),
            pool,
            wake: Arc::new(IocpWake {
                port: Arc::clone(&port),
                calls: AtomicU64::new(0),
            }),
            timer: timer::PacketTimer::new(Arc::clone(&port))?,
            port,
            event: None,
            notifier: None,
            services: services::Services::new(config.max_handles),
            workers: (0..config.max_handles).map(|_| None).collect(),
            deadline: None,
            failure: None,
        })
    }
    fn set_notifier(&mut self, notifier: Notifier) {
        self.notifier = Some(notifier);
    }
    fn spawn(
        &mut self,
        handle: Handle,
        pipes: [Option<Handle>; 3],
        spec: &ProcessSpec,
    ) -> Result<u32> {
        let mut existing = [None; 3];
        for (i, option) in spec.stdio.iter().enumerate() {
            if let ProcessStdio::Handle(h) = option {
                existing[i] = Some(self.get(*h)?.transport.native.raw());
            }
        }
        let (child, parents) =
            process::spawn(spec, existing, self.notifier.clone().ok_or_else(invalid)?)?;
        let pid = child.pid;
        let result = (|| {
            for (h, parent) in pipes.into_iter().zip(parents) {
                if let (Some(h), Some(parent)) = (h, parent) {
                    self.install(h, parent, None, None)?;
                }
            }
            child.resume()
        })();
        if let Err(error) = result {
            for h in pipes.into_iter().flatten() {
                self.release(h);
            }
            return Err(error);
        }
        self.services.child(handle, child);
        Ok(pid)
    }
    fn prepare_close(&mut self, handle: Handle) -> Result<()> {
        self.services.close(handle)
    }
    fn kill(&mut self, handle: Handle, signal: Signal, group: bool) -> Result<()> {
        self.services.kill(handle, signal, group)
    }
    fn signal(&mut self, handle: Handle, signal: Signal) -> Result<()> {
        self.services
            .signal(handle, signal, self.notifier.clone().ok_or_else(invalid)?)
    }
    fn tty_set_mode(&mut self, handle: Handle, mode: TtyMode) -> Result<()> {
        use windows_sys::Win32::System::Console::*;
        let r = self.get(handle)?;
        let original = r.transport.mode.ok_or_else(unsupported)?;
        let raw = r.transport.native.raw();
        let flags = match mode {
            TtyMode::Normal => original,
            TtyMode::Raw | TtyMode::Io if r.transport.console_input => {
                let mut flags = (original
                    | ENABLE_WINDOW_INPUT
                    | ENABLE_VIRTUAL_TERMINAL_INPUT
                    | ENABLE_EXTENDED_FLAGS)
                    & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_QUICK_EDIT_MODE);
                if mode == TtyMode::Io {
                    flags &= !ENABLE_PROCESSED_INPUT;
                }
                flags
            }
            TtyMode::Raw | TtyMode::Io => {
                original | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING
            }
        };
        // SAFETY: mode was captured from this live console handle.
        bool_result(unsafe { SetConsoleMode(raw, flags) })
    }
    fn tty_window_size(&self, handle: Handle) -> Result<WindowSize> {
        use windows_sys::Win32::System::Console::*;
        let r = self.get(handle)?;
        if r.transport.mode.is_none() {
            return Err(unsupported());
        }
        // SAFETY: plain writable console screen-buffer information.
        let mut info = unsafe { std::mem::zeroed() };
        // SAFETY: live owned console screen-buffer handle and correct output.
        let output = if r.transport.console_input {
            use windows_sys::Win32::Storage::FileSystem::*;
            let name = [67u16, 79, 78, 79, 85, 84, 36, 0]; // CONOUT$
            // SAFETY: explicit console device, exclusively owned temporary query handle.
            Some(unsafe {
                port::owned(CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    ptr::null(),
                    OPEN_EXISTING,
                    0,
                    ptr::null_mut(),
                ))
            }?)
        } else {
            None
        };
        let raw = output
            .as_ref()
            .map_or(r.transport.native.raw(), AsRawHandle::as_raw_handle);
        // SAFETY: owned console screen-buffer handle and correct writable output.
        bool_result(unsafe { GetConsoleScreenBufferInfo(raw, &mut info) })?;
        Ok(WindowSize {
            columns: (info.srWindow.Right - info.srWindow.Left + 1) as u16,
            rows: (info.srWindow.Bottom - info.srWindow.Top + 1) as u16,
        })
    }
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn validate_timeout(&self, timeout: Timeout) -> Result<()> {
        if self.event.is_some() && !matches!(timeout, Timeout::Now) {
            return Err(invalid());
        }
        Ok(())
    }
    fn deadline_changed(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
        if self.event.is_some()
            && let Err(error) = self.arm_event_deadline()
        {
            self.failure = Some(error);
        }
    }
    fn waker(&self) -> Arc<Self::Wake> {
        Arc::clone(&self.wake)
    }
    fn open(&mut self, h: Handle, spec: Open) -> Result<()> {
        let (transport, connect, name) = match spec {
            Open::Tcp { addr, opts } => {
                let socket = socket::create(addr.is_ipv6(), false)?;
                let raw = socket.as_raw_socket() as usize;
                socket::bind_to(
                    raw,
                    if addr.is_ipv6() {
                        ([0u16; 8], 0).into()
                    } else {
                        ([0u8; 4], 0).into()
                    },
                )?;
                if addr.ip().is_loopback() {
                    socket::loopback_connect(raw)?;
                }
                if opts.nodelay {
                    let value = 1i32;
                    // SAFETY: TCP_NODELAY takes an initialized integer value.
                    socket::check(unsafe {
                        setsockopt(
                            raw,
                            IPPROTO_TCP,
                            TCP_NODELAY,
                            ptr::from_ref(&value).cast(),
                            4,
                        )
                    })?;
                }
                (
                    Detached::new(Native::Socket(socket), Kind::Tcp, false),
                    Some(addr),
                    None,
                )
            }
            Open::Listener { addr, opts } => {
                if opts.reuse_port {
                    return Err(unsupported());
                }
                let socket = socket::create(addr.is_ipv6(), false)?;
                let raw = socket.as_raw_socket() as usize;
                socket::option(raw, SO_EXCLUSIVEADDRUSE, 1)?;
                socket::bind_to(raw, addr)?;
                // SAFETY: bound socket; backlog is bounded to the signed API range.
                socket::check(unsafe { listen(raw, opts.backlog.min(i32::MAX as u32) as i32) })?;
                (
                    Detached::new(Native::Socket(socket), Kind::Listener, false),
                    None,
                    None,
                )
            }
            Open::Udp { addr, opts } => {
                if opts.reuse_port {
                    return Err(unsupported());
                }
                let socket = socket::create(addr.is_ipv6(), true)?;
                socket::bind_to(socket.as_raw_socket() as usize, addr)?;
                (
                    Detached::new(Native::Socket(socket), Kind::Udp, false),
                    None,
                    None,
                )
            }
            Open::PipeListener { name, opts } => {
                if opts.reuse_port {
                    return Err(unsupported());
                }
                let name = pipes::name(&name)?;
                (pipes::instance(&name, true)?, None, Some(name))
            }
            Open::Pipe(name) => (pipes::connect(&pipes::name(&name)?)?, None, None),
            Open::Stdio(which) => {
                let i = match which {
                    Stdio::Stdin => 0,
                    Stdio::Stdout => 1,
                    Stdio::Stderr => 2,
                };
                (
                    Detached::from_handle(process::stdio(i, false)?)?,
                    None,
                    None,
                )
            }
        };
        self.install(h, transport, connect, name)
    }
    fn local_addr(&self, h: Handle) -> Result<SocketAddr> {
        let r = self.get(h)?;
        if !matches!(r.transport.native, Native::Socket(_)) {
            return Err(unsupported());
        }
        socket::address(r.transport.native.raw() as usize, false)
    }
    fn submit(&mut self, request: Request) -> Result<()> {
        if self.services.contains(request.handle) {
            return self.services.submit(request);
        }
        let r = self.get(request.handle)?;
        let kind = r.transport.kind;
        let valid = match &request.operation {
            Operation::Connect => matches!(kind, Kind::Tcp | Kind::Pipe),
            Operation::Accept { .. } => matches!(kind, Kind::Listener | Kind::PipeListener),
            Operation::Read { .. } | Operation::Write(_) | Operation::Writev(_) => {
                matches!(kind, Kind::Tcp | Kind::Pipe | Kind::Sync)
            }
            Operation::SendTo { .. } | Operation::RecvFrom(_) => kind == Kind::Udp,
            Operation::Shutdown => kind == Kind::Tcp,
            Operation::SendHandle(_) | Operation::RecvHandle => kind == Kind::Pipe,
            _ => false,
        };
        if !valid {
            return Err(unsupported());
        }
        let passed = if let Operation::SendHandle(source) = request.operation {
            let Native::Socket(socket) = &self.get(source)?.transport.native else {
                return Err(unsupported());
            };
            Some(socket::duplicate(socket)?)
        } else {
            None
        };
        let i = request.op.index();
        if self.ops.get(i).is_none_or(Option::is_some) {
            return Err(invalid());
        }
        let d = if kind == Kind::Sync {
            0
        } else {
            direction(&request.operation)
        };
        let r = self.resources[request.handle.index()]
            .as_mut()
            .expect("resource");
        let previous = r.tails[d];
        r.tails[d] = Some(i);
        if previous.is_none() {
            r.heads[d] = Some(i);
        }
        if let Some(previous) = previous {
            self.ops[previous].as_mut().expect("tail").next = Some(i);
        }
        self.ops[i] = Some(Pending {
            request,
            next: None,
            stage: Stage::Start,
            waiting: false,
            queued: false,
            pool_wait: false,
            cancelled: false,
            completion: None,
            offset: 0,
            lease: None,
            accepted: None,
            passed,
            direction: d,
        });
        if previous.is_none() {
            self.schedule(i);
        }
        Ok(())
    }
    fn cancel(&mut self, op: OpId) -> Result<()> {
        if self.services.cancel(op) {
            return Ok(());
        }
        let Some(p) = self
            .ops
            .get(op.index())
            .and_then(Option::as_ref)
            .filter(|p| p.request.op == op)
        else {
            return Ok(());
        };
        if p.waiting {
            if let Some(worker) = &self.workers[p.request.handle.index()] {
                worker.cancel();
            } else {
                // SAFETY: exact live native request; acknowledgement is still required.
                if unsafe { CancelIoEx(self.io_raw(p), self.kernel_ptr(op.index())) } == 0 {
                    let error = os_error();
                    if error.os != Some(ERROR_NOT_FOUND as i32) {
                        return Err(error);
                    }
                }
            }
        }
        self.ops[op.index()].as_mut().expect("op").cancelled = true;
        if self.ops[op.index()].as_ref().expect("op").pool_wait {
            self.pool_waiting.retain(|queued| *queued != op);
            self.ops[op.index()].as_mut().expect("op").pool_wait = false;
        }
        self.schedule(op.index());
        Ok(())
    }
    fn has_work(&self) -> bool {
        self.failure.is_some()
            || !self.ready.is_empty()
            || self.services.has_work()
            || (self.pool.available() && !self.pool_waiting.is_empty())
    }
    fn poll(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<Event<Detached>>,
    ) -> Result<PollInfo> {
        if let Some(error) = self.failure.take() {
            return Err(error);
        }
        let before = events.len();
        self.services.collect(events)?;
        self.run_ready(events);
        if events.len() != before || events.len() == events.capacity() || !self.ready.is_empty() {
            return Ok(PollInfo::default());
        }
        let mut entries = [Entry::default(); 64];
        let (n, info) = if let Some(event) = &mut self.event {
            (event.drain(&mut entries)?, PollInfo::default())
        } else {
            let armed = timeout.is_some_and(|d| !d.is_zero());
            if let Some(delay) = timeout.filter(|d| !d.is_zero()) {
                self.timer.arm(delay)?;
            }
            // GQCSEx's millisecond timeout may expire at an earlier system tick.
            // Once armed, the exact NT timer is the only deadline wake source.
            let result = self
                .port
                .wait(if armed { None } else { timeout }, false, &mut entries);
            if armed {
                self.timer.cancel()?;
            }
            match result? {
                // The private deadline timer packet has the meaning of a timed wait
                // returning zero (see `PollInfo` in backend/mod.rs, and epoll's timerfd):
                // it is not native work. Notifier and I/O packets still count.
                Wait::Entries(n) => {
                    let timer_only = entries[..n].iter().all(|entry| entry.key == TIMER);
                    (
                        n,
                        PollInfo {
                            waits: 1,
                            zero_event_waits: if timer_only { 1 } else { 0 },
                        },
                    )
                }
                Wait::Timeout | Wait::Apc => (
                    0,
                    PollInfo {
                        waits: 1,
                        zero_event_waits: 1,
                    },
                ),
            }
        };
        self.entries(&entries[..n])?;
        self.services.collect(events)?;
        self.run_ready(events);
        Ok(info)
    }
    fn release(&mut self, h: Handle) {
        self.services.release(h);
        if self.get(h).is_ok() {
            self.workers[h.index()] = None;
            self.resources[h.index()] = None;
        }
    }
    fn detach(&mut self, h: Handle) -> Result<Detached> {
        let r = self.get(h)?;
        if r.transport.kind == Kind::PipeListener {
            return Err(unsupported());
        }
        if r.heads.iter().any(Option::is_some) {
            return Err(Error::new(ErrorKind::WouldBlock));
        }
        let mut r = self.resources[h.index()].take().expect("resource");
        self.workers[h.index()] = None;
        r.transport.routed = true;
        Ok(r.transport)
    }
    fn attach(&mut self, h: Handle, transport: Detached) -> Result<()> {
        self.install(h, transport, None, None)
    }
    fn integration(&mut self) -> Result<Integration> {
        if self.event.is_none() {
            self.event = Some(EventIntegration::new(Arc::clone(&self.port))?);
            self.arm_event_deadline()?;
        }
        if self.has_work() {
            self.port.post(WAKE, 0)?;
        }
        Ok(Integration::Event(
            self.event.as_ref().expect("event").event() as usize,
        ))
    }
}
impl Iocp {
    fn arm_event_deadline(&mut self) -> Result<()> {
        if self.timer.cancel()? == 0x103 {
            return Ok(());
        } // drain the in-flight packet before rearming
        if let Some(deadline) = self.deadline {
            let now = Instant::now();
            if deadline <= now {
                self.port.post(WAKE, 0)?;
            } else {
                self.timer.arm(deadline.saturating_duration_since(now))?;
            }
        }
        Ok(())
    }
}
impl Drop for Iocp {
    fn drop(&mut self) {
        for i in 0..self.ops.len() {
            if let Some(p) = &self.ops[i] {
                let op = p.request.op;
                if self.cancel(op).is_err() {
                    std::process::abort();
                }
            }
        }
        let mut events = Vec::with_capacity(64);
        while self.ops.iter().any(Option::is_some) {
            if self.poll(None, &mut events).is_err() {
                std::process::abort();
            }
            events.clear();
            if self.event.is_some() {
                std::thread::yield_now();
            }
        }
        if let Some(event) = &mut self.event
            && event.shutdown().is_err()
        {
            std::process::abort();
        }
    }
}
fn read_buffer(p: &mut Pending, pool: &BufferPool) -> Option<(*mut u8, u32)> {
    let buf = match &p.request.operation {
        Operation::Read { buf, .. } | Operation::RecvFrom(buf) => buf,
        _ => unreachable!(),
    };
    match buf {
        ReadBuf::Provided(buf) => Some((buf.as_mut_ptr(), buf.len().min(u32::MAX as usize) as u32)),
        ReadBuf::Pooled => {
            if p.lease.is_none() {
                p.lease = Some(pool.acquire()?);
            }
            let bytes = p.lease.as_mut().expect("lease").writable();
            Some((
                bytes.as_mut_ptr(),
                bytes.len().min(u32::MAX as usize) as u32,
            ))
        }
    }
}
fn write_length(op: &Operation) -> usize {
    match op {
        Operation::Write(buf) | Operation::SendTo { buf, .. } => buf.as_slice().len(),
        Operation::Writev(bufs) => bufs.bufs.iter().flatten().map(|b| b.as_slice().len()).sum(),
        _ => 0,
    }
}
fn write_buffers(op: &Operation, mut offset: usize, out: &mut [WSABUF; MAX_IOV]) -> usize {
    let mut n = 0;
    let mut add = |buf: &WriteBuf| {
        let bytes = buf.as_slice();
        if offset >= bytes.len() {
            offset -= bytes.len();
            return;
        }
        let bytes = &bytes[offset..];
        offset = 0;
        out[n] = WSABUF {
            len: bytes.len().min(u32::MAX as usize) as u32,
            buf: bytes.as_ptr().cast_mut(),
        };
        n += 1;
    };
    match op {
        Operation::Write(buf) | Operation::SendTo { buf, .. } => add(buf),
        Operation::Writev(bufs) => {
            for buf in bufs.bufs.iter().flatten() {
                add(buf);
            }
        }
        _ => {}
    }
    n
}
