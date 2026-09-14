use super::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub fn integration_fd<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let Integration::Fd(inner) = l.integration().expect("integration") else {
        panic!("Unix fd required");
    };
    let poster = l.poster();
    let producer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        poster.post(Token(42), Payload::U64(99)).expect("post");
    });
    wait_external(inner);
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("turn");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].token, Token(42));
    assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(99))));
    producer.join().expect("producer");
}
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
fn wait_external(inner: i32) {
    // SAFETY: kqueue takes no pointer arguments and creates a descriptor.
    let raw = unsafe { libc::kqueue() };
    assert!(
        raw >= 0,
        "outer kqueue: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: successful kqueue returns exclusive ownership of raw.
    let outer = unsafe { OwnedFd::from_raw_fd(raw) };
    let event = libc::kevent {
        ident: inner as usize,
        filter: libc::EVFILT_READ,
        flags: libc::EV_ADD,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut received = event;
    let timeout = libc::timespec {
        tv_sec: 2,
        tv_nsec: 0,
    };
    // SAFETY: input/output records and timeout are initialized and live for the call.
    let n = unsafe { libc::kevent(outer.as_raw_fd(), &event, 1, &mut received, 1, &timeout) };
    assert_eq!(n, 1, "external kqueue must actually wake");
    let ident = received.ident;
    assert_eq!(ident, inner as usize);
    assert_eq!(received.filter, libc::EVFILT_READ);
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn wait_external(inner: i32) {
    // SAFETY: valid epoll flags; returns a new descriptor.
    let raw = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    assert!(raw >= 0);
    // SAFETY: successful epoll_create1 returns exclusive ownership.
    let outer = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut event = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: 42,
    };
    assert_eq!(
        // SAFETY: initialized input event and two valid epoll descriptors.
        unsafe { libc::epoll_ctl(outer.as_raw_fd(), libc::EPOLL_CTL_ADD, inner, &mut event) },
        0
    );
    // SAFETY: one writable output event slot and a bounded timeout.
    let n = unsafe { libc::epoll_wait(outer.as_raw_fd(), &mut event, 1, 2000) };
    assert_eq!(n, 1, "external epoll must actually wake");
    let key = event.u64;
    assert_eq!(key, 42);
}
