//! Executable fixture: the parent asserts its output and termination status.
use std::{
    io::{Read, Write},
    time::Duration,
};
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let mode = args.get(1).and_then(|s| s.to_str()).expect("fixture mode");
    match mode {
        "exit" => std::process::exit(23),
        "sleep" => std::thread::sleep(Duration::from_secs(60)),
        "environment" => {
            print!(
                "{}|{}|{}",
                args[2].to_string_lossy(),
                std::env::var("TURNLOOP_CHILD_VALUE").expect("child env"),
                std::env::current_dir().expect("child cwd").display()
            );
        }
        "grandchild" => {
            let mut child = std::process::Command::new(&args[0])
                .arg("sleep")
                .spawn()
                .expect("spawn grandchild");
            println!("grandchild:{}", child.id());
            std::io::stdout().flush().expect("flush ready");
            let _ = child.wait();
        }
        "stdio" => native_stdio(),
        "handle" => native_handle(args.get(2).expect("pipe path")),
        #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
        "blocked-signal" => blocked_signal(),
        "copy" => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .read_to_end(&mut bytes)
                .expect("read stdin");
            std::io::stdout().write_all(&bytes).expect("stdout");
        }
        _ => panic!("unknown fixture mode"),
    }
}

#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
fn blocked_signal() {
    // Run in a fresh process so every thread (including the lazy dispatcher)
    // inherits a blocked SIGUSR1. Kqueue still observes signal generation.
    // SAFETY: initialized sigset storage passed only to the signal-set APIs.
    let (mut mask, mut old): (libc::sigset_t, libc::sigset_t) = unsafe { std::mem::zeroed() };
    // SAFETY: valid sets, catchable signal, and this thread's mask only.
    unsafe {
        assert_eq!(libc::sigemptyset(&mut mask), 0);
        assert_eq!(libc::sigaddset(&mut mask, libc::SIGUSR1), 0);
        assert_eq!(libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut old), 0);
    }
    turnloop_contract::native_surface::signal_fanout::<turnloop::backend::Platform>(
        turnloop::Signal::Usr1,
        || {
            // SAFETY: all four loops subscribed before sending to this process.
            assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
        },
    );
    // All subscriptions have stopped and the original disposition is restored.
    // The former no-op handler left SIGUSR1 pending here and unblocking killed us.
    assert_eq!(
        // SAFETY: restore precisely the initial mask on the same thread.
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) },
        0
    );
    println!("four deliveries, four stops, four closes; survived unblock");
}
#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
))]
fn native_stdio() {
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("child loop");
    let input = l.open_stdio(Stdio::Stdin).expect("child stdin");
    let output = l.open_stdio(Stdio::Stdout).expect("child stdout");
    let error = l.open_stdio(Stdio::Stderr).expect("child stderr");
    l.read(input, ReadBuf::Pooled, Token(1))
        .expect("child read");
    let mut out = Completions::default();
    let mut eof = false;
    let mut pending = 0;
    let deadline = l.now() + Duration::from_secs(5);
    while !eof || pending != 0 {
        assert!(l.now() < deadline, "child timed out");
        l.turn(Timeout::Until(deadline), &mut out)
            .expect("child turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read {
                    n,
                    lease: Some(bytes),
                } => {
                    assert!(n > 0);
                    l.write(output, WriteBuf::Owned(bytes.as_slice().to_vec()), Token(2))
                        .expect("child stdout write");
                    l.write(error, WriteBuf::Owned(bytes.as_slice().to_vec()), Token(3))
                        .expect("child stderr write");
                    pending += 2;
                    l.read(input, ReadBuf::Pooled, Token(1))
                        .expect("child next read");
                }
                OpResult::Wrote(n) => {
                    assert!(n > 0);
                    pending -= 1;
                }
                OpResult::Eof => eof = true,
                other => panic!("child unexpected {other:?}"),
            }
        }
    }
    std::process::exit(23);
}
#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
)))]
fn native_stdio() {
    panic!("instantiate stdio fixture on production native backend");
}
#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
))]
fn native_handle(path: &std::ffi::OsStr) {
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("child loop");
    let pipe = l
        .pipe_connect(&PipeName(path.into()), Token(1))
        .expect("child IPC connect");
    let mut out = Completions::default();
    let mut sent = false;
    let mut wrote = false;
    let deadline = l.now() + Duration::from_secs(5);
    while !sent || !wrote {
        assert!(l.now() < deadline, "child IPC timeout");
        l.turn(Timeout::Until(deadline), &mut out)
            .expect("child turn");
        for c in out.drain() {
            match c.result {
                OpResult::Connected => {
                    l.recv_handle(pipe, Token(2)).expect("child receive");
                }
                OpResult::HandleReceived { handle } => {
                    let detached = l.detach(handle).expect("child detach");
                    let handle = l.attach(detached, Token(3)).expect("child attach");
                    l.write(
                        handle,
                        WriteBuf::Owned(b"cross-process socket".to_vec()),
                        Token(4),
                    )
                    .expect("child write");
                    l.send_handle(pipe, handle, Token(5))
                        .expect("child send back");
                }
                OpResult::HandleSent => sent = true,
                OpResult::Wrote(n) => {
                    assert_eq!(n, 20);
                    wrote = true;
                }
                other => panic!("child IPC unexpected {other:?}"),
            }
        }
    }
}
#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
)))]
fn native_handle(_: &std::ffi::OsStr) {
    panic!("instantiate handle fixture on production native backend");
}
