use super::{CHANGE, OVERFLOW, RENAME, State};
use crate::*;
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::Arc,
    thread::JoinHandle,
};
fn error() -> Error {
    std::io::Error::last_os_error().into()
}
pub(super) struct Watch {
    stop: OwnedFd,
    thread: Option<JoinHandle<()>>,
}
impl Watch {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn new(path: &FsPath, recursive: bool, state: Arc<State>) -> Result<Self> {
        if recursive || path.preopen.is_some() {
            return Err(Error::new(ErrorKind::Unsupported));
        }
        // SAFETY: scalar flags; resulting descriptor is uniquely owned.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(error());
        }
        // SAFETY: new descriptor from successful inotify_init1.
        let events = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: live inotify fd, prepared path and native event mask.
        let wd = unsafe {
            libc::inotify_add_watch(
                fd,
                path.native.as_ptr(),
                libc::IN_MODIFY
                    | libc::IN_ATTRIB
                    | libc::IN_CREATE
                    | libc::IN_DELETE
                    | libc::IN_MOVED_FROM
                    | libc::IN_MOVED_TO
                    | libc::IN_MOVE_SELF
                    | libc::IN_DELETE_SELF,
            )
        };
        if wd < 0 {
            return Err(error());
        }
        let mut pipe = [0; 2];
        // SAFETY: writable descriptor pair and supported pipe flags.
        if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
            return Err(error());
        }
        // SAFETY: pipe2 returned two uniquely owned descriptors.
        let (read, stop) =
            unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        let thread = std::thread::Builder::new()
            .name("turnloop-watch".into())
            .spawn(move || {
                let mut poll = [
                    libc::pollfd {
                        fd: events.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: read.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                let mut buffer = [0u8; 8192];
                loop {
                    // SAFETY: two live descriptors and correctly sized poll output; no periodic timeout.
                    let n = unsafe { libc::poll(poll.as_mut_ptr(), 2, -1) };
                    if n < 0 {
                        if error().os == Some(libc::EINTR) {
                            continue;
                        }
                        state.event(OVERFLOW);
                        break;
                    }
                    if poll[1].revents != 0 {
                        break;
                    }
                    // SAFETY: live nonblocking inotify fd and writable stack buffer.
                    let n = unsafe {
                        libc::read(events.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len())
                    };
                    if n < 0 {
                        let e = error();
                        if e.os == Some(libc::EAGAIN) || e.os == Some(libc::EINTR) {
                            continue;
                        }
                        state.event(OVERFLOW);
                        break;
                    }
                    let mut at = 0;
                    let mut flags = 0;
                    let mut removed = false;
                    while at + 16 <= n as usize {
                        let mask =
                            u32::from_ne_bytes(buffer[at + 4..at + 8].try_into().expect("mask"));
                        let len = u32::from_ne_bytes(
                            buffer[at + 12..at + 16].try_into().expect("length"),
                        ) as usize;
                        if mask & (libc::IN_MODIFY | libc::IN_ATTRIB) != 0 {
                            flags |= CHANGE;
                        }
                        if mask
                            & (libc::IN_CREATE
                                | libc::IN_DELETE
                                | libc::IN_MOVED_FROM
                                | libc::IN_MOVED_TO
                                | libc::IN_MOVE_SELF
                                | libc::IN_DELETE_SELF)
                            != 0
                        {
                            flags |= RENAME;
                        }
                        if mask & libc::IN_Q_OVERFLOW != 0 {
                            flags |= OVERFLOW;
                        }
                        removed |= mask & libc::IN_IGNORED != 0;
                        at += 16 + len;
                    }
                    state.event(flags);
                    if removed {
                        break;
                    }
                }
                state.stopped();
            })
            .map_err(Error::from)?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
    #[cfg(target_os = "freebsd")]
    pub fn new(path: &FsPath, recursive: bool, state: Arc<State>) -> Result<Self> {
        if recursive || path.preopen.is_some() {
            return Err(Error::new(ErrorKind::Unsupported));
        }
        // SAFETY: prepared path and scalar flags.
        let fd = unsafe {
            libc::open(
                path.native.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(error());
        }
        // SAFETY: uniquely owned new descriptor.
        let file = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: kqueue creates a fresh descriptor.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(error());
        }
        // SAFETY: uniquely owned new descriptor.
        let stop = unsafe { OwnedFd::from_raw_fd(kq) };
        let queue = stop.try_clone().map_err(Error::from)?;
        let changes = [
            libc::kevent {
                ident: fd as _,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE
                    | libc::NOTE_EXTEND
                    | libc::NOTE_ATTRIB
                    | libc::NOTE_RENAME
                    | libc::NOTE_DELETE,
                data: 0,
                udata: std::ptr::null_mut(),
                ext: [0; 4],
            },
            libc::kevent {
                ident: 1,
                filter: libc::EVFILT_USER,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
                ext: [0; 4],
            },
        ];
        // SAFETY: two initialized filters, live descriptors, no event output.
        if unsafe {
            libc::kevent(
                kq,
                changes.as_ptr(),
                2,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(error());
        }
        let thread = std::thread::Builder::new()
            .name("turnloop-watch".into())
            .spawn(move || {
                let _file = file;
                let mut event = std::mem::MaybeUninit::uninit();
                loop {
                    // SAFETY: owned kqueue and correctly sized event output; blocks without ticks.
                    let n = unsafe {
                        libc::kevent(
                            queue.as_raw_fd(),
                            std::ptr::null(),
                            0,
                            event.as_mut_ptr(),
                            1,
                            std::ptr::null(),
                        )
                    };
                    if n < 0 {
                        if error().os == Some(libc::EINTR) {
                            continue;
                        }
                        state.event(OVERFLOW);
                        break;
                    }
                    // SAFETY: one event initialized by successful kevent.
                    let e = unsafe { event.assume_init() };
                    if e.filter == libc::EVFILT_USER {
                        break;
                    }
                    state.event(
                        CHANGE
                            | if e.fflags & (libc::NOTE_RENAME | libc::NOTE_DELETE) != 0 {
                                RENAME
                            } else {
                                0
                            },
                    );
                }
                state.stopped();
            })
            .map_err(Error::from)?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
    pub fn cancel(&mut self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            // SAFETY: owned cancellation pipe, stack byte valid for synchronous write.
            if unsafe { libc::write(self.stop.as_raw_fd(), [1u8].as_ptr().cast(), 1) } < 0
                && error().os != Some(libc::EAGAIN)
            {
                return Err(error());
            }
        }
        #[cfg(target_os = "freebsd")]
        {
            let e = libc::kevent {
                ident: 1,
                filter: libc::EVFILT_USER,
                flags: 0,
                fflags: libc::NOTE_TRIGGER,
                data: 0,
                udata: std::ptr::null_mut(),
                ext: [0; 4],
            };
            // SAFETY: live kqueue and registered user-event identity.
            if unsafe {
                libc::kevent(
                    self.stop.as_raw_fd(),
                    &e,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            } < 0
            {
                return Err(error());
            }
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
