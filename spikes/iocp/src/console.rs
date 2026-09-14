//! Process-global dispatcher probe. Run only in the dedicated console_child binary.
use crate::port::{Entry, Port, Wait, bool_result};
use std::{
    io, ptr,
    sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{GetLastError, HANDLE},
    System::{Console::*, IO::PostQueuedCompletionStatus},
};

static PORT: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(ptr::null_mut());
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static ERROR: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Signal {
    Int = 1,
    Break = 2,
    Hup = 3,
}
pub fn map(control: u32) -> Option<Signal> {
    match control {
        CTRL_C_EVENT => Some(Signal::Int),
        CTRL_BREAK_EVENT => Some(Signal::Break),
        CTRL_CLOSE_EVENT => Some(Signal::Hup),
        _ => None,
    }
}

unsafe extern "system" fn handler(control: u32) -> i32 {
    let Some(signal) = map(control) else {
        return 0;
    };
    // SeqCst orders callback entry/load with teardown's null-store/active-load.
    // Callback may start after unregistration, but then sees null and uses no handle.
    ACTIVE.fetch_add(1, Ordering::SeqCst);
    let port: HANDLE = PORT.load(Ordering::SeqCst);
    let handled = if port.is_null() {
        0
    } else {
        // SAFETY: teardown keeps port alive until all callbacks that could load it
        // have left. No locks, allocations, panics, or host callback execution.
        let ok = unsafe { PostQueuedCompletionStatus(port, signal as u32, 50, ptr::null()) };
        if ok == 0 {
            // SAFETY: thread-local error retrieval immediately after failed post.
            ERROR.store(unsafe { GetLastError() }, Ordering::Release);
        }
        1
    };
    ACTIVE.fetch_sub(1, Ordering::SeqCst);
    handled
}

struct Registration;
impl Drop for Registration {
    fn drop(&mut self) {
        PORT.store(ptr::null_mut(), Ordering::SeqCst);
        // SAFETY: removes exactly this installed function pointer. Unlike registered
        // waits, this API does not document joining active handler threads.
        // https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 0);
        }
        while ACTIVE.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }
}

/// # Safety
/// This process must have its own isolated console (CREATE_NEW_CONSOLE), with no
/// unrelated processes attached. This probe generates real CTRL_C / CTRL_BREAK.
pub unsafe fn isolated_probe() -> io::Result<u32> {
    let port = Port::new()?;
    if PORT
        .compare_exchange(
            ptr::null_mut(),
            port.raw(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "console dispatcher already installed",
        ));
    }
    ERROR.store(0, Ordering::Release);
    // SAFETY: stable function pointer. Handler installed before enabling CTRL_C.
    if let Err(error) = bool_result(unsafe { SetConsoleCtrlHandler(Some(handler), 1) }) {
        PORT.store(ptr::null_mut(), Ordering::SeqCst);
        return Err(error);
    }
    let _registration = Registration;
    // SAFETY: caller granted isolated-console test; clear inherited ignore-CTRL_C.
    bool_result(unsafe { SetConsoleCtrlHandler(None, 0) })?;
    let mut received = 0;
    for (control, expected) in [
        (CTRL_C_EVENT, Signal::Int),
        (CTRL_BREAK_EVENT, Signal::Break),
    ] {
        // SAFETY: caller guarantees isolated console. Group zero broadcasts only
        // to this console; CTRL_C cannot be restricted to a nonzero process group.
        // https://learn.microsoft.com/en-us/windows/console/generateconsolectrlevent
        bool_result(unsafe { GenerateConsoleCtrlEvent(control, 0) })?;
        let mut entries = [Entry::default(); 4];
        assert_eq!(
            port.wait(Some(Duration::from_secs(3)), false, &mut entries)?,
            Wait::Entries(1)
        );
        assert_eq!((entries[0].key, entries[0].bytes), (50, expected as u32));
        received += 1;
    }
    assert_eq!(ERROR.load(Ordering::Acquire), 0);
    // CTRL_CLOSE is a best-effort HUP notification, not an ordinary signal whose
    // handler can return and expect future turn() calls: Windows terminates the
    // process after its handler returns or times out. SIGTERM has no mapping.
    // https://learn.microsoft.com/en-us/windows/console/handlerroutine
    assert_eq!(map(CTRL_CLOSE_EVENT), Some(Signal::Hup));
    assert_eq!(map(CTRL_LOGOFF_EVENT), None);
    Ok(received)
}
