#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    )
))]
use turnloop::*;

// A fan-out fixture must consume only its own SIGUSR1. Another fixture's send
// can release its workers before its own send, after they restore SIG_DFL.
static SIGNAL_FANOUT: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn local_echo_and_descriptor_ownership() {
    let path = std::env::temp_dir().join(format!("tl-ipc-{}.sock", std::process::id()));
    let name = PipeName(path.clone());
    turnloop_contract::native_surface::ipc::<backend::Platform>(&name);
    std::fs::remove_file(path).expect("remove listener path");
}

#[test]
fn concurrent_256_children_exit_once() {
    turnloop_contract::native_surface::children::<backend::Platform>(std::ffi::OsStr::new(env!(
        "CARGO_BIN_EXE_native_child"
    )));
}
#[test]
fn spawned_child_stdio_uses_the_driver() {
    turnloop_contract::native_surface::child_stdio::<backend::Platform>(std::ffi::OsStr::new(
        env!("CARGO_BIN_EXE_native_child"),
    ));
}
#[test]
fn signals_reach_four_loops_on_four_threads() {
    let _guard = SIGNAL_FANOUT.lock().expect("serialize SIGUSR1 fixtures");
    turnloop_contract::native_surface::signal_fanout::<backend::Platform>(Signal::Usr1, || {
        // SAFETY: SIGUSR1 is subscribed by all four loops before the barrier opens.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
    });
}
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
#[test]
fn pending_signal_cannot_outlive_kqueue_unsubscribe() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_native_child"))
        .arg("blocked-signal")
        .output()
        .expect("signal fixture");
    assert!(
        output.status.success(),
        "blocked signal fixture: {:?}; {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"four deliveries, four stops, four closes; survived unblock\n"
    );
}

#[test]
fn signal_subscribe_raise_unsubscribe_stress() {
    let _guard = SIGNAL_FANOUT.lock().expect("serialize SIGUSR1 fixtures");
    // Each round proves delivery and Stopped/Closed on all four owning threads;
    // the next round exercises restoration followed by fresh subscriptions.
    for _ in 0..256 {
        turnloop_contract::native_surface::signal_fanout::<backend::Platform>(Signal::Usr1, || {
            // SAFETY: the fan-out barrier proves all four subscriptions exist.
            assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
        });
    }
}
#[test]
fn shared_external_wait_service_routes_and_cancels() {
    turnloop_contract::native_surface::external_waits::<backend::Platform>();
}
#[test]
fn kills_live_child_and_grandchild_as_a_group() {
    turnloop_contract::native_surface::process_group::<backend::Platform>(std::ffi::OsStr::new(
        env!("CARGO_BIN_EXE_native_child"),
    ));
}
#[test]
fn registered_processes_and_signals_do_not_spin() {
    // Every owned child subscribes to process-wide SIGCHLD. Parallel tests
    // exiting unrelated children notify this loop too, invalidating the quiet
    // premise. A fresh process gives the unchanged contract its own dispatcher.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_native_child"))
        .arg("services-no-spin")
        .output()
        .expect("no-spin fixture");
    assert!(
        output.status.success(),
        "no-spin fixture: {:?}; {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"60 service timer expiries; no-spin bounds passed\n"
    );
}
#[test]
fn terminal_modes_resize_and_restore_on_close_and_drop() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let (mut master, mut slave) = (-1, -1);
    assert_eq!(
        // SAFETY: valid writable descriptor slots; null name/termios/winsize use defaults.
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
    // SAFETY: successful openpty transferred two new exclusively owned descriptors.
    let (_master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let get = || {
        let mut available = 0;
        assert_eq!(
            // SAFETY: query pending input on the owned PTY. On XNU, FIONREAD applies
            // the pending canonical-mode transition (PENDIN) without consuming bytes,
            // so the subsequent full termios equality compares settled terminal state.
            unsafe { libc::ioctl(slave.as_raw_fd(), libc::FIONREAD, &mut available) },
            0
        );
        // SAFETY: termios is valid zeroed C output storage.
        let mut mode: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: live PTY slave and initialized writable termios.
        assert_eq!(unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut mode) }, 0);
        mode
    };
    let original = get();
    for close in [true, false] {
        let mut l = Loop::new(Config::default()).expect("loop");
        let h = l
            .attach(
                Detached::from_fd(slave.try_clone().expect("dup slave")).expect("adopt tty"),
                Token(1),
            )
            .expect("attach tty");
        l.tty_set_mode(h, TtyMode::Raw).expect("raw");
        assert_eq!(get().c_lflag & libc::ICANON, 0);
        assert_ne!(get().c_lflag & libc::ISIG, 0);
        l.tty_set_mode(h, TtyMode::Io).expect("io");
        assert_eq!(get().c_lflag & libc::ISIG, 0);
        l.tty_set_mode(h, TtyMode::Normal).expect("normal");
        assert_eq!(get().c_lflag, original.c_lflag);
        let resize = l
            .tty_resize_start(h, Token(2))
            .expect("resize subscription");
        let size = libc::winsize {
            ws_row: 31,
            ws_col: 117,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            // SAFETY: live PTY and initialized window size. This fixture has no controlling session.
            unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            0
        );
        // SAFETY: SIGWINCH is subscribed; simulate the controlling-terminal notification.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGWINCH) }, 0);
        let mut out = Completions::default();
        let until = l.now() + std::time::Duration::from_secs(2);
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out)
                .expect("resize turn");
            if !out.is_empty() {
                assert_eq!(out.len(), 1);
                assert!(matches!(out[0].result, OpResult::Signal(Signal::WinCh)));
                break;
            }
        }
        assert_eq!(
            l.tty_window_size(h).expect("size"),
            WindowSize {
                columns: 117,
                rows: 31
            }
        );
        l.signal_stop(resize, Token(3)).expect("stop resize");
        l.tty_set_mode(h, TtyMode::Io).expect("io again");
        if close {
            l.close(h, Token(4)).expect("close tty");
            let mut closed = false;
            while !closed {
                l.turn(Timeout::Now, &mut out).expect("close turn");
                closed |= out
                    .iter()
                    .any(|c| c.handle == Some(h) && matches!(c.result, OpResult::Closed));
            }
        }
        drop(l);
        let restored = get();
        assert_eq!(restored.c_lflag, original.c_lflag);
        assert_eq!(restored.c_iflag, original.c_iflag);
        assert_eq!(restored.c_oflag, original.c_oflag);
    }
}
#[test]
fn file_backed_stdio_runs_in_the_child() {
    use std::{
        io::{Read, Seek, SeekFrom, Write},
        process::{Command, Stdio as ChildStdio},
    };
    let path = std::env::temp_dir().join(format!("tl-stdio-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("fixture file");
    file.write_all(b"regular file stdin\n")
        .expect("fixture write");
    file.seek(SeekFrom::Start(0)).expect("seek");
    let output = Command::new(env!("CARGO_BIN_EXE_native_child"))
        .arg("stdio")
        .stdin(ChildStdio::from(file.try_clone().expect("dup file")))
        .output()
        .expect("spawn file stdin");
    assert_eq!(
        output.status.code(),
        Some(23),
        "child stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"regular file stdin\n");
    assert_eq!(output.stderr, output.stdout);
    // File-backed stdout as well: child loop must send writes to the pool.
    file.set_len(0).expect("truncate fixture");
    file.seek(SeekFrom::Start(0)).expect("seek");
    let mut child = Command::new(env!("CARGO_BIN_EXE_native_child"))
        .arg("stdio")
        .stdin(ChildStdio::piped())
        .stdout(ChildStdio::from(file.try_clone().expect("dup stdout")))
        .stderr(ChildStdio::null())
        .spawn()
        .expect("spawn file stdout");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(b"regular file stdout\n")
        .expect("send");
    assert_eq!(child.wait().expect("wait").code(), Some(23));
    file.seek(SeekFrom::Start(0)).expect("seek");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).expect("read file");
    assert_eq!(bytes, b"regular file stdout\n");
    std::fs::remove_file(path).expect("remove fixture");
}
#[test]
fn descriptor_roundtrip_through_a_spawned_process() {
    let path = std::env::temp_dir().join(format!("tl-process-ipc-{}.sock", std::process::id()));
    turnloop_contract::native_surface::ipc_process::<backend::Platform>(
        std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")),
        &PipeName(path.clone()),
    );
    std::fs::remove_file(path).expect("remove socket path");
}
#[test]
fn spawn_options_kill_and_close_reap_owned_children() {
    use std::time::Duration;
    let mut l = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.args = vec!["environment".into(), "argument with spaces".into()];
    spec.env_clear = true;
    spec.env = vec![("TURNLOOP_CHILD_VALUE".into(), "value with spaces".into())];
    let cwd = std::env::temp_dir().canonicalize().expect("temp directory");
    spec.cwd = Some(cwd.clone());
    spec.stdio = [ProcessStdio::Null, ProcessStdio::Pipe, ProcessStdio::Null];
    // SAFETY: getuid/getgid only return current process credentials.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    spec.uid = Some(uid);
    spec.gid = Some(gid);
    let child = l.spawn(&spec, Token(1)).expect("spawn configured child");
    l.read_start(child.stdout.expect("stdout"), Token(2))
        .expect("read output");
    let until = l.now() + Duration::from_secs(5);
    let mut out = Completions::default();
    let mut bytes = Vec::new();
    let mut eof = 0;
    let mut exits = 0;
    while eof == 0 || exits == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease: Some(b) } => {
                    assert!(n > 0);
                    bytes.extend_from_slice(b.as_slice());
                }
                OpResult::Eof => eof += 1,
                OpResult::Exited(status) => {
                    assert_eq!(status.code, Some(0));
                    exits += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(
        String::from_utf8(bytes).expect("output"),
        format!("argument with spaces|value with spaces|{}", cwd.display())
    );
    for close in [false, true] {
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.args.push("sleep".into());
        let child = l.spawn(&spec, Token(3)).expect("sleeping child");
        if close {
            l.close(child.handle, Token(4)).expect("close live child");
        } else {
            l.kill(child.handle, Signal::Kill).expect("kill child");
        }
        let mut exit = 0;
        let mut cancelled = 0;
        let mut closed = 0;
        while if close { closed == 0 } else { exit == 0 } {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                match c.result {
                    OpResult::Exited(status) => {
                        assert_eq!(status.signal, Some(libc::SIGKILL));
                        exit += 1;
                    }
                    OpResult::Cancelled => cancelled += 1,
                    OpResult::Closed => {
                        assert_eq!(cancelled, 1);
                        closed += 1;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        assert_eq!(
            (exit, cancelled, closed),
            if close { (0, 1, 1) } else { (1, 0, 0) }
        );
        let mut status = 0;
        assert_eq!(
            // SAFETY: WNOHANG query of this fixture child only. ECHILD proves the
            // library already reaped it, rather than leaving a zombie behind.
            unsafe { libc::waitpid(child.pid as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}

#[test]
fn process_drop_reaps_and_signal_drop_restores_disposition() {
    let child = {
        let mut l = Loop::new(Config::default()).expect("loop");
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.args.push("sleep".into());
        l.spawn(&spec, Token(1)).expect("owned child")
    };
    let mut code = 0;
    assert_eq!(
        // SAFETY: this fixture owns the child identity, and WNOHANG never blocks.
        unsafe { libc::waitpid(child.pid as i32, &mut code, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    let action = || {
        // SAFETY: sigaction is plain C output storage.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        assert_eq!(
            // SAFETY: query only, leaving the process disposition unchanged.
            unsafe { libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut action) },
            0
        );
        action
    };
    let before = action();
    {
        let mut l = Loop::new(Config::default()).expect("signal loop");
        l.signal_start(Signal::Hup, Token(2))
            .expect("subscribe HUP");
        assert_ne!(
            action().sa_sigaction,
            before.sa_sigaction,
            "dispatcher handler installed"
        );
    }
    let after = action();
    assert_eq!(after.sa_sigaction, before.sa_sigaction);
    // Linux may synthesize SA_RESTORER; compare the documented handler flags.
    let mask = libc::SA_RESTART | libc::SA_NOCLDSTOP | libc::SA_NOCLDWAIT | libc::SA_SIGINFO;
    assert_eq!(after.sa_flags & mask, before.sa_flags & mask);
}

#[test]
fn queued_file_writes_preserve_order_and_close_quiesces_buffers() {
    use std::{
        io::{Read, Seek, SeekFrom},
        os::fd::OwnedFd,
    };
    let path = std::env::temp_dir().join(format!("tl-file-order-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("file");
    let mut l = Loop::new(Config::default()).expect("loop");
    let fd: OwnedFd = file.try_clone().expect("dup file").into();
    let h = l
        .attach(Detached::from_fd(fd).expect("file transport"), Token(1))
        .expect("attach");
    for i in 0..64 {
        l.write(h, WriteBuf::Owned(vec![i; 32]), Token(i as u64))
            .expect("queued write");
    }
    let mut writes = 0;
    let mut out = Completions::with_capacity(3);
    let until = l.now() + std::time::Duration::from_secs(5);
    while writes != 64 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("file turn");
        for c in out.drain() {
            assert_eq!(c.token, Token(writes));
            assert!(matches!(c.result, OpResult::Wrote(32)));
            writes += 1;
        }
    }
    file.seek(SeekFrom::Start(0)).expect("seek");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).expect("read bytes");
    assert_eq!(bytes.len(), 2048);
    for (i, chunk) in bytes.as_chunks::<32>().0.iter().enumerate() {
        assert_eq!(*chunk, [i as u8; 32]);
    }
    file.seek(SeekFrom::Start(0))
        .expect("rewind for provided read");
    let mut provided = vec![0; 4096];
    // SAFETY: storage stays fixed through loop destruction, which must quiesce
    // pending or running pool-backed I/O before returning.
    let buf = unsafe { IoBufMut::from_raw_parts(provided.as_mut_ptr(), provided.len()) };
    l.read(h, ReadBuf::Provided(buf), Token(100))
        .expect("provided file read");
    l.turn(Timeout::Now, &mut out).expect("start worker");
    drop(l);
    provided.fill(37);
    assert!(provided.iter().all(|&b| b == 37));
    std::fs::remove_file(path).expect("remove file");
}

#[test]
fn sigchld_subscription_cooperates_with_owned_child_reaping() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let signal = l.signal_start(Signal::Chld, Token(1)).expect("SIGCHLD");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.args.push("sleep".into());
    let live = l.spawn(&spec, Token(2)).expect("live child");
    spec.args[0] = "exit".into();
    let exited = l.spawn(&spec, Token(3)).expect("exiting child");
    let until = l.now() + std::time::Duration::from_secs(5);
    let (mut notification, mut exit) = (0, 0);
    let mut out = Completions::default();
    while notification == 0 || exit == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out)
            .expect("exit and signal");
        for c in out.drain() {
            match c.result {
                OpResult::Signal(Signal::Chld) => {
                    assert_eq!(c.token, Token(1));
                    notification += 1;
                }
                OpResult::Exited(status) => {
                    assert_eq!(c.handle, Some(exited.handle));
                    assert_eq!(status.code, Some(23));
                    exit += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(exit, 1);
    l.signal_stop(signal, Token(4))
        .expect("stop public subscription");
    let mut closed = false;
    while !closed {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("stop turn");
        for c in out.drain() {
            match c.result {
                OpResult::Stopped => {}
                OpResult::Closed => closed = true,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    l.kill(live.handle, Signal::Kill)
        .expect("terminate remaining child");
    let mut reaped = 0;
    while reaped == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out)
            .expect("remaining child exit");
        for c in out.drain() {
            assert_eq!(c.handle, Some(live.handle));
            let OpResult::Exited(status) = c.result else {
                panic!("missing remaining child exit")
            };
            assert_eq!(status.signal, Some(libc::SIGKILL));
            reaped += 1;
        }
    }
    assert_eq!(reaped, 1);
}
