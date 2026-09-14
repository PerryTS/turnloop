use super::{bool_result, unsupported};
use crate::{Error, ErrorKind, Notifier, Result, Signal};
use std::{
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
};
use windows_sys::Win32::System::Console::*;

static SLOTS: [AtomicPtr<Ticket>; 1024] = [const { AtomicPtr::new(ptr::null_mut()) }; 1024];
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static REGISTRATION: Mutex<usize> = Mutex::new(0);
struct Ticket {
    signal: Signal,
    pending: AtomicBool,
    notifier: Notifier,
}
pub(super) struct Subscription {
    ticket: Box<Ticket>,
    index: usize,
}

pub(super) fn dispatch(signal: Signal) -> bool {
    ACTIVE.fetch_add(1, Ordering::SeqCst);
    let mut handled = false;
    for slot in &SLOTS {
        let pointer = slot.load(Ordering::SeqCst);
        if pointer.is_null() {
            continue;
        }
        // SAFETY: removal nulls the slot before waiting for every active dispatcher;
        // the Box and its notifier remain live through this entire access.
        let ticket = unsafe { &*pointer };
        if ticket.signal == signal {
            handled = true;
            ticket.pending.store(true, Ordering::Release);
            let _ = ticket.notifier.notify();
        }
    }
    ACTIVE.fetch_sub(1, Ordering::SeqCst);
    handled
}
unsafe extern "system" fn handler(control: u32) -> i32 {
    let signal = match control {
        CTRL_C_EVENT => Signal::Int,
        CTRL_BREAK_EVENT => Signal::Break,
        CTRL_CLOSE_EVENT => Signal::Hup,
        _ => return 0,
    };
    i32::from(dispatch(signal))
}
impl Subscription {
    pub(super) fn new(signal: Signal, notifier: Notifier) -> Result<Self> {
        if !matches!(
            signal,
            Signal::Int | Signal::Break | Signal::Hup | Signal::WinCh
        ) {
            return Err(unsupported());
        }
        let mut count = REGISTRATION.lock().unwrap_or_else(|e| e.into_inner());
        let index = SLOTS
            .iter()
            .position(|s| s.load(Ordering::SeqCst).is_null())
            .ok_or(Error::new(ErrorKind::ResourceLimit))?;
        if *count == 0 {
            // SAFETY: process-lifetime function pointer; handler has no locks or allocations.
            bool_result(unsafe { SetConsoleCtrlHandler(Some(handler), 1) })?;
        }
        let mut ticket = Box::new(Ticket {
            signal,
            pending: AtomicBool::new(false),
            notifier,
        });
        SLOTS[index].store(ptr::from_mut(&mut *ticket), Ordering::SeqCst);
        *count += 1;
        Ok(Self { ticket, index })
    }
    pub(super) fn ready(&self) -> bool {
        self.ticket.pending.load(Ordering::Acquire)
    }
    pub(super) fn take(&self) -> bool {
        self.ticket.pending.swap(false, Ordering::AcqRel)
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let mut count = REGISTRATION.lock().unwrap_or_else(|e| e.into_inner());
        SLOTS[self.index].store(ptr::null_mut(), Ordering::SeqCst);
        *count -= 1;
        if *count == 0 {
            // SAFETY: remove only our handler, preserving every host handler.
            unsafe {
                SetConsoleCtrlHandler(Some(handler), 0);
            }
        }
        while ACTIVE.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }
}
