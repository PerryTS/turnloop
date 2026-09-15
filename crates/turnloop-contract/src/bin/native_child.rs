//! Executable fixture: the parent asserts its output and termination status.
use std::{
    io::{Read, Write},
    time::Duration,
};
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let mode = args.get(1).and_then(|s| s.to_str()).expect("fixture mode");
    match mode {
        #[cfg(any(
            target_vendor = "apple",
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd"
        ))]
        "services-no-spin" => {
            turnloop_contract::native_surface::services_no_spin::<turnloop::backend::Platform>(
                &args[0],
                turnloop::Signal::Usr2,
            );
            println!("60 service timer expiries; no-spin bounds passed");
        }
        "exit" => std::process::exit(23),
        "exit-with" => std::process::exit(
            args[2]
                .to_str()
                .and_then(|code| code.parse().ok())
                .expect("exit code"),
        ),
        "sleep" => std::thread::sleep(Duration::from_secs(60)),
        "roundtrip" => {
            let mut stdout = std::io::stdout().lock();
            let values = args[2..].iter().cloned().chain([
                std::env::var_os("TURNLOOP_CHILD_VALUE").expect("child value"),
                std::env::var_os("TURNLOOP_CHILD_OTHER").expect("child other"),
                std::env::current_dir()
                    .expect("cwd")
                    .canonicalize()
                    .expect("canonical cwd")
                    .into_os_string(),
                std::env::var_os("PATH").unwrap_or_else(|| "<absent>".into()),
            ]);
            for value in values {
                let value = value.to_str().expect("UTF-8 fixture value");
                writeln!(stdout, "{}", value.len()).expect("field length");
                stdout.write_all(value.as_bytes()).expect("field bytes");
            }
        }
        "environment" => {
            print!(
                "{}|{}|{}",
                args[2].to_string_lossy(),
                std::env::var("TURNLOOP_CHILD_VALUE").expect("child env"),
                std::env::current_dir().expect("child cwd").display()
            );
        }
        #[cfg(unix)]
        "session" => {
            // SAFETY: queries this process only; no pointer arguments or mutation.
            let (pid, group, session) =
                unsafe { (libc::getpid(), libc::getpgrp(), libc::getsid(0)) };
            println!("{pid}:{group}:{session}");
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
        #[cfg(windows)]
        "console-probe" => console_probe(&args),
        #[cfg(windows)]
        "lifetime-parent" => lifetime_parent(&args),
        #[cfg(windows)]
        "orphan-leader" => {
            let child = std::process::Command::new(&args[0])
                .arg("sleep")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("orphan grandchild");
            println!("grandchild:{}", child.id());
            std::io::stdout().flush().expect("grandchild identity");
            // Handshake keeps the leader alive until the observer pins the grandchild.
            std::io::stdin()
                .read_exact(&mut [0])
                .expect("leader exit permission");
            std::process::exit(23);
        }
        "stdio" => native_stdio(),
        "handle" => native_handle(args.get(2).expect("pipe path")),
        #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
        "blocked-signal" => blocked_signal(),
        // Both extra descriptors carry bytes in both directions. The channel
        // number is read out of the environment exactly as Node reads
        // NODE_CHANNEL_FD, so this proves the handoff, not a hard-coded 3.
        #[cfg(any(unix, windows))]
        "channel" => {
            let channel = numbered_fd("NODE_CHANNEL_FD");
            let extra = numbered_fd("TURNLOOP_EXTRA_FD");
            let mut request = [0u8; 6];
            read_exact(channel, &mut request);
            assert_eq!(&request, b"ping-3", "channel request");
            write_all(channel, b"pong-3");
            read_exact(extra, &mut request);
            assert_eq!(&request, b"ping-4", "extra request");
            write_all(extra, b"pong-4");
            print!("ok");
            std::io::stdout().flush().expect("flush transcript");
        }
        // One-way pipe, null device and an adopted parent transport.
        #[cfg(any(unix, windows))]
        "extra-sources" => {
            let (one_way, null, adopted) = (fd_stream(3), fd_stream(4), fd_stream(5));
            write_all(one_way, b"three");
            write_all(null, b"void");
            let mut discard = [0u8; 4];
            assert_eq!(read_once(null, &mut discard), 0, "null device reads EOF");
            write_all(adopted, b"five");
            print!("ok");
            std::io::stdout().flush().expect("flush transcript");
        }
        #[cfg(unix)]
        "tty-session" => {
            // SAFETY: queries this process only; no pointer arguments or mutation.
            let (pid, group, session) =
                unsafe { (libc::getpid(), libc::getpgrp(), libc::getsid(0)) };
            // SAFETY: constant device path with integer flags; -1 means no
            // controlling terminal, which is the distinction under test.
            let terminal = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR) };
            let foreground = if terminal < 0 {
                -1
            } else {
                // SAFETY: live descriptor on this process's controlling terminal.
                unsafe { libc::tcgetpgrp(terminal) }
            };
            if terminal >= 0 {
                // SAFETY: closing the descriptor this branch just opened.
                unsafe {
                    libc::close(terminal);
                }
            }
            println!("{pid}:{group}:{session}:{}", i32::from(terminal >= 0));
            assert_eq!(
                foreground, group,
                "child leads its terminal foreground group"
            );
        }
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

#[cfg(windows)]
fn console_probe(args: &[std::ffi::OsString]) {
    use turnloop::*;
    use windows_sys::Win32::System::{Console::*, Threading::*};
    let host: u32 = args
        .get(2)
        .and_then(|pid| pid.to_str())
        .and_then(|pid| pid.parse().ok())
        .expect("host process id");
    let mut pids = [0u32; 64];
    // SAFETY: initialized writable array and its actual element count.
    let count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } as usize;
    // Membership in the host's console, not merely having a console: CREATE_NO_WINDOW
    // gives the child its own windowless console, which host Ctrl-C does not reach.
    let own = count > 0;
    let attached = pids[..count.min(pids.len())].contains(&host);
    // SAFETY: writable C startup structure for this process.
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    // SAFETY: valid writable output structure, no retained pointers are dereferenced.
    unsafe {
        GetStartupInfoW(&mut startup);
    }
    if !attached {
        println!("console:false,own:{own},show:{}", startup.wShowWindow);
        return;
    }
    let mut driver = Loop::new(Config::default()).expect("console child loop");
    driver
        .signal_start(Signal::Int, Token(91))
        .expect("console child signal");
    // SAFETY: enable Ctrl-C only in this fixture process, after subscription.
    assert_ne!(unsafe { SetConsoleCtrlHandler(None, 0) }, 0);
    println!("console:true,own:{own},show:{}", startup.wShowWindow);
    std::io::stdout().flush().expect("ready for broadcast");
    let mut out = Completions::default();
    let deadline = driver.now() + Duration::from_secs(10);
    loop {
        assert!(driver.now() < deadline, "console child missed broadcast");
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("child signal turn");
        if let Some(c) = out.drain().next() {
            assert_eq!(c.token, Token(91));
            assert!(matches!(c.result, OpResult::Signal(Signal::Int)));
            println!("ctrl-c-received");
            return;
        }
    }
}

#[cfg(windows)]
fn lifetime_parent(args: &[std::ffi::OsString]) {
    use turnloop::*;
    let mut driver = Loop::new(Config::default()).expect("lifetime parent loop");
    let mut spec = ProcessSpec::new(&args[0]);
    spec.args.push("sleep".into());
    spec.stdio = [ProcessStdio::Null; 3];
    spec.windows_hide = true;
    spec.detached = args[2] == "detached";
    let child = driver.spawn(&spec, Token(1)).expect("lifetime child");
    println!("child:{}", child.pid);
    std::io::stdout().flush().expect("child identity");
    // Pinning the child before parent exit prevents all PID-reuse races.
    std::io::stdin()
        .read_exact(&mut [0])
        .expect("parent exit permission");
    if args[3] == "exit" {
        std::process::exit(23); // OS closes the process-wide job, without Rust Drop
    }
    std::thread::sleep(Duration::from_secs(60)); // observer terminates this parent
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
    target_os = "freebsd",
    target_os = "windows"
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
    target_os = "freebsd",
    target_os = "windows"
)))]
fn native_stdio() {
    panic!("instantiate stdio fixture on production native backend");
}
#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "windows"
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
    target_os = "freebsd",
    target_os = "windows"
)))]
fn native_handle(_: &std::ffi::OsStr) {
    panic!("instantiate handle fixture on production native backend");
}

/// A child descriptor as this platform's stream identity: the number itself on
/// Unix, and the C run-time's handle for it on Windows, which is precisely how
/// libuv's `uv_pipe_open` turns Node's `NODE_CHANNEL_FD` into a usable stream.
#[cfg(unix)]
type FdStream = std::os::fd::RawFd;
#[cfg(windows)]
type FdStream = windows_sys::Win32::Foundation::HANDLE;

#[cfg(windows)]
unsafe extern "C" {
    fn _get_osfhandle(fd: i32) -> isize;
}

#[cfg(any(unix, windows))]
fn fd_stream(fd: i32) -> FdStream {
    #[cfg(unix)]
    {
        fd
    }
    #[cfg(windows)]
    {
        // SAFETY: the C run-time owns its descriptor table; this only reads it.
        let handle = unsafe { _get_osfhandle(fd) };
        assert!(handle > 0, "descriptor {fd} is absent from the C run-time");
        handle as FdStream
    }
}

#[cfg(any(unix, windows))]
fn numbered_fd(variable: &str) -> FdStream {
    let value = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is unset"));
    fd_stream(value.parse().expect("descriptor number"))
}

#[cfg(any(unix, windows))]
fn read_once(stream: FdStream, buffer: &mut [u8]) -> usize {
    #[cfg(unix)]
    {
        // SAFETY: live inherited descriptor and a writable buffer of that length.
        let n = unsafe { libc::read(stream, buffer.as_mut_ptr().cast(), buffer.len()) };
        assert!(n >= 0, "read: {}", std::io::Error::last_os_error());
        n as usize
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, Storage::FileSystem::ReadFile};
        let mut read = 0;
        // SAFETY: live inherited handle, writable buffer of the stated length,
        // and a synchronous handle so no OVERLAPPED is required.
        let ok = unsafe {
            ReadFile(
                stream,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_BROKEN_PIPE as i32),
                "read: {error}"
            );
            return 0;
        }
        read as usize
    }
}

#[cfg(any(unix, windows))]
fn read_exact(stream: FdStream, buffer: &mut [u8]) {
    let mut filled = 0;
    while filled < buffer.len() {
        let n = read_once(stream, &mut buffer[filled..]);
        assert_ne!(n, 0, "premature end of stream after {filled} bytes");
        filled += n;
    }
}

#[cfg(any(unix, windows))]
fn write_all(stream: FdStream, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        #[cfg(unix)]
        // SAFETY: live inherited descriptor and a readable buffer of that length.
        let n = unsafe { libc::write(stream, bytes.as_ptr().cast(), bytes.len()) };
        #[cfg(unix)]
        assert!(n > 0, "write: {}", std::io::Error::last_os_error());
        #[cfg(unix)]
        let n = n as usize;
        #[cfg(windows)]
        let n = {
            let mut wrote = 0;
            // SAFETY: live inherited handle, readable buffer of the stated
            // length, and a synchronous handle so no OVERLAPPED is required.
            let ok = unsafe {
                windows_sys::Win32::Storage::FileSystem::WriteFile(
                    stream,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut wrote,
                    std::ptr::null_mut(),
                )
            };
            assert_ne!(ok, 0, "write: {}", std::io::Error::last_os_error());
            wrote as usize
        };
        bytes = &bytes[n..];
    }
}
