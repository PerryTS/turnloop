//! One worker per direction of an adopted synchronous handle. Requests retain their caller
//! buffers until the worker's completion packet; cancellation joins buffer access.
use super::{
    Detached, Kind, Native, bool_result, os_error, port::Port, process, signals, unsupported,
};
use crate::{Result, Signal};
use std::{
    os::windows::io::{AsRawHandle, OwnedHandle},
    ptr,
    sync::{Arc, Condvar, Mutex, OnceLock, mpsc},
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType, ReadFile, WriteFile},
    System::{
        Console::*,
        IO::{CancelSynchronousIo, IO_STATUS_BLOCK, OVERLAPPED, PostQueuedCompletionStatus},
    },
};
pub(super) const KEY: usize = 3;
#[derive(Clone, Copy)]
struct Job {
    pointer: usize,
    len: u32,
    write: bool,
    overlapped: usize,
}
struct State {
    job: Option<Job>,
    active: bool,
    stop: bool,
    cancelled: bool,
    result: Option<Result<u32>>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    port: Arc<Port>,
    handle: OwnedHandle,
    console: bool,
    thread_handle: Mutex<Option<OwnedHandle>>,
}
pub(super) struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    canceller: mpsc::Sender<Arc<Shared>>,
}

// CancelSynchronousIo only cancels I/O that has entered the kernel. A cancellation
// can race between the worker's active flag and ReadFile. One process-wide helper
// closes that interval without blocking a turn or creating a per-request thread.
// It sleeps on its queue while idle, then retries only until a cancelled worker
// has entered I/O or finished; no periodic timer or ordinary-I/O polling is used.
fn canceller() -> Result<mpsc::Sender<Arc<Shared>>> {
    static CANCELLER: OnceLock<Result<mpsc::Sender<Arc<Shared>>>> = OnceLock::new();
    CANCELLER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Arc<Shared>>();
            std::thread::Builder::new()
                .name("turnloop-stdio-cancel".into())
                .spawn(move || {
                    while let Ok(shared) = receiver.recv() {
                        loop {
                            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
                            if !state.active || !state.cancelled {
                                break;
                            }
                            let thread = shared
                                .thread_handle
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            let Some(thread) = thread.as_ref() else {
                                break;
                            };
                            // SAFETY: duplicated thread handle pins its identity. The state
                            // lock prevents this cancellation reaching a subsequent request.
                            let ok = unsafe { CancelSynchronousIo(thread.as_raw_handle()) };
                            if ok != 0 {
                                while state.active && state.cancelled {
                                    state = shared
                                        .changed
                                        .wait(state)
                                        .unwrap_or_else(|e| e.into_inner());
                                }
                                break;
                            }
                            if os_error().os != Some(ERROR_NOT_FOUND as i32) {
                                std::process::abort();
                            }
                            drop(state);
                            std::thread::yield_now();
                        }
                    }
                })?;
            Ok(sender)
        })
        .clone()
}
impl Worker {
    pub(super) fn new(handle: HANDLE, console: bool, port: Arc<Port>) -> Result<Self> {
        let canceller = canceller()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                job: None,
                active: false,
                stop: false,
                cancelled: false,
                result: None,
            }),
            changed: Condvar::new(),
            port,
            handle: process::duplicate(handle, false)?,
            console,
            thread_handle: Mutex::new(None),
        });
        let run = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("turnloop-stdio".into())
            .spawn(move || pump(run))?;
        let worker = Self {
            shared,
            thread: Some(thread),
            canceller,
        };
        let duplicate = process::duplicate(
            worker.thread.as_ref().expect("worker").as_raw_handle(),
            false,
        )?;
        *worker
            .shared
            .thread_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(duplicate);
        Ok(worker)
    }
    pub(super) fn start(
        &self,
        pointer: *mut u8,
        len: u32,
        write: bool,
        overlapped: *mut OVERLAPPED,
    ) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        assert!(state.job.is_none() && !state.active && state.result.is_none());
        state.cancelled = false;
        state.job = Some(Job {
            pointer: pointer as usize,
            len,
            write,
            overlapped: overlapped as usize,
        });
        self.shared.changed.notify_one();
    }
    pub(super) fn cancel(&self) {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled = true;
        if state.active {
            // SAFETY: owned thread is live until Drop joins. Cancellation does not
            // acknowledge caller buffers; only finish after its packet does that.
            unsafe {
                CancelSynchronousIo(self.thread.as_ref().expect("worker").as_raw_handle());
            }
            if self.canceller.send(Arc::clone(&self.shared)).is_err() {
                std::process::abort();
            }
        }
        self.shared.changed.notify_one();
    }
    pub(super) fn finish(&self) -> Result<u32> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .result
            .take()
            .expect("worker completion")
    }
}
fn pump(shared: Arc<Shared>) {
    let mut console = ConsoleBytes::default();
    loop {
        let (job, cancelled) = {
            let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
            while state.job.is_none() && !state.stop {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
            if state.stop {
                return;
            }
            let job = state.job.take().expect("job");
            state.active = true;
            (job, state.cancelled)
        };
        let result = if cancelled {
            Err(super::socket::error(ERROR_OPERATION_ABORTED as i32))
        } else {
            // Classification belongs to quiescent adoption. GetConsoleMode is
            // itself I/O, and on a synchronous pipe can wait behind the other
            // worker's idle ReadFile before this worker even reaches WriteFile.
            // Console identity is immutable; tty_set_mode only changes its flags.
            if shared.console && !job.write {
                console.read(
                    shared.handle.as_raw_handle(),
                    job.pointer as *mut u8,
                    job.len,
                )
            } else {
                let mut n = 0;
                // SAFETY: backend retains exclusive read/immutable write memory until
                // the packet below is consumed. Handle is synchronous and owned.
                let ok = unsafe {
                    if job.write {
                        WriteFile(
                            shared.handle.as_raw_handle(),
                            job.pointer as *const u8,
                            job.len,
                            &mut n,
                            ptr::null_mut(),
                        )
                    } else {
                        ReadFile(
                            shared.handle.as_raw_handle(),
                            job.pointer as *mut u8,
                            job.len,
                            &mut n,
                            ptr::null_mut(),
                        )
                    }
                };
                if ok == 0 { Err(os_error()) } else { Ok(n) }
            }
        };
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active = false;
        state.result = Some(result);
        // SAFETY: retained port and opaque stable kernel slot. Publication happens
        // under the same lock used by finish, after all native buffer access ends.
        if unsafe {
            PostQueuedCompletionStatus(
                shared.port.raw(),
                0,
                KEY,
                job.overlapped as *const OVERLAPPED,
            )
        } == 0
        {
            std::process::abort();
        }
        shared.changed.notify_all();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Backend Drop first waits for completion acknowledgements. There is no
        // active request here, including after normal close and quiescent detach.
        {
            let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            assert!(!state.active && state.job.is_none());
            state.stop = true;
            self.shared.changed.notify_one();
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            std::process::abort();
        }
    }
}
#[derive(Default)]
struct ConsoleBytes {
    bytes: [u8; 4],
    len: usize,
    offset: usize,
    repeats: u16,
    high: Option<u16>,
}
impl ConsoleBytes {
    fn read(&mut self, handle: HANDLE, pointer: *mut u8, len: u32) -> Result<u32> {
        if len == 0 {
            return Ok(0);
        }
        loop {
            if self.repeats > 0 {
                let n = (self.len - self.offset).min(len as usize);
                // SAFETY: worker retains exclusive caller buffer; source range initialized.
                unsafe {
                    ptr::copy_nonoverlapping(self.bytes.as_ptr().add(self.offset), pointer, n);
                }
                self.offset += n;
                if self.offset == self.len {
                    self.offset = 0;
                    self.repeats -= 1;
                }
                return Ok(n as u32);
            }
            // SAFETY: plain C console input output record.
            let mut record: INPUT_RECORD = unsafe { std::mem::zeroed() };
            let mut read = 0;
            // SAFETY: owned console and one writable input record; synchronous worker only.
            bool_result(unsafe { ReadConsoleInputW(handle, &mut record, 1, &mut read) })?;
            if read == 0 {
                continue;
            }
            if record.EventType == WINDOW_BUFFER_SIZE_EVENT as u16 {
                signals::dispatch(Signal::WinCh);
                continue;
            }
            if record.EventType != KEY_EVENT as u16 {
                continue;
            }
            // SAFETY: EventType selected the initialized key-event union arm.
            let key = unsafe { record.Event.KeyEvent };
            if key.bKeyDown == 0 {
                continue;
            }
            // SAFETY: ReadConsoleInputW initializes the Unicode member.
            let unit = unsafe { key.uChar.UnicodeChar };
            if unit == 0 {
                continue;
            }
            if (0xd800..=0xdbff).contains(&unit) {
                self.high = Some(unit);
                continue;
            }
            let code = if let Some(high) = self.high.take() {
                if (0xdc00..=0xdfff).contains(&unit) {
                    0x10000 + ((high as u32 - 0xd800) << 10) + (unit as u32 - 0xdc00)
                } else {
                    unit as u32
                }
            } else {
                unit as u32
            };
            let ch = char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER);
            self.len = ch.encode_utf8(&mut self.bytes).len();
            self.offset = 0;
            self.repeats = key.wRepeatCount.max(1);
        }
    }
}
impl Detached {
    /// Adopt a quiescent file, pipe or console handle. Synchronous handles retain
    /// their console classification and use per-direction workers on attachment.
    /// Arbitrary character devices retain their driver's I/O serialization and
    /// cancellation semantics. Overlapped pipes use event-routed I/O even with a prior IOCP
    /// association. Other overlapped handles return Unsupported. Captured console
    /// modes are restored on close or drop.
    pub fn from_handle(handle: OwnedHandle) -> Result<Self> {
        let mut mode = 0;
        // SAFETY: owned handle, writable mode output.
        let console = unsafe { GetConsoleMode(handle.as_raw_handle(), &mut mode) } != 0;
        // SAFETY: valid zeroed C output storage for screen-buffer classification.
        let mut info = unsafe { std::mem::zeroed() };
        // SAFETY: owned handle; failure distinguishes input from screen-buffer handles.
        let console_input = console
            && unsafe { GetConsoleScreenBufferInfo(handle.as_raw_handle(), &mut info) } == 0;
        let kind = if console {
            Kind::Sync
        } else {
            handle_kind(handle.as_raw_handle())?
        };
        let mut transport = Self::new(Native::Handle(handle), kind, kind == Kind::Pipe);
        transport.mode = console.then_some(mode);
        transport.console_input = console_input;
        Ok(transport)
    }
}

fn handle_kind(handle: HANDLE) -> Result<Kind> {
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_MODE_INFORMATION, FILE_SYNCHRONOUS_IO_ALERT, FILE_SYNCHRONOUS_IO_NONALERT,
        FileModeInformation, NtQueryInformationFile,
    };
    // SAFETY: valid zeroed C output storage; FileModeInformation completes inline
    // and does not initiate data I/O or retain either output pointer.
    let mut status: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    let mut info = FILE_MODE_INFORMATION { Mode: 0 };
    // SAFETY: live owned handle and correctly sized writable mode information.
    // https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntqueryinformationfile
    let result = unsafe {
        NtQueryInformationFile(
            handle,
            &mut status,
            ptr::from_mut(&mut info).cast(),
            std::mem::size_of_val(&info) as u32,
            FileModeInformation,
        )
    };
    if result != 0 {
        return Err(unsupported());
    }
    if info.Mode & (FILE_SYNCHRONOUS_IO_ALERT | FILE_SYNCHRONOUS_IO_NONALERT) != 0 {
        Ok(Kind::Sync)
    // SAFETY: live owned handle; classification has no lifetime side effects.
    } else if unsafe { GetFileType(handle) } == FILE_TYPE_PIPE {
        Ok(Kind::Pipe)
    } else {
        // Never submit synchronous ReadFile with null OVERLAPPED on an unknown
        // asynchronous handle: a pending result would outlive worker buffers.
        Err(unsupported())
    }
}
