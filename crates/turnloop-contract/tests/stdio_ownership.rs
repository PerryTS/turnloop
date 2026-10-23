//! Process-isolated contracts for shared Unix stdio descriptions.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    )
))]
use std::{process::Command, sync::Barrier, time::Duration};
use turnloop::*;

fn isolated(name: &str, body: impl FnOnce()) {
    if std::env::var("TURNLOOP_STDIO_CASE").as_deref() == Ok(name) {
        body();
        return;
    }
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", name, "--nocapture"])
        .env("TURNLOOP_STDIO_CASE", name)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("isolated fixture");
    // Keep stdin open: a cancelled pending read must not depend on EOF.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while child.try_wait().expect("child status").is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill().expect("kill timed out fixture");
            let output = child.wait_with_output().expect("reap fixture");
            panic!("{name} timed out: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().expect("fixture output");
    assert!(output.status.success(), "{name}: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

fn flags(fd: i32) -> i32 {
    // SAFETY: integer-only query on the fixture's standard descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    assert!(flags >= 0, "F_GETFL: {}", std::io::Error::last_os_error());
    flags
}
fn nonblocking(fd: i32) {
    assert_ne!(
        flags(fd) & libc::O_NONBLOCK,
        0,
        "live stream became blocking"
    );
}
fn close(l: &mut Loop, h: Handle, pending_read: bool) {
    l.close(h, Token(99)).expect("close");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut cancelled = 0;
    let mut closed = false;
    while !closed {
        assert!(l.now() < until, "close timed out");
        l.turn(Timeout::Until(until), &mut out).expect("close turn");
        for completion in out.drain() {
            match completion.result {
                OpResult::Cancelled => cancelled += 1,
                OpResult::Closed => {
                    assert_eq!(cancelled, usize::from(pending_read));
                    assert!(!closed, "duplicate close");
                    closed = true;
                }
                other => panic!("unexpected completion {other:?}"),
            }
        }
    }
}

#[test]
fn overlapping_stdio_close_orders_restore_original_flags() {
    isolated(
        "overlapping_stdio_close_orders_restore_original_flags",
        || {
            let before = flags(1);
            for reverse in [false, true] {
                let mut l = Loop::new(Config::default()).expect("loop");
                let a = l.open_stdio(Stdio::Stdout).expect("first stdout");
                let b = l.open_stdio(Stdio::Stdout).expect("second stdout");
                nonblocking(1);
                let (first, last) = if reverse { (b, a) } else { (a, b) };
                close(&mut l, first, false);
                nonblocking(1);
                close(&mut l, last, false);
                assert_eq!(flags(1), before);
            }
        },
    );
}

#[test]
fn aliased_stdout_stderr_survive_other_loop_drop() {
    isolated("aliased_stdout_stderr_survive_other_loop_drop", || {
        // SAFETY: isolated child, before any stdio adoption or worker exists.
        assert_eq!(unsafe { libc::dup2(1, 2) }, 2);
        let before = flags(1);
        let mut first = Loop::new(Config::default()).expect("first loop");
        let mut last = Loop::new(Config::default()).expect("last loop");
        first.open_stdio(Stdio::Stdout).expect("stdout");
        let error = last.open_stdio(Stdio::Stderr).expect("stderr alias");
        drop(first);
        nonblocking(1);
        nonblocking(2);
        close(&mut last, error, false);
        assert_eq!(flags(1), before);
        assert_eq!(flags(2), before);
    });
}

#[test]
fn stdio_handoff_releases_lease_without_closing_host_descriptor() {
    isolated(
        "stdio_handoff_releases_lease_without_closing_host_descriptor",
        || {
            let before = flags(1);
            let mut l = Loop::new(Config::default()).expect("loop");
            let a = l.open_stdio(Stdio::Stdout).expect("first stdout");
            let b = l.open_stdio(Stdio::Stdout).expect("second stdout");
            let fd = l.detach(a).expect("detach first").into_fd();
            nonblocking(1);
            drop(fd);
            let fd = l.detach(b).expect("detach last").into_fd();
            assert_eq!(flags(1), before);
            drop(fd);
            assert_eq!(flags(1), before);
        },
    );
}

#[test]
fn pending_stdio_read_cancels_before_restoring_shared_flags() {
    isolated(
        "pending_stdio_read_cancels_before_restoring_shared_flags",
        || {
            let before = flags(0);
            let mut l = Loop::new(Config::default()).expect("loop");
            let a = l.open_stdio(Stdio::Stdin).expect("first stdin");
            let b = l.open_stdio(Stdio::Stdin).expect("second stdin");
            l.read(a, ReadBuf::Pooled, Token(1))
                .expect("pending first read");
            let mut out = Completions::default();
            l.turn(Timeout::Now, &mut out).expect("submit read");
            assert_eq!(out.len(), 0, "stdin must still be open and empty");
            close(&mut l, a, true);
            nonblocking(0);
            l.read(b, ReadBuf::Pooled, Token(2))
                .expect("pending last read");
            l.turn(Timeout::Now, &mut out)
                .expect("submit surviving read");
            assert_eq!(out.len(), 0);
            close(&mut l, b, true);
            assert_eq!(flags(0), before);
        },
    );
}

#[test]
fn initially_nonblocking_stdio_and_closed_unused_descriptor() {
    isolated(
        "initially_nonblocking_stdio_and_closed_unused_descriptor",
        || {
            let before = flags(1) | libc::O_NONBLOCK;
            // SAFETY: fixture owns its process stdio, before any adoption exists.
            assert_eq!(unsafe { libc::fcntl(1, libc::F_SETFL, before) }, 0);
            let mut l = Loop::new(Config::default()).expect("loop");
            // SAFETY: isolated fixture will not read its original stdin again.
            assert_eq!(unsafe { libc::close(0) }, 0);
            let h = l.open_stdio(Stdio::Stdout).expect("stdout");
            // SAFETY: integer-only query; none of the duplicates may fill slot zero.
            assert_eq!(unsafe { libc::fcntl(0, libc::F_GETFL) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
            assert!(l.open_stdio(Stdio::Stdin).is_err());
            close(&mut l, h, false);
            assert_eq!(flags(1), before);
        },
    );
}

#[test]
fn stdio_leases_overlap_across_threads_and_repeated_cohorts() {
    isolated(
        "stdio_leases_overlap_across_threads_and_repeated_cohorts",
        || {
            let before = flags(1);
            let barrier = Barrier::new(4);
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut l = Loop::new(Config::default()).expect("thread loop");
                        for _ in 0..64 {
                            let h = l.open_stdio(Stdio::Stdout).expect("thread stdout");
                            barrier.wait();
                            nonblocking(1);
                            barrier.wait();
                            close(&mut l, h, false);
                            barrier.wait();
                            assert_eq!(flags(1), before);
                            barrier.wait();
                        }
                    });
                }
            });
            assert_eq!(flags(1), before);
        },
    );
}

#[test]
fn overlapping_terminal_modes_restore_the_first_snapshot() {
    isolated(
        "overlapping_terminal_modes_restore_the_first_snapshot",
        || {
            use std::os::fd::{FromRawFd, OwnedFd};
            let (mut master, mut slave) = (-1, -1);
            assert_eq!(
                // SAFETY: writable descriptor outputs; null pointers request defaults.
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            // SAFETY: openpty returned two fresh owned descriptors.
            let (_master, _slave) =
                unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
            // SAFETY: isolated child changes its stdin before managed adoption.
            assert_eq!(unsafe { libc::dup2(slave, 0) }, 0);
            let get = || {
                let mut available = 0;
                // SAFETY: live PTY and writable integer. Settle XNU's PENDIN state.
                assert_eq!(unsafe { libc::ioctl(0, libc::FIONREAD, &mut available) }, 0);
                // SAFETY: termios is plain C output storage.
                let mut mode: libc::termios = unsafe { std::mem::zeroed() };
                // SAFETY: live PTY and writable initialized storage.
                assert_eq!(unsafe { libc::tcgetattr(0, &mut mode) }, 0);
                (
                    mode.c_iflag,
                    mode.c_oflag,
                    mode.c_cflag,
                    mode.c_lflag,
                    mode.c_cc,
                )
            };
            let original = get();
            let mut l = Loop::new(Config::default()).expect("loop");
            let a = l.open_stdio(Stdio::Stdin).expect("first terminal");
            l.tty_set_mode(a, TtyMode::Raw).expect("raw mode");
            let raw = get();
            assert_ne!(raw, original);
            let b = l.open_stdio(Stdio::Stdin).expect("second terminal");
            close(&mut l, a, false);
            assert_eq!(get(), raw, "first close reset a live terminal");
            l.tty_window_size(b).expect("terminal remains recognized");
            l.tty_set_mode(b, TtyMode::Normal)
                .expect("original normal mode");
            assert_eq!(
                get(),
                original,
                "second adoption captured an already raw mode"
            );
            l.tty_set_mode(b, TtyMode::Raw).expect("raw again");
            close(&mut l, b, false);
            assert_eq!(get(), original);
        },
    );
}

#[test]
fn failed_stdio_snapshot_releases_partial_duplicates() {
    isolated("failed_stdio_snapshot_releases_partial_duplicates", || {
        let before = flags(1);
        let mut l = Loop::new(Config::default()).expect("loop");
        // SAFETY: rlimit is plain C output storage.
        let mut old: libc::rlimit = unsafe { std::mem::zeroed() };
        // SAFETY: writable output for this process's descriptor limit.
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut old) }, 0);
        let small = libc::rlimit {
            rlim_cur: 64.min(old.rlim_cur),
            rlim_max: old.rlim_max,
        };
        // SAFETY: isolated subprocess lowers only its own soft limit.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &small) }, 0);
        let mut fillers = Vec::new();
        loop {
            match std::fs::File::open("/dev/null") {
                Ok(fd) => fillers.push(fd),
                Err(e) => {
                    assert_eq!(e.raw_os_error(), Some(libc::EMFILE));
                    break;
                }
            }
        }
        assert!(fillers.len() > 2);
        drop(fillers.pop());
        drop(fillers.pop());
        // Room for the requested duplicate and one saved descriptor, but not
        // the whole snapshot. Failure must release both, without changing flags.
        assert_eq!(
            l.open_stdio(Stdio::Stdout)
                .expect_err("partial snapshot")
                .os,
            Some(libc::EMFILE)
        );
        assert_eq!(flags(1), before);
        let one = std::fs::File::open("/dev/null").expect("first slot returned");
        let two = std::fs::File::open("/dev/null").expect("second slot returned");
        drop((one, two, fillers));
        // SAFETY: restore the original soft limit without exceeding the hard limit.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &old) }, 0);
        let h = l
            .open_stdio(Stdio::Stdout)
            .expect("fresh snapshot after failure");
        close(&mut l, h, false);
        assert_eq!(flags(1), before);
    });
}
