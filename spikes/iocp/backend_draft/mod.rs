//! Compiled draft of the Windows Backend boundary, pending core trait-v0.
//! See README.md: these types are internal backend events, not a replacement Loop.
use crate::{
    integration::EventIntegration,
    port::{Entry, Port, WAKE, Wait, bool_result},
    tcp::Winsock,
    timer::ApcTimer,
};
use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    io,
    marker::PhantomData,
    os::windows::io::{AsRawHandle, AsRawSocket, OwnedHandle, OwnedSocket},
    ptr,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_IO_PENDING, ERROR_NOT_FOUND, ERROR_PIPE_CONNECTED, HANDLE, RtlNtStatusToDosError,
    },
    Networking::WinSock::*,
    Storage::FileSystem::{ReadFile, SetFileCompletionNotificationModes, WriteFile},
    System::{
        IO::{CancelIoEx, OVERLAPPED},
        Pipes::ConnectNamedPipe,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Token(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    index: usize,
    generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpId {
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultKind {
    Read(u32),
    Wrote(u32),
    ReadReady,
    Connected,
    Cancelled,
    #[default]
    Closed,
    Error(u32),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub token: Token,
    pub result: ResultKind,
    pub op: Option<OpId>,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnInfo {
    pub completions: usize,
    pub waits: u32,
    pub notified: bool,
    pub alive: bool,
}

pub enum Resource {
    Socket(OwnedSocket),
    Pipe(OwnedHandle),
}
impl Resource {
    fn raw(&self) -> HANDLE {
        match self {
            Self::Socket(s) => s.as_raw_socket() as HANDLE,
            Self::Pipe(p) => p.as_raw_handle(),
        }
    }
    fn is_socket(&self) -> bool {
        matches!(self, Self::Socket(_))
    }
}
struct HandleSlot {
    generation: u64,
    resource: Option<Resource>,
    operations: usize,
    closing: Option<Token>,
}
#[derive(Clone, Copy)]
pub enum Request {
    Read {
        buffer: *mut u8,
        len: u32,
    },
    Write {
        buffer: *const u8,
        len: u32,
    },
    /// Internal D3 stage: core obtains a pooled buffer and calls nonblocking recv.
    IdleRead,
    PipeConnect,
}
#[derive(Clone, Copy, PartialEq)]
enum State {
    Free,
    Pending,
    Ready,
}
struct OpSlot {
    generation: u64,
    state: State,
    handle: usize,
    token: Token,
    request: Request,
    result: ResultKind,
    cancelled: bool,
}

const RUNNING: u8 = 0;
const PARKED: u8 = 1;
const NOTIFIED: u8 = 2;
struct Wake {
    state: AtomicU8,
    calls: AtomicU64,
    port: Arc<Port>,
}
#[derive(Clone)]
pub struct Notifier(Arc<Wake>);
impl Notifier {
    pub fn notify(&self) -> io::Result<()> {
        if self.0.state.swap(NOTIFIED, Ordering::AcqRel) == PARKED {
            self.0.calls.fetch_add(1, Ordering::Relaxed);
            self.0.port.post(WAKE, 0)?;
        }
        Ok(())
    }
    pub fn syscall_count(&self) -> u64 {
        self.0.calls.load(Ordering::Relaxed)
    }
    pub fn is_parked(&self) -> bool {
        self.0.state.load(Ordering::Acquire) == PARKED
    }
}

pub struct IocpBackend {
    // Slots never move or resize; kernel only sees addresses inside these boxes.
    ops: Box<[OpSlot]>,
    // Separate kernel storage: never create &mut references to OS-written memory.
    kernel: Box<[UnsafeCell<OVERLAPPED>]>,
    handles: Box<[HandleSlot]>,
    ready: VecDeque<usize>,
    port: Arc<Port>,
    timer: ApcTimer,
    wake: Arc<Wake>,
    event: Option<EventIntegration>,
    pending: usize,
    _bound: PhantomData<Rc<()>>,
    _winsock: Winsock,
}

impl IocpBackend {
    pub fn new(handle_capacity: usize, operation_capacity: usize) -> io::Result<Self> {
        if handle_capacity == 0 || operation_capacity == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "zero capacity"));
        }
        let winsock = Winsock::new()?;
        let port = Arc::new(Port::new()?);
        let kernel = (0..operation_capacity)
            .map(|_| {
                // SAFETY: valid inactive OVERLAPPED, before any submission.
                UnsafeCell::new(unsafe { std::mem::zeroed() })
            })
            .collect();
        let ops = (0..operation_capacity)
            .map(|_| OpSlot {
                generation: 0,
                state: State::Free,
                handle: 0,
                token: Token(0),
                request: Request::IdleRead,
                result: ResultKind::Closed,
                cancelled: false,
            })
            .collect();
        let handles = (0..handle_capacity)
            .map(|_| HandleSlot {
                generation: 0,
                resource: None,
                operations: 0,
                closing: None,
            })
            .collect();
        let wake = Arc::new(Wake {
            state: AtomicU8::new(RUNNING),
            calls: AtomicU64::new(0),
            port: Arc::clone(&port),
        });
        Ok(Self {
            ops,
            kernel,
            handles,
            ready: VecDeque::with_capacity(operation_capacity),
            port,
            timer: ApcTimer::new()?,
            wake,
            event: None,
            pending: 0,
            _bound: PhantomData,
            _winsock: winsock,
        })
    }
    pub fn notifier(&self) -> Notifier {
        Notifier(Arc::clone(&self.wake))
    }

    /// # Safety
    /// Resource is overlapped and not already associated with an IOCP. Socket
    /// providers must return IFS handles. Caller has no outstanding I/O on it.
    pub unsafe fn register(&mut self, resource: Resource) -> io::Result<Handle> {
        let index = self
            .handles
            .iter()
            .position(|h| h.resource.is_none())
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "handle capacity"))?;
        if resource.is_socket() {
            let mut info = WSAPROTOCOL_INFOW::default();
            let mut n = std::mem::size_of_val(&info) as i32;
            // SAFETY: correct socket option output and live owned socket.
            if unsafe {
                getsockopt(
                    resource.raw() as usize,
                    SOL_SOCKET,
                    SO_PROTOCOL_INFOW,
                    (&mut info as *mut WSAPROTOCOL_INFOW).cast(),
                    &mut n,
                )
            } == SOCKET_ERROR
            {
                return Err(wsa_error());
            }
            if info.dwServiceFlags1 & XP1_IFS_HANDLES == 0 {
                return Err(io::Error::new(io::ErrorKind::Unsupported, "non-IFS socket"));
            }
            let mut nonblocking = 1;
            // SAFETY: FIONBIO input is u32; prevents the post-zero-read recv blocking.
            if unsafe { ioctlsocket(resource.raw() as usize, FIONBIO, &mut nonblocking) }
                == SOCKET_ERROR
            {
                return Err(wsa_error());
            }
        }
        // SAFETY: caller guarantees fresh overlapped resource, held throughout call.
        unsafe { self.port.associate(resource.raw(), index + 1) }?;
        // SAFETY: live overlapped IFS handle; state is recorded only on success.
        bool_result(unsafe { SetFileCompletionNotificationModes(resource.raw(), 1) })?;
        let slot = &mut self.handles[index];
        slot.generation = slot
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("handle generation exhausted"))?;
        slot.resource = Some(resource);
        slot.closing = None;
        Ok(Handle {
            index,
            generation: slot.generation,
        })
    }
    fn handle_index(&self, handle: Handle) -> io::Result<usize> {
        match self.handles.get(handle.index) {
            Some(h) if h.generation == handle.generation && h.resource.is_some() => {
                Ok(handle.index)
            }
            _ => Err(io::Error::new(io::ErrorKind::NotFound, "stale handle")),
        }
    }

    /// # Safety
    /// Read buffers must remain writable and exclusively borrowed, write buffers
    /// immutable and readable, until this op's completion is delivered or the
    /// backend is dropped (Drop drains). Lengths must describe valid memory.
    pub unsafe fn submit(
        &mut self,
        handle: Handle,
        token: Token,
        request: Request,
    ) -> io::Result<OpId> {
        let h = self.handle_index(handle)?;
        if self.handles[h].closing.is_some() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "handle closing"));
        }
        let resource = self.handles[h]
            .resource
            .as_ref()
            .ok_or_else(|| io::Error::other("missing resource"))?;
        if (matches!(request, Request::IdleRead) && !resource.is_socket())
            || (matches!(request, Request::PipeConnect) && resource.is_socket())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "operation/resource mismatch",
            ));
        }
        let i = self
            .ops
            .iter()
            .position(|op| op.state == State::Free)
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "operation capacity"))?;
        let op = &mut self.ops[i];
        op.generation = op
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("operation generation exhausted"))?;
        // SAFETY: free slot; no kernel packet can still reference it.
        unsafe {
            *self.kernel[i].get() = std::mem::zeroed();
        }
        op.handle = h;
        op.token = token;
        op.request = request;
        op.cancelled = false;
        let mut bytes = 0;
        let mut flags = 0;
        let raw = resource.raw();
        let socket = resource.is_socket();
        // SAFETY: caller's buffer contract plus stable op slab, valid owned handle.
        // Stack WSABUF is captured during Winsock submission; no callback requested.
        let success = unsafe {
            match request {
                Request::Read { buffer, len } if socket => {
                    WSARecv(
                        raw as usize,
                        &WSABUF { len, buf: buffer },
                        1,
                        &mut bytes,
                        &mut flags,
                        self.kernel[i].get(),
                        None,
                    ) == 0
                }
                Request::Write { buffer, len } if socket => {
                    WSASend(
                        raw as usize,
                        &WSABUF {
                            len,
                            buf: buffer.cast_mut(),
                        },
                        1,
                        &mut bytes,
                        0,
                        self.kernel[i].get(),
                        None,
                    ) == 0
                }
                Request::IdleRead => {
                    WSARecv(
                        raw as usize,
                        &WSABUF {
                            len: 0,
                            buf: ptr::NonNull::<u8>::dangling().as_ptr(),
                        },
                        1,
                        &mut bytes,
                        &mut flags,
                        self.kernel[i].get(),
                        None,
                    ) == 0
                }
                Request::Read { buffer, len } => {
                    ReadFile(raw, buffer, len, &mut bytes, self.kernel[i].get()) != 0
                }
                Request::Write { buffer, len } => {
                    WriteFile(raw, buffer, len, &mut bytes, self.kernel[i].get()) != 0
                }
                Request::PipeConnect => ConnectNamedPipe(raw, self.kernel[i].get()) != 0,
            }
        };
        let error = if success {
            0
        } else if socket {
            wsa_error().raw_os_error().unwrap_or(1) as u32
        } else {
            io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32
        };
        let connected = matches!(request, Request::PipeConnect) && error == ERROR_PIPE_CONNECTED;
        if success || connected {
            op.state = State::Ready;
            op.result = success_result(request, bytes);
            self.ready.push_back(i);
        } else if error == ERROR_IO_PENDING {
            op.state = State::Pending;
            self.pending += 1;
        } else {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        self.handles[h].operations += 1;
        Ok(OpId {
            index: i,
            generation: op.generation,
        })
    }

    pub fn cancel(&mut self, id: OpId) -> io::Result<bool> {
        let Some(op) = self.ops.get_mut(id.index) else {
            return Ok(false);
        };
        if op.generation != id.generation || op.state == State::Free || op.cancelled {
            return Ok(false);
        }
        if op.state == State::Pending {
            let resource = self.handles[op.handle]
                .resource
                .as_ref()
                .ok_or_else(|| io::Error::other("missing handle"))?;
            // SAFETY: stable pending op and resource; cancellation is only a request.
            // https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex
            if unsafe { CancelIoEx(resource.raw(), self.kernel[id.index].get()) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                    return Err(error);
                }
            }
        }
        op.cancelled = true;
        if op.state == State::Ready {
            op.result = ResultKind::Cancelled;
        }
        Ok(true)
    }
    pub fn close(&mut self, handle: Handle, token: Token) -> io::Result<()> {
        let h = self.handle_index(handle)?;
        if self.handles[h].closing.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "already closing",
            ));
        }
        self.handles[h].closing = Some(token);
        for i in 0..self.ops.len() {
            let op = &self.ops[i];
            if op.state != State::Free && op.handle == h {
                let id = OpId {
                    index: i,
                    generation: op.generation,
                };
                self.cancel(id)?;
            }
        }
        Ok(())
    }

    /// Opt-in helper. In event mode turn accepts Now only; host composes its own
    /// deadline wait or uses PacketTimer as demonstrated by the integration probe.
    pub fn integration(&mut self) -> io::Result<HANDLE> {
        if self.event.is_none() {
            self.event = Some(EventIntegration::new(Arc::clone(&self.port))?);
            let notified = self.wake.state.swap(PARKED, Ordering::AcqRel) == NOTIFIED;
            if notified || !self.ready.is_empty() {
                self.port.post(WAKE, 0)?;
            }
        }
        Ok(self
            .event
            .as_ref()
            .ok_or_else(|| io::Error::other("missing helper"))?
            .event())
    }

    /// `timeout` is already min(host budget, core timer deadline). Zero means Now.
    pub fn turn(
        &mut self,
        timeout: Option<Duration>,
        out: &mut [Completion],
    ) -> io::Result<TurnInfo> {
        if out.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty output"));
        }
        let mut info = TurnInfo::default();
        if self.event.is_some() && timeout != Some(Duration::ZERO) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "event integration requires turn(Now)",
            ));
        }
        let queued = !self.ready.is_empty()
            || self
                .handles
                .iter()
                .any(|h| h.closing.is_some() && h.operations == 0);
        let mut entries = [Entry::default(); 64];
        let n = if queued {
            0
        } else if let Some(event) = &mut self.event {
            // Helper is the sole port consumer. Core notify must wake the event too;
            // keep logical state PARKED while the GUI host owns its external wait.
            event.drain(&mut entries)?
        } else {
            let state = self.wake.state.compare_exchange(
                RUNNING,
                PARKED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            if state == Err(NOTIFIED) {
                info.notified = true;
                0
            } else {
                if let Some(delay) = timeout.filter(|d| !d.is_zero())
                    && let Err(error) = self.timer.arm(delay)
                {
                    self.wake.state.store(RUNNING, Ordering::Release);
                    return Err(error);
                }
                info.waits = 1;
                let result = self.port.wait(timeout, true, &mut entries);
                let notified = self.wake.state.swap(RUNNING, Ordering::AcqRel) == NOTIFIED;
                info.notified |= notified;
                self.timer.cancel()?;
                match result? {
                    Wait::Entries(n) => n,
                    Wait::Timeout | Wait::Apc => 0,
                }
            }
        };
        for entry in &entries[..n] {
            if entry.key == WAKE {
                info.notified = true;
                continue;
            }
            let Some(i) = self
                .kernel
                .iter()
                .position(|kernel| kernel.get() as usize == entry.overlapped)
            else {
                return Err(io::Error::other("unknown OVERLAPPED"));
            };
            let op = &mut self.ops[i];
            if op.state != State::Pending || entry.key != op.handle + 1 {
                return Err(io::Error::other("stale or duplicate completion"));
            }
            self.pending -= 1;
            op.state = State::Ready;
            op.result = if op.cancelled {
                ResultKind::Cancelled
            } else if entry.status < 0 {
                // SAFETY: pure NTSTATUS -> Win32 mapping, no pointers.
                // https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-rtlntstatustodoserror
                ResultKind::Error(unsafe { RtlNtStatusToDosError(entry.status) })
            } else {
                success_result(op.request, entry.bytes)
            };
            self.ready.push_back(i);
        }
        while info.completions < out.len() {
            let Some(i) = self.ready.pop_front() else {
                break;
            };
            let op = &mut self.ops[i];
            out[info.completions] = Completion {
                token: op.token,
                result: op.result,
                op: Some(OpId {
                    index: i,
                    generation: op.generation,
                }),
            };
            info.completions += 1;
            self.handles[op.handle].operations -= 1;
            op.state = State::Free;
        }
        for h in &mut self.handles {
            if info.completions == out.len() {
                break;
            }
            if h.operations == 0
                && let Some(token) = h.closing.take()
            {
                out[info.completions] = Completion {
                    token,
                    result: ResultKind::Closed,
                    op: None,
                };
                info.completions += 1;
                h.resource.take(); // only after all op completions were emitted
            }
        }
        info.alive = self.handles.iter().any(|h| h.resource.is_some());
        if self.event.is_some() {
            // Preserve notifications racing with host return; already-notified means
            // signal now, otherwise producer's exchange sees PARKED and posts.
            if self.wake.state.swap(PARKED, Ordering::AcqRel) == NOTIFIED {
                self.port.post(WAKE, 0)?;
                info.notified = true;
            }
            if !self.ready.is_empty()
                || self
                    .handles
                    .iter()
                    .any(|h| h.closing.is_some() && h.operations == 0)
            {
                self.port.post(WAKE, 0)?;
            }
        } else {
            info.notified |= self.wake.state.swap(RUNNING, Ordering::AcqRel) == NOTIFIED;
        }
        Ok(info)
    }
}

impl Drop for IocpBackend {
    fn drop(&mut self) {
        // Cancel + drain is mandatory even on early exit. Teardown can block and
        // is outside D7's bounded turn. No driver-owned thread in direct mode.
        for (i, op) in self.ops.iter().enumerate() {
            if op.state == State::Pending
                && let Some(resource) = &self.handles[op.handle].resource
            {
                // SAFETY: storage/handle retained until packet drain below.
                unsafe {
                    CancelIoEx(resource.raw(), self.kernel[i].get());
                }
            }
        }
        let mut out = [Completion::default(); 64];
        while self.pending > 0 || !self.ready.is_empty() {
            let timeout = if self.event.is_some() {
                Some(Duration::ZERO)
            } else {
                None
            };
            if self.turn(timeout, &mut out).is_err() {
                // Cannot establish completion after a broken port: retain storage
                // and handles rather than expose caller memory to a freed OVERLAPPED.
                // This is a fatal invariant breach: returning could also release
                // the caller's buffers. Abort is safer than pretending Drop drained.
                std::process::abort();
            }
            if self.event.is_some() {
                std::thread::yield_now();
            }
        }
        if let Some(event) = &mut self.event {
            let _ = event.shutdown();
        }
    }
}

fn success_result(request: Request, bytes: u32) -> ResultKind {
    match request {
        Request::Read { .. } => ResultKind::Read(bytes),
        Request::Write { .. } => ResultKind::Wrote(bytes),
        Request::IdleRead => ResultKind::ReadReady,
        Request::PipeConnect => ResultKind::Connected,
    }
}
fn wsa_error() -> io::Error {
    // SAFETY: thread-local error query.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}
