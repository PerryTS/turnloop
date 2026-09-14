//! EV_CLEAR readiness and an EVFILT_USER wake registered on the same kqueue.
use crate::{
    Result,
    backend::{
        PollInfo, Wake,
        poller::{Poller, Ready, last_error, timespec},
    },
};
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
pub struct KqueueWake {
    fd: OwnedFd,
    calls: AtomicU64,
}
impl Wake for KqueueWake {
    fn wake(&self) -> Result<()> {
        let ev = event(0, libc::EVFILT_USER, 0, libc::NOTE_TRIGGER, 0);
        loop {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // SAFETY: kqueue is owned by this Arc; the change is valid for the call.
            let n = unsafe {
                libc::kevent(
                    self.fd.as_raw_fd(),
                    &ev,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if n >= 0 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
    }
    fn syscall_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}
pub(crate) struct Kqueue {
    wake: Arc<KqueueWake>,
    events: Vec<libc::kevent>,
}
fn event(ident: usize, filter: i16, flags: u16, fflags: u32, key: u64) -> libc::kevent {
    libc::kevent {
        ident,
        filter,
        flags,
        fflags,
        data: 0,
        udata: key as usize as *mut libc::c_void,
    }
}
impl Poller for Kqueue {
    type W = KqueueWake;
    fn new(capacity: usize) -> Result<Self> {
        // SAFETY: kqueue takes no pointers and returns a new owned descriptor.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(last_error());
        }
        // SAFETY: successful kqueue returned exclusive ownership of fd.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: valid owned descriptor, integer fcntl argument.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(last_error());
        }
        let ev = event(0, libc::EVFILT_USER, libc::EV_ADD | libc::EV_CLEAR, 0, 0);
        // SAFETY: valid change record; no event output and therefore no wait.
        if unsafe {
            libc::kevent(
                fd.as_raw_fd(),
                &ev,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(last_error());
        }
        Ok(Self {
            wake: Arc::new(KqueueWake {
                fd,
                calls: AtomicU64::new(0),
            }),
            events: vec![event(0, 0, 0, 0, 0); capacity],
        })
    }
    fn waker(&self) -> Arc<Self::W> {
        self.wake.clone()
    }
    fn register(&mut self, fd: RawFd, key: u64) -> Result<()> {
        let changes = [
            event(
                fd as usize,
                libc::EVFILT_READ,
                libc::EV_ADD | libc::EV_CLEAR,
                0,
                key,
            ),
            event(
                fd as usize,
                libc::EVFILT_WRITE,
                libc::EV_ADD | libc::EV_CLEAR,
                0,
                key,
            ),
        ];
        // SAFETY: descriptors and both initialized input records are valid.
        if unsafe {
            libc::kevent(
                self.fd(),
                changes.as_ptr(),
                2,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(last_error());
        }
        Ok(())
    }
    fn deregister(&mut self, fd: RawFd) -> Result<()> {
        let changes = [
            event(fd as usize, libc::EVFILT_READ, libc::EV_DELETE, 0, 0),
            event(fd as usize, libc::EVFILT_WRITE, libc::EV_DELETE, 0, 0),
        ];
        // SAFETY: valid registered fd and initialized input records; no wait.
        if unsafe {
            libc::kevent(
                self.fd(),
                changes.as_ptr(),
                2,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(last_error());
        }
        Ok(())
    }
    fn wait(&mut self, timeout: Option<Duration>, out: &mut Vec<Ready>) -> Result<PollInfo> {
        let ts = timeout.map(timespec);
        // SAFETY: output storage holds events.len() initialized kevents. Timeout
        // pointer is either null or points to a live timespec for this call.
        let n = unsafe {
            libc::kevent(
                self.fd(),
                std::ptr::null(),
                0,
                self.events.as_mut_ptr(),
                self.events.len() as i32,
                ts.as_ref().map_or(std::ptr::null(), |v| v),
            )
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                return Ok(PollInfo { waits: 1, zero_event_waits: 1 });
            }
            return Err(e.into());
        }
        for e in &self.events[..n as usize] {
            if e.filter == libc::EVFILT_USER {
                continue;
            }
            out.push(Ready {
                key: e.udata as usize as u64,
                read: e.filter == libc::EVFILT_READ,
                write: e.filter == libc::EVFILT_WRITE,
            });
        }
        Ok(PollInfo { waits: 1, zero_event_waits: u32::from(n == 0) })
    }
    fn fd(&self) -> RawFd {
        self.wake.fd.as_raw_fd()
    }
}
