//! Edge-triggered epoll with eventfd wake and nanosecond wait budgets.
//! epoll_pwait2 support is probed at construction, never with a second wait in a
//! turn. Older kernels use a one-shot timerfd plus one epoll_wait(-1).
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
const WAKE: u64 = 0;
const TIMER: u64 = u64::MAX;
pub struct EpollWake {
    fd: OwnedFd,
    calls: AtomicU64,
}
impl Wake for EpollWake {
    fn wake(&self) -> Result<()> {
        let value: u64 = 1;
        loop {
            self.calls.fetch_add(1, Ordering::Relaxed);
            // SAFETY: eventfd is owned by this Arc and value is a live eight-byte integer.
            let n = unsafe { libc::write(self.fd.as_raw_fd(), (&value as *const u64).cast(), 8) };
            if n == 8 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(());
            } // already readable
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
    }
    fn syscall_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}
pub(crate) struct Epoll {
    fd: OwnedFd,
    wake: Arc<EpollWake>,
    timer: Option<OwnedFd>,
    events: Vec<libc::epoll_event>,
}
fn owned(fd: i32) -> Result<OwnedFd> {
    if fd < 0 {
        return Err(last_error());
    }
    // SAFETY: caller passes only newly created descriptors from successful syscalls.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
fn add(epoll: RawFd, fd: RawFd, key: u64, flags: i32) -> Result<()> {
    let mut e = libc::epoll_event {
        events: flags as u32,
        u64: key,
    };
    // SAFETY: valid descriptors and initialized epoll event input.
    if unsafe { libc::epoll_ctl(epoll, libc::EPOLL_CTL_ADD, fd, &mut e) } < 0 {
        return Err(last_error());
    }
    Ok(())
}
fn drain_counter(fd: RawFd) -> Result<()> {
    let mut count = 0u64;
    loop {
        // SAFETY: a writable eight-byte integer is the required counter read buffer.
        let n = unsafe { libc::read(fd, (&mut count as *mut u64).cast(), 8) };
        if n == 8 {
            continue;
        }
        let e = std::io::Error::last_os_error();
        if e.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(());
        }
        if e.kind() != std::io::ErrorKind::Interrupted {
            return Err(e.into());
        }
    }
}
impl Poller for Epoll {
    type W = EpollWake;
    fn new(capacity: usize) -> Result<Self> {
        // SAFETY: valid epoll creation flags; returns a new descriptor.
        let fd = owned(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?;
        // SAFETY: valid eventfd initial value and flags; returns a new descriptor.
        let wakefd = owned(unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) })?;
        add(
            fd.as_raw_fd(),
            wakefd.as_raw_fd(),
            WAKE,
            libc::EPOLLIN | libc::EPOLLET,
        )?;
        let mut event = libc::epoll_event { events: 0, u64: 0 };
        let zero = timespec(Duration::ZERO);
        // SAFETY: valid epoll descriptor, one event output slot, zero timeout, and
        // null signal mask. The Linux kernel sigset ABI is eight bytes.
        let probe = unsafe {
            libc::syscall(
                libc::SYS_epoll_pwait2,
                fd.as_raw_fd(),
                &mut event,
                1,
                &zero,
                std::ptr::null::<libc::sigset_t>(),
                8usize,
            )
        };
        let unsupported = if probe < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOSYS) {
                return Err(e.into());
            }
            true
        } else {
            false
        };
        let timer = if unsupported || cfg!(feature = "epoll-timerfd") {
            // SAFETY: CLOCK_MONOTONIC and timerfd flags are valid.
            let timer = owned(unsafe {
                libc::timerfd_create(
                    libc::CLOCK_MONOTONIC,
                    libc::TFD_NONBLOCK | libc::TFD_CLOEXEC,
                )
            })?;
            add(
                fd.as_raw_fd(),
                timer.as_raw_fd(),
                TIMER,
                libc::EPOLLIN | libc::EPOLLET,
            )?;
            Some(timer)
        } else {
            None
        };
        Ok(Self {
            fd,
            wake: Arc::new(EpollWake {
                fd: wakefd,
                calls: AtomicU64::new(0),
            }),
            timer,
            events: vec![libc::epoll_event { events: 0, u64: 0 }; capacity],
        })
    }
    fn waker(&self) -> Arc<Self::W> {
        self.wake.clone()
    }
    fn register(&mut self, fd: RawFd, key: u64) -> Result<()> {
        add(
            self.fd(),
            fd,
            key,
            libc::EPOLLIN | libc::EPOLLOUT | libc::EPOLLRDHUP | libc::EPOLLET,
        )
    }
    fn deregister(&mut self, fd: RawFd) -> Result<()> {
        // SAFETY: valid descriptors; Linux ignores the event argument for DEL.
        if unsafe { libc::epoll_ctl(self.fd(), libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut()) } < 0
        {
            return Err(last_error());
        }
        Ok(())
    }
    fn wait(&mut self, timeout: Option<Duration>, out: &mut Vec<Ready>) -> Result<PollInfo> {
        let n = if let Some(timer) = &self.timer {
            let spec = libc::itimerspec {
                it_interval: timespec(Duration::ZERO),
                it_value: timespec(timeout.unwrap_or(Duration::ZERO)),
            };
            // SAFETY: initialized relative one-shot timer specification.
            if unsafe { libc::timerfd_settime(timer.as_raw_fd(), 0, &spec, std::ptr::null_mut()) }
                < 0
            {
                return Err(last_error());
            }
            let millis = if timeout == Some(Duration::ZERO) {
                0
            } else {
                -1
            };
            // SAFETY: initialized output event array of the specified capacity.
            unsafe {
                libc::epoll_wait(
                    self.fd(),
                    self.events.as_mut_ptr(),
                    self.events.len() as i32,
                    millis,
                ) as i64
            }
        } else {
            let ts = timeout.map(timespec);
            // SAFETY: valid descriptor, writable event array and optional live timespec.
            unsafe {
                libc::syscall(
                    libc::SYS_epoll_pwait2,
                    self.fd(),
                    self.events.as_mut_ptr(),
                    self.events.len() as i32,
                    ts.as_ref().map_or(std::ptr::null(), |v| v),
                    std::ptr::null::<libc::sigset_t>(),
                    8usize,
                ) as i64
            }
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                return Ok(PollInfo { waits: 1, zero_event_waits: 1 });
            }
            return Err(e.into());
        }
        for e in &self.events[..n as usize] {
            let key = e.u64;
            if key == WAKE {
                drain_counter(self.wake.fd.as_raw_fd())?;
            } else if key == TIMER {
                if let Some(timer) = &self.timer {
                    drain_counter(timer.as_raw_fd())?;
                }
            } else {
                let flags = e.events as i32;
                out.push(Ready {
                    key,
                    read: flags
                        & (libc::EPOLLIN | libc::EPOLLHUP | libc::EPOLLRDHUP | libc::EPOLLERR)
                        != 0,
                    write: flags & (libc::EPOLLOUT | libc::EPOLLHUP | libc::EPOLLERR) != 0,
                });
            }
        }
        Ok(PollInfo { waits: 1, zero_event_waits: u32::from(n == 0) })
    }
    fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}
