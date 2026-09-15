use super::{CHANGE, OVERFLOW, RENAME, State};
use crate::*;
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{IO::*, Threading::*},
};
fn error() -> Error {
    std::io::Error::last_os_error().into()
}
pub(super) struct Watch {
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<()>>,
}
fn event() -> Result<OwnedHandle> {
    // SAFETY: unnamed manual-reset event with default security.
    let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if h.is_null() {
        return Err(error());
    }
    // SAFETY: fresh owned event handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}
impl Watch {
    pub fn new(path: &FsPath, recursive: bool, state: Arc<State>) -> Result<Self> {
        if path.preopen.is_some() {
            return Err(Error::new(ErrorKind::Unsupported));
        }
        // SAFETY: prepared wide path; overlapped directory handle exclusively owned below.
        let h = unsafe {
            CreateFileW(
                path.native.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(error());
        }
        // SAFETY: uniquely owned result of CreateFileW.
        let directory = unsafe { OwnedHandle::from_raw_handle(h) };
        let stop = Arc::new(event()?);
        let completion = event()?;
        let cancel = stop.clone();
        let ready = Arc::new((Mutex::new(None), Condvar::new()));
        let started = ready.clone();
        let thread = std::thread::Builder::new()
            .name("turnloop-watch".into())
            .spawn(move || {
                let mut buffer = [0u32; 2048];
                // SAFETY: zero is the documented initial OVERLAPPED state; event set next.
                let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                overlapped.hEvent = completion.as_raw_handle();
                let mut first = true;
                loop {
                    // SAFETY: live manual-reset event owned by this worker.
                    unsafe { ResetEvent(completion.as_raw_handle()) };
                    // SAFETY: pinned worker stack output and OVERLAPPED remain live until
                    // GetOverlappedResult acknowledges success or cancellation below.
                    let ok = unsafe {
                        ReadDirectoryChangesW(
                            directory.as_raw_handle(),
                            buffer.as_mut_ptr().cast(),
                            std::mem::size_of_val(&buffer) as u32,
                            i32::from(recursive),
                            FILE_NOTIFY_CHANGE_FILE_NAME
                                | FILE_NOTIFY_CHANGE_DIR_NAME
                                | FILE_NOTIFY_CHANGE_ATTRIBUTES
                                | FILE_NOTIFY_CHANGE_SIZE
                                | FILE_NOTIFY_CHANGE_LAST_WRITE
                                | FILE_NOTIFY_CHANGE_CREATION,
                            std::ptr::null_mut(),
                            &mut overlapped,
                            None,
                        )
                    };
                    let failure = if ok == 0 { Some(error()) } else { None };
                    if first {
                        let mut r = started
                            .0
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        *r = Some(failure.map_or(Ok(()), Err));
                        started.1.notify_one();
                        first = false;
                    }
                    if failure.is_some() {
                        state.event(OVERFLOW);
                        break;
                    }
                    let handles = [cancel.as_raw_handle(), completion.as_raw_handle()];
                    // SAFETY: two live events; block until native completion or explicit cancellation.
                    let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    if wait != WAIT_OBJECT_0 + 1 {
                        // SAFETY: cancel this exact in-flight operation, then acknowledge it.
                        unsafe { CancelIoEx(directory.as_raw_handle(), &overlapped) };
                        let mut n = 0;
                        // SAFETY: output/OVERLAPPED remain live until this wait completes.
                        unsafe {
                            GetOverlappedResult(directory.as_raw_handle(), &overlapped, &mut n, 1)
                        };
                        if wait != WAIT_OBJECT_0 {
                            state.event(OVERFLOW);
                        }
                        break;
                    }
                    let mut n = 0;
                    // SAFETY: event signaled; collect exact operation before buffer reuse.
                    let ok = unsafe {
                        GetOverlappedResult(directory.as_raw_handle(), &overlapped, &mut n, 0)
                    };
                    if ok == 0 {
                        state.event(OVERFLOW);
                        break;
                    }
                    if n == 0 {
                        state.event(OVERFLOW);
                        continue;
                    }
                    // SAFETY: initialized byte length returned by native API within supplied buffer.
                    let bytes = unsafe {
                        std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), n as usize)
                    };
                    let mut at = 0;
                    let mut flags = 0;
                    while at + 12 <= bytes.len() {
                        let action =
                            u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("action"));
                        flags |= if action == FILE_ACTION_MODIFIED {
                            CHANGE
                        } else {
                            RENAME
                        };
                        let next = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("offset"))
                            as usize;
                        if next == 0 {
                            break;
                        }
                        if next < 12 || at + next > bytes.len() {
                            flags |= OVERFLOW;
                            break;
                        }
                        at += next;
                    }
                    state.event(flags);
                }
                state.stopped();
            })
            .map_err(Error::from)?;
        let watch = Self {
            stop,
            thread: Some(thread),
        };
        let mut ready_state = ready
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while ready_state.is_none() {
            ready_state = ready
                .1
                .wait(ready_state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        ready_state.take().expect("watch startup acknowledged")?;
        Ok(watch)
    }
    pub fn cancel(&mut self) -> Result<()> {
        // SAFETY: live cancellation event; worker acknowledges pending native I/O.
        if unsafe { SetEvent(self.stop.as_raw_handle()) } == 0 {
            return Err(error());
        }
        Ok(())
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
