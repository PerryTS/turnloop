#![cfg(all(windows, not(loom)))]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io::Write,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::Mutex,
    time::Duration,
};

// GetProcessHandleCount measures the entire test process. Every test in this
// binary must hold this guard until its handles and helper threads are dropped.
static HANDLES: Mutex<()> = Mutex::new(());
use turnloop::*;
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Pipes::CreatePipe,
        Threading::{
            GetCurrentProcess, GetProcessHandleCount, OpenProcess, PROCESS_SYNCHRONIZE,
            WaitForSingleObject,
        },
    },
};

fn child_wait_handle(pid: u32) -> OwnedHandle {
    // SAFETY: the driver still owns this child, preventing PID reuse. Only wait access.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(
        !handle.is_null(),
        "open owned child: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: successful OpenProcess transfers unique ownership.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    handle.try_clone().expect("duplicate child wait handle")
}

fn assert_cancelled_then_closed(driver: &mut Loop, child: Handle, wait: &OwnedHandle) {
    let mut out = Completions::with_capacity(1);
    let deadline = driver.now() + Duration::from_secs(10);
    let mut count = 0;
    while driver.alive() {
        assert!(driver.now() < deadline, "child close deadline");
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("close turn");
        for completion in out.drain() {
            assert_eq!(completion.handle, Some(child));
            assert!(completion.terminal);
            match count {
                0 => {
                    assert_eq!(completion.token, Token(1));
                    assert!(completion.op.is_some());
                    assert!(matches!(completion.result, OpResult::Cancelled));
                }
                1 => {
                    assert_eq!(completion.token, Token(2));
                    assert!(completion.op.is_none());
                    assert!(matches!(completion.result, OpResult::Closed));
                    assert_eq!(
                        // SAFETY: duplicated child handle is live for this nonblocking query.
                        unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
                        WAIT_OBJECT_0,
                        "Closed must follow child termination"
                    );
                }
                _ => panic!("duplicate child completion: {completion:?}"),
            }
            count += 1;
        }
    }
    assert_eq!(count, 2);
    driver
        .turn(Timeout::Now, &mut out)
        .expect("check duplicates");
    assert!(out.is_empty());
}

#[test]
fn kill_then_close_children_completes_once() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut completed = 0;
    for group in [false, true] {
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.windows_hide = true;
        spec.args.push("sleep".into());
        spec.stdio = [ProcessStdio::Null; 3];
        spec.new_process_group = group;
        for _ in 0..200 {
            let child = driver.spawn(&spec, Token(1)).expect("sleeping child");
            let wait = child_wait_handle(child.pid);
            assert_eq!(
                // SAFETY: owned duplicate, zero-timeout liveness query.
                unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
                WAIT_TIMEOUT,
                "kill subject must still be alive"
            );
            if group {
                driver
                    .kill_group(child.handle, Signal::Kill)
                    .expect("kill tree");
            } else {
                driver.kill(child.handle, Signal::Kill).expect("kill child");
            }
            driver
                .close(child.handle, Token(2))
                .expect("close after kill");
            assert_cancelled_then_closed(&mut driver, child.handle, &wait);
            completed += 1;
        }
    }
    assert_eq!(completed, 400);
}

#[test]
fn close_after_raw_child_wait_before_servicing_exit() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.windows_hide = true;
    spec.args.push("exit".into());
    spec.stdio = [ProcessStdio::Null; 3];
    let child = driver
        .spawn(&spec, Token(1))
        .expect("immediately exiting child");
    let wait = child_wait_handle(child.pid);
    assert_eq!(
        // SAFETY: duplicated process handle, bounded wait without servicing the loop.
        unsafe { WaitForSingleObject(wait.as_raw_handle(), 10_000) },
        WAIT_OBJECT_0
    );
    assert_eq!(
        driver
            .kill(child.handle, Signal::Kill)
            .expect_err("already exited")
            .kind,
        ErrorKind::NotFound
    );
    driver
        .close(child.handle, Token(2))
        .expect("close before exit turn");
    assert_cancelled_then_closed(&mut driver, child.handle, &wait);
}

#[test]
fn batch_programs_are_rejected_before_spawn() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory = std::env::temp_dir().join(format!("turnloop-batch-{}", std::process::id()));
    std::fs::create_dir(&directory).expect("private batch directory");
    let marker = directory.join("marker");
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    let mut rejected = 0;
    for extension in ["cmd", "CmD", "bat", "BaT"] {
        let name = format!("fixture.{extension}");
        let path = directory.join(&name);
        std::fs::write(&path, "@echo executed>\"%~dp0marker\"\r\n").expect("batch fixture");
        for program in [path.as_os_str(), std::ffi::OsStr::new(&name)] {
            let mut spec = ProcessSpec::new(program);
            spec.windows_hide = true;
            spec.env
                .push(("PATH".into(), directory.as_os_str().to_owned()));
            spec.args.push("\"&echo injected".into());
            assert_eq!(
                driver
                    .spawn(&spec, Token(1))
                    .expect_err("reject batch")
                    .kind,
                ErrorKind::InvalidInput
            );
            assert!(
                !driver.alive(),
                "failed spawn retained a child or operation"
            );
            driver.turn(Timeout::Now, &mut out).expect("empty turn");
            assert!(out.is_empty(), "failed spawn produced a child completion");
            assert!(!marker.exists(), "batch ran despite rejection");
            rejected += 1;
        }
        // Positive control: the same file really creates the marker when the
        // caller explicitly opts into cmd.exe and its shell parsing.
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c"])
            .arg(&path)
            .status()
            .expect("explicit batch shell");
        assert!(status.success());
        assert_eq!(std::fs::read(&marker).expect("marker"), b"executed\r\n");
        std::fs::remove_file(&marker).expect("remove control marker");
        std::fs::remove_file(&path).expect("remove batch");
    }
    assert_eq!(rejected, 8);
    std::fs::remove_dir(directory).expect("remove private batch directory");
}

#[test]
fn imported_overlapped_pipe_routes_away_from_its_existing_port() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::FILE_FLAG_OVERLAPPED,
        System::IO::CreateIoCompletionPort,
    };
    let mut driver = Loop::new(Config::default()).expect("loop");
    let name = format!(r"\\.\pipe\turnloop-import-{}", std::process::id());
    let listener = driver
        .pipe_listen(&PipeName(name.clone().into()), &ListenOpts::default())
        .expect("listen");
    driver.accept(listener, Token(1)).expect("accept");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(name)
        .expect("overlapped client");
    // SAFETY: creates a new, unassociated completion port with no pointer retention.
    let port = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1) };
    assert!(!port.is_null());
    // SAFETY: successful port creation transferred sole ownership.
    let port = unsafe { OwnedHandle::from_raw_handle(port) };
    // SAFETY: owned overlapped pipe and live port; this initial association is permanent.
    let associated =
        unsafe { CreateIoCompletionPort(file.as_raw_handle(), port.as_raw_handle(), 9, 0) };
    assert_eq!(associated, port.as_raw_handle());
    let client = driver
        .attach(
            Detached::from_handle(file.into()).expect("classify pipe"),
            Token(2),
        )
        .expect("attach migrated pipe");
    let deadline = driver.now() + Duration::from_secs(3);
    let mut out = Completions::default();
    let server = loop {
        assert!(driver.now() < deadline, "accept timeout");
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("turn");
        if let Some(c) = out.drain().next() {
            if let OpResult::PipeAccepted { conn } = c.result {
                break conn;
            }
            panic!("unexpected {c:?}");
        }
    };
    turnloop_contract::native_surface::transfer(&mut driver, server, client, b"routed read");
    turnloop_contract::native_surface::transfer(&mut driver, client, server, b"routed write");
}

#[test]
fn overlapped_regular_files_are_rejected_before_worker_submission() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(std::env::current_exe().expect("test executable"))
        .expect("overlapped file");
    assert!(matches!(
        Detached::from_handle(file.into()),
        Err(Error {
            kind: ErrorKind::Unsupported,
            ..
        })
    ));
}
fn handles() -> u32 {
    let mut count = 0;
    // SAFETY: current-process pseudo handle and writable count output.
    assert_ne!(
        // SAFETY: current-process pseudo handle and writable count output.
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) },
        0
    );
    count
}
#[test]
fn cancelled_synchronous_reads_release_buffers_and_threads() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: two output slots; noninherited anonymous synchronous pipe.
    assert_ne!(
        // SAFETY: two output slots; noninherited anonymous synchronous pipe.
        unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 4096) },
        0
    );
    // SAFETY: successful CreatePipe transferred two independent handle owners.
    let (read, write) = unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    };
    let mut writer = std::fs::File::from(write);
    let mut checked = 0;
    for round in 0..128 {
        let mut memory = [0xa5; 64]; // outlives backend on every unwind/drop path
        let mut driver = Loop::new(Config {
            max_handles: 4,
            max_operations: 4,
            pooled_buffers: 2,
            ..Config::default()
        })
        .expect("loop");
        let h = driver
            .attach(
                Detached::from_handle(read.try_clone().expect("dup read")).expect("stdio"),
                Token(1),
            )
            .expect("attach");
        // SAFETY: buffer stays fixed/exclusive until cancellation or completed driver destruction.
        let buf = unsafe { IoBufMut::from_raw_parts(memory.as_mut_ptr(), memory.len()) };
        driver
            .read(h, ReadBuf::Provided(buf), Token(2))
            .expect("read");
        let mut out = Completions::with_capacity(1);
        driver
            .turn(Timeout::Now, &mut out)
            .expect("start synchronous read");
        assert!(out.is_empty());
        if round % 3 == 0 {
            std::thread::yield_now();
        }
        if round % 2 == 0 {
            driver.close(h, Token(3)).expect("close blocked stdio");
            let deadline = driver.now() + Duration::from_secs(3);
            let mut cancelled = false;
            while driver.alive() {
                assert!(driver.now() < deadline, "synchronous cancellation race");
                driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("cancel turn");
                for c in out.drain() {
                    match c.result {
                        OpResult::Cancelled => {
                            assert!(!cancelled);
                            cancelled = true;
                        }
                        OpResult::Closed => {
                            assert!(cancelled, "buffer acknowledgement before close")
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                }
            }
            assert!(cancelled);
        }
        drop(driver);
        let payload = [round as u8; 32];
        writer
            .write_all(&payload)
            .expect("write after cancellation/drop");
        let mut fresh = Loop::new(Config {
            max_handles: 4,
            max_operations: 4,
            pooled_buffers: 2,
            ..Config::default()
        })
        .expect("fresh reader loop");
        let h = fresh
            .attach(
                Detached::from_handle(read.try_clone().expect("fresh read handle"))
                    .expect("fresh pipe"),
                Token(4),
            )
            .expect("fresh reader");
        let op = fresh
            .read(h, ReadBuf::Pooled, Token(5))
            .expect("fresh read");
        let deadline = fresh.now() + Duration::from_secs(3);
        while out.is_empty() {
            assert!(
                fresh.now() < deadline,
                "cancelled read consumed the new data"
            );
            fresh
                .turn(Timeout::Until(deadline), &mut out)
                .expect("fresh read turn");
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op, Some(op));
        assert_eq!(out[0].handle, Some(h));
        let OpResult::Read {
            n,
            lease: Some(lease),
        } = &out[0].result
        else {
            panic!("missing fresh read: {:?}", out[0]);
        };
        assert_eq!(*n, payload.len());
        assert_eq!(
            lease.as_slice(),
            payload,
            "cancelled read must not consume future bytes"
        );
        assert_eq!(memory, [0xa5; 64]);
        checked += 1;
    }
    assert_eq!(checked, 128);
}
#[test]
fn loop_drop_and_stale_wakers_release_windows_handles() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cycle = || {
        let mut driver = Loop::new(Config::default()).expect("loop");
        let (_, _, receiver) = turnloop_contract::pair(&mut driver);
        driver
            .read(receiver, ReadBuf::Pooled, Token(5))
            .expect("read");
        driver
            .turn(Timeout::Now, &mut Completions::default())
            .expect("arm");
        let notifier = driver.notifier();
        let poster = driver.poster();
        drop(driver);
        assert!(matches!(
            notifier.notify(),
            Err(Error {
                kind: ErrorKind::NotFound,
                ..
            })
        ));
        let error = poster
            .post(Token(6), Payload::U64(7))
            .expect_err("closed poster");
        assert_eq!(error.error.kind, ErrorKind::NotFound);
        assert!(matches!(error.payload, Some(Payload::U64(7))));
    };
    cycle(); // process-lifetime Winsock/runtime setup precedes the baseline
    let baseline = handles();
    for _ in 0..32 {
        cycle();
        assert_eq!(handles(), baseline, "native handle leak");
    }
}

#[test]
fn cancelled_child_watch_completes_while_child_is_alive() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.windows_hide = true;
    spec.args.push("sleep".into());
    spec.stdio = [ProcessStdio::Null; 3];
    let child = driver.spawn(&spec, Token(1)).expect("sleeping child");
    let wait = child_wait_handle(child.pid);
    // Process does not expose its implicit exit OpId. detach first cancels that
    // watch and returns WouldBlock until its acknowledgement has been delivered.
    assert_eq!(
        driver
            .detach(child.handle)
            .expect_err("cancel pending exit watch")
            .kind,
        ErrorKind::WouldBlock
    );
    let mut out = Completions::default();
    let info = driver
        .turn(Timeout::After(Duration::from_secs(2)), &mut out)
        .expect("one cancellation turn");
    assert_eq!(info.os_waits, 0, "cancelled watch is immediately ready");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].handle, Some(child.handle));
    assert_eq!(out[0].token, Token(1));
    assert!(out[0].terminal);
    assert!(matches!(out[0].result, OpResult::Cancelled));
    assert!(!driver.cancel(out[0].op.expect("exit operation")));
    assert_eq!(
        // SAFETY: live duplicated process handle, zero timeout checks actual liveness.
        unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
        WAIT_TIMEOUT,
        "watch cancellation must not terminate the child"
    );
    driver.kill(child.handle, Signal::Kill).expect("kill child");
    assert_eq!(
        // SAFETY: bounded wait on the owned duplicate, without servicing an exit watch.
        unsafe { WaitForSingleObject(wait.as_raw_handle(), 10_000) },
        WAIT_OBJECT_0
    );
    driver.close(child.handle, Token(2)).expect("close child");
    driver.turn(Timeout::Now, &mut out).expect("close turn");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].handle, Some(child.handle));
    assert_eq!(out[0].token, Token(2));
    assert!(matches!(out[0].result, OpResult::Closed));
    assert!(!driver.alive());
    driver
        .turn(Timeout::Now, &mut out)
        .expect("no duplicate cancellation");
    assert!(out.is_empty());
}

#[test]
fn pipe_backlog_connects_before_accept_is_serviced_and_rearms() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::io::Read;
    const BACKLOG: usize = 8;
    let mut driver = Loop::new(Config::default()).expect("loop");
    let name = PipeName(format!(r"\\.\pipe\tl-backlog-{}", std::process::id()).into());
    let listener = driver
        .pipe_listen(
            &name,
            &ListenOpts {
                backlog: BACKLOG as u32,
                ..ListenOpts::default()
            },
        )
        .expect("backlog listener");
    let mut accepted = 0;
    let mut out = Completions::with_capacity(1);
    for round in 0..3 {
        // The host has not submitted or serviced an accept in this round. Every
        // client opens synchronously, proving all eight native instances exist.
        let mut clients: [std::fs::File; BACKLOG] = std::array::from_fn(|i| {
            let mut client = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name.0)
                .expect("client before accept");
            client
                .write_all(&[i as u8])
                .expect("unique client identity");
            client
        });
        let op = driver
            .accept_start(listener, Token(1))
            .expect("multishot accept");
        let deadline = driver.now() + Duration::from_secs(5);
        let mut servers = Vec::new();
        while servers.len() != BACKLOG {
            assert!(driver.now() < deadline);
            driver
                .turn(Timeout::Until(deadline), &mut out)
                .expect("accept backlog");
            for c in out.drain() {
                assert_eq!(
                    (c.op, c.handle, c.token, c.terminal),
                    (Some(op), Some(listener), Token(1), false)
                );
                let OpResult::PipeAccepted { conn } = c.result else {
                    panic!("unexpected {c:?}")
                };
                assert!(!servers.contains(&conn), "duplicate accepted handle");
                servers.push(conn);
            }
        }
        assert!(driver.cancel(op));
        driver.turn(Timeout::Now, &mut out).expect("stop accept");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op, Some(op));
        assert!(matches!(out[0].result, OpResult::Cancelled));
        let mut seen = [false; BACKLOG];
        for server in servers {
            let read = driver
                .read(server, ReadBuf::Pooled, Token(2))
                .expect("identify accepted peer");
            let peer = loop {
                assert!(driver.now() < deadline);
                driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("identity read");
                if out.is_empty() {
                    continue;
                }
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].op, Some(read));
                let OpResult::Read {
                    n: 1,
                    lease: Some(bytes),
                } = &out[0].result
                else {
                    panic!("missing identity")
                };
                let i = bytes.as_slice()[0] as usize;
                assert!(i < BACKLOG && !std::mem::replace(&mut seen[i], true));
                driver
                    .write(server, WriteBuf::Owned(vec![round, i as u8]), Token(3))
                    .expect("reply to same client");
                break i;
            };
            loop {
                assert!(driver.now() < deadline);
                driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("reply");
                if out.is_empty() {
                    continue;
                }
                assert_eq!(out.len(), 1);
                assert!(matches!(out[0].result, OpResult::Wrote(2)));
                break;
            }
            let mut reply = [0; 2];
            clients[peer]
                .read_exact(&mut reply)
                .expect("exact peer reply before server close");
            assert_eq!(reply, [round, peer as u8]);
            driver.close(server, Token(4)).expect("close server");
            driver.turn(Timeout::Now, &mut out).expect("closed");
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Closed));
            accepted += 1;
        }
        assert!(seen.into_iter().all(|v| v));
        driver
            .turn(Timeout::Now, &mut out)
            .expect("no duplicate accepts");
        assert!(out.is_empty());
    }
    assert_eq!(accepted, 24);
}

#[test]
fn busy_pipe_connect_parks_expires_cancels_and_retries() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = PipeName(format!(r"\\.\pipe\tl-busy-{}", std::process::id()).into());
    let mut server = Loop::new(Config::default()).expect("server loop");
    let listener = server
        .pipe_listen(
            &name,
            &ListenOpts {
                backlog: 1,
                ..ListenOpts::default()
            },
        )
        .expect("one instance");
    let occupied = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name.0)
        .expect("occupy sole instance");
    let mut client = Loop::new(Config::default()).expect("client loop");
    let mut out = Completions::with_capacity(1);
    let mut timeouts = 0;
    for delay in [
        Duration::from_micros(500),
        Duration::from_millis(2),
        Duration::from_millis(10),
    ] {
        let at = client.now() + delay;
        let h = client
            .pipe_connect_until(&name, at, Token(1))
            .expect("busy connection accepted");
        let mut turns = 0;
        let mut empty_waits = 0;
        loop {
            assert!(client.now() < at + Duration::from_secs(3));
            let info = client
                .turn(Timeout::Until(at + Duration::from_secs(3)), &mut out)
                .expect("park busy connect");
            turns += 1;
            empty_waits += info.zero_event_waits;
            if out.is_empty() {
                continue;
            }
            assert_eq!(out.len(), 1);
            assert_eq!(
                (out[0].handle, out[0].token, out[0].terminal),
                (Some(h), Token(1), true)
            );
            assert!(matches!(
                out[0].result,
                OpResult::Err(Error {
                    kind: ErrorKind::TimedOut,
                    ..
                })
            ));
            assert!(client.now() >= at);
            assert!(
                turns <= 2 && empty_waits <= 1,
                "busy retry spun: {turns} turns, {empty_waits} empty waits"
            );
            timeouts += 1;
            break;
        }
        assert_eq!(client.next_deadline(), None);
        assert_eq!(
            client
                .detach(h)
                .expect_err("pending-open state is not a connected transport")
                .kind,
            ErrorKind::Unsupported
        );
        client.close(h, Token(2)).expect("close expired client");
        client.turn(Timeout::Now, &mut out).expect("closed client");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Closed));
    }
    assert_eq!(timeouts, 3);
    // A host turn deadline bounds only that turn; it must leave the unbounded
    // connection pending. Replenishing the server backlog then wakes that request.
    let h = client
        .pipe_connect(&name, Token(3))
        .expect("busy unbounded connect");
    let at = client.now() + Duration::from_millis(10);
    let info = client
        .turn(Timeout::Until(at), &mut out)
        .expect("pending availability wait");
    assert_eq!((info.os_waits, info.zero_event_waits), (1, 1));
    assert!(client.now() >= at && out.is_empty());
    server
        .accept(listener, Token(4))
        .expect("consume occupied instance");
    let deadline = server.now() + Duration::from_secs(5);
    let mut accepts = 0;
    while accepts == 0 {
        assert!(server.now() < deadline);
        server
            .turn(Timeout::Until(deadline), &mut out)
            .expect("rearm listener");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::PipeAccepted { .. }));
            accepts += 1;
        }
    }
    let mut connected = 0;
    while connected == 0 {
        assert!(client.now() < deadline);
        client
            .turn(Timeout::Until(deadline), &mut out)
            .expect("availability wake");
        for c in out.drain() {
            assert_eq!((c.handle, c.token), (Some(h), Token(3)));
            assert!(matches!(c.result, OpResult::Connected));
            connected += 1;
        }
    }
    assert_eq!((accepts, connected), (1, 1));
    let cancelled = client.pipe_connect(&name, Token(5)).expect("busy again");
    client
        .turn(Timeout::Now, &mut out)
        .expect("arm cancellable wait");
    assert!(out.is_empty());
    client.close(cancelled, Token(6)).expect("cancel busy open");
    let mut count = 0;
    while count != 2 {
        assert!(client.now() < deadline);
        client
            .turn(Timeout::Until(deadline), &mut out)
            .expect("cancel/close availability");
        for c in out.drain() {
            assert_eq!(c.handle, Some(cancelled));
            assert!(match count {
                0 => matches!(c.result, OpResult::Cancelled),
                1 => matches!(c.result, OpResult::Closed),
                _ => false,
            });
            count += 1;
        }
    }
    drop(occupied);
}

#[test]
fn close_live_child_with_exit_watch_reaps_before_closed() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut completed = 0;
    for group in [false, true] {
        let mut driver = Loop::new(Config::default()).expect("loop");
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.windows_hide = true;
        spec.args.push("sleep".into());
        spec.stdio = [ProcessStdio::Null; 3];
        spec.new_process_group = group;
        let child = driver.spawn(&spec, Token(1)).expect("live child");
        let wait = child_wait_handle(child.pid);
        assert_eq!(
            // SAFETY: owned identity, nonblocking query establishes a live close subject.
            unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        driver
            .close(child.handle, Token(2))
            .expect("close without prior kill");
        assert_cancelled_then_closed(&mut driver, child.handle, &wait);
        completed += 1;
    }
    assert_eq!(completed, 2);
}

#[test]
fn loop_drop_terminates_live_children_and_releases_their_handles() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let cycle = || {
        let mut driver = Loop::new(Config::default()).expect("loop");
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.windows_hide = true;
        spec.args.push("sleep".into());
        spec.stdio = [ProcessStdio::Null; 3];
        let waits: [OwnedHandle; 8] = std::array::from_fn(|i| {
            spec.new_process_group = i % 2 == 0;
            let child = driver
                .spawn(&spec, Token(i as u64))
                .expect("owned live child");
            let wait = child_wait_handle(child.pid);
            assert_eq!(
                // SAFETY: independent owned wait handle proves the child is live before drop.
                unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
                WAIT_TIMEOUT
            );
            wait
        });
        drop(driver);
        for wait in waits {
            assert_eq!(
                // SAFETY: duplicate survives loop destruction; no blocking wait can conceal a leak.
                unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
                WAIT_OBJECT_0
            );
        }
        8
    };
    assert_eq!(cycle(), 8); // initialize Windows thread-pool wait infrastructure
    let baseline = handles();
    let mut reaped = 0;
    for _ in 0..16 {
        reaped += cycle();
        assert_eq!(
            handles(),
            baseline,
            "live-child drop leaked a native handle"
        );
    }
    assert_eq!(reaped, 128);
}

#[test]
fn child_argv_environment_and_directory_roundtrip_exactly() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let directory =
        std::env::temp_dir().join(format!("tl-argv-{}-日本語 space", std::process::id()));
    std::fs::create_dir(&directory).expect("private unicode cwd");
    let cwd = directory.canonicalize().expect("canonical cwd");
    let arguments = [
        "",
        "two words",
        "plain",
        "\"",
        "trailing\\",
        "two trailing \\\\",
        "slash\\\"quote",
        "x\"\"y",
        "日本語 🦀",
        "\\\\server\\dir with spaces\\",
        "tab\tline\nend",
    ];
    let value = "value with spaces, \"quotes\", 日本語 🦀 and trailing\\";
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.windows_hide = true;
    spec.args.push("roundtrip".into());
    spec.args.extend(arguments.iter().map(Into::into));
    spec.env_clear = true;
    spec.env = vec![
        ("TURNLOOP_CHILD_VALUE".into(), "overwritten".into()),
        ("turnloop_child_value".into(), value.into()),
        ("TURNLOOP_CHILD_OTHER".into(), "".into()),
    ];
    spec.cwd = Some(cwd.clone());
    spec.stdio = [ProcessStdio::Null, ProcessStdio::Pipe, ProcessStdio::Null];
    let child = driver.spawn(&spec, Token(1)).expect("configured child");
    let stdout = child.stdout.expect("stdout");
    let read = driver
        .read_start(stdout, Token(2))
        .expect("read exact fixture output");
    let mut out = Completions::with_capacity(1);
    let mut bytes = Vec::new();
    let (mut reads, mut eof, mut exits) = (0, 0, 0);
    let deadline = driver.now() + Duration::from_secs(10);
    while eof == 0 || exits == 0 {
        assert!(driver.now() < deadline);
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("child output");
        for c in out.drain() {
            match c.result {
                OpResult::Read {
                    n,
                    lease: Some(data),
                } => {
                    assert_eq!(
                        (c.op, c.handle, c.token),
                        (Some(read), Some(stdout), Token(2))
                    );
                    assert!(n > 0 && !c.terminal);
                    bytes.extend_from_slice(data.as_slice());
                    reads += 1;
                }
                OpResult::Eof => {
                    assert_eq!(c.op, Some(read));
                    assert!(c.terminal);
                    eof += 1;
                }
                OpResult::Exited(status) => {
                    assert_eq!((c.handle, c.token), (Some(child.handle), Token(1)));
                    assert_eq!(status.code, Some(0));
                    assert!(c.terminal);
                    exits += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(reads > 0);
    assert_eq!((eof, exits), (1, 1));
    let mut expected = Vec::new();
    for field in
        arguments
            .into_iter()
            .chain([value, "", cwd.to_str().expect("UTF-8 cwd"), "<absent>"])
    {
        writeln!(&mut expected, "{}", field.len()).expect("expected length");
        expected.extend_from_slice(field.as_bytes());
    }
    assert_eq!(
        bytes, expected,
        "all 15 length-delimited argv/env/cwd fields"
    );
    driver.close(stdout, Token(3)).expect("close stdout");
    driver
        .close(child.handle, Token(4))
        .expect("close reaped child");
    let mut closed = 0;
    while driver.alive() {
        assert!(driver.now() < deadline);
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("close");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::Closed));
            closed += 1;
        }
    }
    assert_eq!(closed, 2);
    std::fs::remove_dir(directory).expect("remove private cwd");
}

#[test]
fn worker_file_fifo_and_loop_drop_quiesce_queued_buffers() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::io::{Read, Seek, SeekFrom};
    let path = std::env::temp_dir().join(format!("tl-windows-file-fifo-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("file");
    let mut provided = [[0xa5; 32]; 64]; // outlive driver on every unwind path
    let mut driver = Loop::new(Config::default()).expect("loop");
    let h = driver
        .attach(
            Detached::from_handle(file.try_clone().expect("duplicate").into())
                .expect("file transport"),
            Token(0),
        )
        .expect("attach worker file");
    let ops: [_; 64] = std::array::from_fn(|i| {
        driver
            .write(h, WriteBuf::Owned(vec![i as u8; 32]), Token(i as u64))
            .expect("FIFO write")
    });
    let mut out = Completions::with_capacity(1);
    let deadline = driver.now() + Duration::from_secs(10);
    let mut writes = 0;
    while writes < 64 {
        assert!(driver.now() < deadline);
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("file writes");
        for c in out.drain() {
            assert_eq!(
                (c.op, c.handle, c.token, c.terminal),
                (Some(ops[writes]), Some(h), Token(writes as u64), true)
            );
            assert!(matches!(c.result, OpResult::Wrote(32)));
            writes += 1;
        }
    }
    file.rewind().expect("rewind after writes");
    let mut bytes = [0; 2048];
    file.read_exact(&mut bytes).expect("FIFO file data");
    for (i, chunk) in bytes.as_chunks::<32>().0.iter().enumerate() {
        assert_eq!(*chunk, [i as u8; 32]);
    }
    file.rewind().expect("rewind before queued reads");
    for (i, memory) in provided.iter_mut().enumerate() {
        // SAFETY: each distinct fixed output stays exclusive until the corresponding
        // terminal completion or full driver destruction, including panic unwinding.
        let buffer = unsafe { IoBufMut::from_raw_parts(memory.as_mut_ptr(), memory.len()) };
        driver
            .read(h, ReadBuf::Provided(buffer), Token(100 + i as u64))
            .expect("queued read on drop");
    }
    driver
        .turn(Timeout::Now, &mut out)
        .expect("start first worker read");
    assert!(
        out.len() <= 1,
        "FIFO permits at most one started read per turn"
    );
    let mut reads = 0;
    for c in out.drain() {
        assert_eq!(c.token, Token(100));
        assert!(matches!(c.result, OpResult::Read { n: 32, lease: None }));
        reads += 1;
    }
    assert!(
        64 - reads > 0,
        "drop must have queued caller buffers to quiesce"
    );
    drop(driver);
    provided.fill([37; 32]);
    // Reuse the shared file offset with fresh ownership after drop. Old queued
    // reads must not consume these bytes or write to the released caller buffers.
    file.seek(SeekFrom::Start(0)).expect("rewind after drop");
    let mut fresh = Loop::new(Config::default()).expect("fresh loop");
    let h = fresh
        .attach(
            Detached::from_handle(file.try_clone().expect("fresh duplicate").into()).expect("file"),
            Token(1),
        )
        .expect("fresh worker");
    fresh
        .read(h, ReadBuf::Pooled, Token(2))
        .expect("fresh read");
    let deadline = fresh.now() + Duration::from_secs(5);
    loop {
        assert!(fresh.now() < deadline);
        fresh
            .turn(Timeout::Until(deadline), &mut out)
            .expect("fresh worker completion");
        if out.is_empty() {
            continue;
        }
        let OpResult::Read {
            n: 2048,
            lease: Some(data),
        } = &out[0].result
        else {
            panic!("fresh read missing")
        };
        assert_eq!(data.as_slice(), bytes);
        break;
    }
    drop(fresh);
    assert_eq!(provided, [[37; 32]; 64]);
    assert_eq!(writes, 64);
    drop(file);
    std::fs::remove_file(path).expect("remove FIFO file");
}

#[test]
fn local_connect_deadlines_complete_and_cancel() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = PipeName(format!(r"\\.\pipe\tl-connect-deadline-{}", std::process::id()).into());
    turnloop_contract::native_surface::pipe_connect_deadlines::<backend::Platform>(&name);
}

#[test]
fn listener_reuse_and_busy_connect_drop_release_native_handles() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = PipeName(format!(r"\\.\pipe\tl-pipe-drop-{}", std::process::id()).into());
    let cycle = || {
        let mut server = Loop::new(Config::default()).expect("server");
        let mut out = Completions::with_capacity(1);
        for _ in 0..4 {
            let listener = server
                .pipe_listen(
                    &name,
                    &ListenOpts {
                        backlog: 4,
                        ..ListenOpts::default()
                    },
                )
                .expect("fresh listener generation");
            let accept = server.accept(listener, Token(1)).expect("pending accept");
            server.turn(Timeout::Now, &mut out).expect("arm accept");
            assert!(out.is_empty());
            let _occupied: [std::fs::File; 4] = std::array::from_fn(|_| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&name.0)
                    .expect("occupy backlog")
            });
            let mut client = Loop::new(Config::default()).expect("busy client");
            client
                .pipe_connect(&name, Token(2))
                .expect("pending busy connect");
            let at = client.now() + Duration::from_millis(2);
            let info = client
                .turn(Timeout::Until(at), &mut out)
                .expect("park availability before drop");
            assert_eq!((info.os_waits, info.zero_event_waits), (1, 1));
            assert!(client.now() >= at && out.is_empty());
            drop(client); // must cancel/drain the pending FSCTL before freeing its input
            server
                .close(listener, Token(3))
                .expect("close with queued native packets");
            let until = server.now() + Duration::from_secs(5);
            let mut count = 0;
            while count < 2 {
                assert!(server.now() < until);
                server
                    .turn(Timeout::Until(until), &mut out)
                    .expect("retire listener generation");
                for c in out.drain() {
                    assert_eq!(c.handle, Some(listener));
                    if count == 0 {
                        assert_eq!(c.op, Some(accept));
                        assert!(matches!(c.result, OpResult::Cancelled));
                    } else {
                        assert!(matches!(c.result, OpResult::Closed));
                    }
                    count += 1;
                }
            }
            // The next listener is opened before the old private IOCP packets
            // are necessarily dequeued; stale addresses must not target it.
            assert!(!server.alive());
        }
        4
    };
    assert_eq!(cycle(), 4);
    let baseline = handles();
    let mut drops = 0;
    for _ in 0..8 {
        drops += cycle();
        assert_eq!(handles(), baseline, "listener or availability handle leak");
    }
    assert_eq!(drops, 32);
}

// Pins identity and guarantees cleanup even if an assertion fails.
struct LiveProcess(OwnedHandle);
impl LiveProcess {
    fn open(pid: u32) -> Self {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE};
        // SAFETY: fixture parent keeps this child alive until our acknowledgement.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
        assert!(
            !raw.is_null(),
            "pin process {pid}: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: successful OpenProcess transfers unique ownership.
        Self(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
    fn wait(&self, timeout: u32) -> u32 {
        // SAFETY: owned process handle pins identity through bounded wait.
        unsafe { WaitForSingleObject(self.0.as_raw_handle(), timeout) }
    }
}
impl Drop for LiveProcess {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::TerminateProcess;
        if self.wait(0) == WAIT_TIMEOUT {
            // SAFETY: only the test-created process identified by this owned handle.
            unsafe {
                TerminateProcess(self.0.as_raw_handle(), 1);
            }
            assert_eq!(self.wait(10_000), WAIT_OBJECT_0, "fixture cleanup");
        }
    }
}

struct FixtureParent(std::process::Child);
impl Drop for FixtureParent {
    fn drop(&mut self) {
        if self.0.try_wait().expect("fixture parent status").is_none() {
            self.0.kill().expect("fixture parent cleanup");
            self.0.wait().expect("fixture parent reap");
        }
    }
}

#[test]
fn parent_death_kills_only_non_detached_children() {
    use std::io::BufRead;
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut checked = 0;
    for detached in [false, true] {
        for exit in [false, true] {
            let mut parent = FixtureParent(
                std::process::Command::new(env!("CARGO_BIN_EXE_native_child"))
                    .args([
                        "lifetime-parent",
                        if detached { "detached" } else { "attached" },
                        if exit { "exit" } else { "kill" },
                    ])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::inherit())
                    .spawn()
                    .expect("fixture parent"),
            );
            let stdout = parent.0.stdout.take().expect("parent stdout");
            let (send, receive) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let mut line = String::new();
                std::io::BufReader::new(stdout)
                    .read_line(&mut line)
                    .expect("ready line");
                send.send(line).expect("ready notification");
            });
            let line = receive
                .recv_timeout(Duration::from_secs(10))
                .expect("spawn watchdog");
            reader.join().expect("identity reader");
            let pid: u32 = line
                .trim()
                .strip_prefix("child:")
                .expect("executed spawn marker")
                .parse()
                .expect("child PID");
            let child = LiveProcess::open(pid);
            assert_eq!(
                child.wait(0),
                WAIT_TIMEOUT,
                "child must be live before parent exit"
            );
            if exit {
                parent
                    .0
                    .stdin
                    .take()
                    .expect("parent stdin")
                    .write_all(b"x")
                    .expect("allow exit");
            } else {
                parent.0.kill().expect("abrupt parent death");
            }
            assert_eq!(
                // SAFETY: std Child owns this handle until after the wait.
                unsafe { WaitForSingleObject(parent.0.as_raw_handle(), 10_000) },
                WAIT_OBJECT_0
            );
            let status = parent.0.wait().expect("parent exit status");
            if exit {
                assert_eq!(status.code(), Some(23));
            }
            if detached {
                assert_eq!(
                    child.wait(200),
                    WAIT_TIMEOUT,
                    "detached child survives parent death"
                );
            } else {
                assert_eq!(
                    child.wait(10_000),
                    WAIT_OBJECT_0,
                    "lifetime job must terminate child"
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 4);
}

#[test]
fn normal_leader_exit_keeps_grandchildren_alive_through_close_and_drop() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut checked = 0;
    for group in [false, true] {
        for consume_exit in [false, true] {
            for close in [false, true] {
                let (mut input, mut writer) = (ptr::null_mut(), ptr::null_mut());
                assert_ne!(
                    // SAFETY: initialized outputs for a non-inheritable synchronous pipe.
                    unsafe { CreatePipe(&mut input, &mut writer, ptr::null(), 0) },
                    0
                );
                // SAFETY: successful CreatePipe transfers two independent owners.
                let (input, writer) = unsafe {
                    (
                        OwnedHandle::from_raw_handle(input),
                        OwnedHandle::from_raw_handle(writer),
                    )
                };
                let mut writer = std::fs::File::from(writer);
                let mut driver = Loop::new(Config::default()).expect("loop");
                let control = driver
                    .attach(Detached::from_handle(input).expect("input pipe"), Token(0))
                    .expect("control pipe");
                let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
                spec.args.push("orphan-leader".into());
                spec.windows_hide = true;
                spec.new_process_group = group;
                spec.stdio = [
                    ProcessStdio::Handle(control),
                    ProcessStdio::Pipe,
                    ProcessStdio::Null,
                ];
                let child = driver.spawn(&spec, Token(1)).expect("orphan leader");
                let leader = LiveProcess::open(child.pid);
                driver
                    .read_start(child.stdout.expect("stdout"), Token(2))
                    .expect("leader marker");
                let mut out = Completions::with_capacity(1);
                let mut marker = Vec::new();
                let deadline = driver.now() + Duration::from_secs(10);
                while !marker.contains(&b'\n') {
                    assert!(driver.now() < deadline);
                    driver
                        .turn(Timeout::Until(deadline), &mut out)
                        .expect("leader ready turn");
                    for c in out.drain() {
                        let OpResult::Read {
                            n,
                            lease: Some(bytes),
                        } = c.result
                        else {
                            panic!("{c:?}")
                        };
                        assert!(n > 0);
                        marker.extend_from_slice(bytes.as_slice());
                    }
                }
                let pid: u32 = std::str::from_utf8(&marker)
                    .expect("marker UTF8")
                    .trim()
                    .strip_prefix("grandchild:")
                    .expect("spawn ran")
                    .parse()
                    .expect("PID");
                let grandchild = LiveProcess::open(pid);
                assert_eq!(grandchild.wait(0), WAIT_TIMEOUT);
                assert_eq!(leader.wait(0), WAIT_TIMEOUT);
                writer.write_all(b"x").expect("allow normal leader exit");
                assert_eq!(leader.wait(10_000), WAIT_OBJECT_0);
                if consume_exit {
                    let mut exits = 0;
                    while exits == 0 {
                        assert!(driver.now() < deadline);
                        driver
                            .turn(Timeout::Until(deadline), &mut out)
                            .expect("exit turn");
                        for c in out.drain() {
                            match c.result {
                                OpResult::Exited(status) => {
                                    assert_eq!(status.code, Some(23));
                                    exits += 1;
                                }
                                OpResult::Eof => {}
                                other => panic!("unexpected {other:?}"),
                            }
                        }
                    }
                    assert_eq!(exits, 1);
                }
                if close {
                    driver
                        .close(child.handle, Token(3))
                        .expect("close exited leader");
                    let mut closed = 0;
                    while closed == 0 {
                        assert!(driver.now() < deadline);
                        driver
                            .turn(Timeout::Until(deadline), &mut out)
                            .expect("close turn");
                        for c in out.drain() {
                            match c.result {
                                OpResult::Closed => {
                                    assert_eq!(c.handle, Some(child.handle));
                                    closed += 1;
                                }
                                OpResult::Eof | OpResult::Cancelled => {}
                                other => panic!("unexpected {other:?}"),
                            }
                        }
                    }
                    assert_eq!(closed, 1);
                }
                drop(driver);
                assert_eq!(
                    grandchild.wait(200),
                    WAIT_TIMEOUT,
                    "normal leader release killed grandchild"
                );
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 8);
}

#[test]
fn idle_synchronous_pipe_reader_does_not_spin() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut driver, client, _server) =
        turnloop_contract::native_surface::synchronous_duplex_pair();
    let read = driver
        .read(client, ReadBuf::Pooled, Token(90))
        .expect("idle read");
    let mut out = Completions::with_capacity(1);
    let (mut expiries, mut waits) = (0, 0);
    for micros in [500, 2_000, 10_000] {
        for _ in 0..20 {
            let at = driver.now() + Duration::from_micros(micros);
            let timer = driver.timer(at, None, Token(91)).expect("timer");
            let (mut turns, mut empty_waits) = (0, 0);
            loop {
                turns += 1;
                assert!(turns <= 2, "idle synchronous read caused timer spin");
                let info = driver
                    .turn(Timeout::Until(at), &mut out)
                    .expect("timer turn");
                assert!(info.os_waits <= 1);
                waits += info.os_waits;
                empty_waits += info.zero_event_waits;
                assert!(empty_waits <= 1);
                if !out.is_empty() {
                    assert_eq!(out.len(), 1);
                    assert_eq!((out[0].handle, out[0].token), (Some(timer), Token(91)));
                    assert!(matches!(out[0].result, OpResult::Timer));
                    assert!(driver.now() >= at);
                    expiries += 1;
                    break;
                }
            }
            driver.close(timer, Token(92)).expect("release timer");
            driver.turn(Timeout::Now, &mut out).expect("timer closed");
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Closed));
        }
    }
    assert_eq!(expiries, 60);
    assert!(waits >= 60);
    assert!(
        driver.cancel(read),
        "read stayed pending through all deadlines"
    );
    let deadline = driver.now() + Duration::from_secs(3);
    loop {
        assert!(driver.now() < deadline);
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("read cancellation");
        if !out.is_empty() {
            break;
        }
    }
    assert_eq!(out.len(), 1);
    assert_eq!(
        (out[0].op, out[0].token, out[0].terminal),
        (Some(read), Token(90), true)
    );
    assert!(matches!(out[0].result, OpResult::Cancelled));
}

/// Raw kernel control for the synchronous duplex worker design, independent of turnloop.
/// Windows serializes all I/O on a synchronous file object: WriteFile on a duplicate of
/// the same pipe endpoint waits (with no IRP of its own, so CancelSynchronousIo cannot
/// reach it) until the idle ReadFile leaves the kernel. Cancelling the idle read releases
/// the write and consumes no bytes. GetConsoleMode on a pipe fails without joining that
/// queue; sem-fix1's hypothesis that it was the blocking call did not hold on windows-2025.
#[test]
fn synchronous_pipe_write_waits_behind_idle_read_until_the_read_is_cancelled() {
    use std::{io::Read, sync::mpsc, time::Instant};
    use windows_sys::Win32::{
        Foundation::{
            ERROR_NOT_FOUND, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED, GENERIC_READ,
            GENERIC_WRITE, GetLastError, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            CreateFileW, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        },
        System::{
            Console::GetConsoleMode,
            IO::CancelSynchronousIo,
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
            },
            Threading::GetThreadIOPendingFlag,
        },
    };
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let name: Vec<u16> = format!(r"\\.\pipe\turnloop-sync-serialize-{}", std::process::id())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: terminated private name; one synchronous duplex instance, no inheritance.
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    // SAFETY: successful create transferred unique ownership.
    let mut server = std::fs::File::from(unsafe { OwnedHandle::from_raw_handle(raw) });
    // SAFETY: terminated private name; synchronous client with both data directions.
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    // SAFETY: successful create transferred unique ownership.
    let client = std::fs::File::from(unsafe { OwnedHandle::from_raw_handle(raw) });
    // SAFETY: both ends already open, so synchronous connect cannot wait for a client.
    if unsafe { ConnectNamedPipe(server.as_raw_handle(), ptr::null_mut()) } == 0 {
        // SAFETY: immediately inspect this thread's last error.
        assert_eq!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED);
    }
    fn io_pending(thread: &std::thread::JoinHandle<()>) -> bool {
        let mut pending = 0;
        // SAFETY: the join handle pins the thread; the output is valid.
        assert_ne!(
            // SAFETY: the join handle pins the thread; the output is valid.
            unsafe { GetThreadIOPendingFlag(thread.as_raw_handle(), &mut pending) },
            0
        );
        pending != 0
    }
    type Raw = (i32, u32, u32, [u8; 4]);
    let spawn_read = |file: std::fs::File| {
        let (done, result) = mpsc::channel::<Raw>();
        let thread = std::thread::spawn(move || {
            let mut bytes = [0u8; 4];
            let mut count = 0;
            // SAFETY: owned synchronous handle and exclusive buffer until the call returns.
            let ok = unsafe {
                ReadFile(
                    file.as_raw_handle(),
                    bytes.as_mut_ptr(),
                    4,
                    &mut count,
                    ptr::null_mut(),
                )
            };
            // SAFETY: this thread's last error, read immediately after the call.
            let last = unsafe { GetLastError() };
            let error = if ok == 0 { last } else { 0 };
            done.send((ok, error, count, bytes)).expect("read result");
        });
        (thread, result)
    };
    let (read_thread, read_done) = spawn_read(client.try_clone().expect("reader duplicate"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !io_pending(&read_thread) {
        assert!(
            Instant::now() < deadline,
            "raw ReadFile never entered kernel I/O"
        );
        std::thread::sleep(Duration::from_millis(1)); // test synchronization only
    }
    // Control I/O on a pipe fails promptly rather than queueing behind the read.
    let query = client.try_clone().expect("query duplicate");
    let (queried, query_done) = mpsc::channel();
    let query_thread = std::thread::spawn(move || {
        let mut mode = 0;
        // SAFETY: owned duplicate and valid output, probing beside the idle read.
        queried
            .send(unsafe { GetConsoleMode(query.as_raw_handle(), &mut mode) })
            .expect("query completed");
    });
    let query_result = query_done.recv_timeout(Duration::from_secs(2));
    let writer = client.try_clone().expect("writer duplicate");
    let (written, write_done) = mpsc::channel::<Raw>();
    let write_thread = std::thread::spawn(move || {
        let mut count = 0;
        // SAFETY: owned synchronous handle and immutable byte until the call returns.
        let ok = unsafe {
            WriteFile(
                writer.as_raw_handle(),
                [0x33].as_ptr(),
                1,
                &mut count,
                ptr::null_mut(),
            )
        };
        // SAFETY: this thread's last error, read immediately after the call.
        let last = unsafe { GetLastError() };
        let error = if ok == 0 { last } else { 0 };
        written
            .send((ok, error, count, [0; 4]))
            .expect("write result");
    });
    let before_cancel = write_done.recv_timeout(Duration::from_millis(500));
    let writer_pending = io_pending(&write_thread);
    // SAFETY: the join handle pins the writer thread for this call.
    let cancel_writer = unsafe { CancelSynchronousIo(write_thread.as_raw_handle()) };
    // SAFETY: this thread's last error, read immediately after the call.
    let cancel_writer_error = unsafe { GetLastError() };
    // Release the serialization exactly as the read worker's preemption does. The
    // cancellation is retried only while the read has not yet been cancelled.
    // SAFETY: the join handle pins the reader thread for this call.
    assert_ne!(
        // SAFETY: the join handle pins the reader thread for this call.
        unsafe { CancelSynchronousIo(read_thread.as_raw_handle()) },
        0,
        "idle ReadFile is cancellable"
    );
    let read = read_done
        .recv_timeout(Duration::from_secs(5))
        .expect("cancelled read returned");
    let write = write_done.recv_timeout(Duration::from_secs(5));
    read_thread.join().expect("read joined");
    query_thread.join().expect("query joined");
    // Evidence assertions follow cleanup of every thread that could still be blocked.
    let write = write.expect("write released by the read cancellation");
    write_thread.join().expect("write joined");
    assert_eq!(
        query_result.expect("GetConsoleMode did not wait behind the read"),
        0,
        "pipe is not a console"
    );
    assert!(
        matches!(before_cancel, Err(mpsc::RecvTimeoutError::Timeout)),
        "WriteFile completed beside an idle ReadFile on one synchronous object"
    );
    assert!(
        !writer_pending,
        "the waiting writer holds no kernel request"
    );
    assert_eq!(
        (cancel_writer, cancel_writer_error),
        (0, ERROR_NOT_FOUND),
        "CancelSynchronousIo cannot release a writer waiting behind the read"
    );
    assert_eq!(
        (read.0, read.1, read.2),
        (0, ERROR_OPERATION_ABORTED, 0),
        "idle read aborted without data"
    );
    assert_eq!(
        (write.0, write.2),
        (1, 1),
        "write completed after the read left"
    );
    let mut byte = [0];
    server
        .read_exact(&mut byte)
        .expect("raw write reached peer");
    assert_eq!(byte, [0x33]);
    // The aborted read consumed nothing: a reissued read receives every later byte.
    server.write_all(&[0x77, 0x78]).expect("peer bytes");
    let (reissued, reissued_done) = spawn_read(client.try_clone().expect("reissue duplicate"));
    let reissued_result = reissued_done
        .recv_timeout(Duration::from_secs(5))
        .expect("reissued read");
    reissued.join().expect("reissued joined");
    assert_eq!(
        (
            reissued_result.0,
            reissued_result.2,
            &reissued_result.3[..2]
        ),
        (1, 2, &[0x77, 0x78][..])
    );
}

#[test]
fn duplex_writes_preempt_reads_at_every_entry_point_without_losing_bytes() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut driver, client, server) = turnloop_contract::native_surface::synchronous_duplex_pair();
    let mut out = Completions::with_capacity(4);
    let mut checked = [0; 3];
    for i in 0..384usize {
        let byte = i as u8;
        let reply = !byte;
        let read = driver
            .read(client, ReadBuf::Pooled, Token(1))
            .expect("client read");
        let mut events = [0; 4]; // client wrote, server read, server wrote, client read
        let collect =
            |driver: &mut Loop, out: &mut Completions, events: &mut [usize; 4], write: OpId| {
                for c in out.drain() {
                    assert!(c.terminal);
                    match (c.handle, c.result) {
                        (Some(h), OpResult::Wrote(1)) if h == client => {
                            assert_eq!(c.op, Some(write));
                            events[0] += 1;
                        }
                        (
                            Some(h),
                            OpResult::Read {
                                n: 1,
                                lease: Some(bytes),
                            },
                        ) if h == server => {
                            assert_eq!(bytes.as_slice(), [byte]);
                            events[1] += 1;
                            driver
                                .write(server, WriteBuf::Owned(vec![reply]), Token(4))
                                .expect("peer reply");
                        }
                        (Some(h), OpResult::Wrote(1)) if h == server => events[2] += 1,
                        (
                            Some(h),
                            OpResult::Read {
                                n: 1,
                                lease: Some(bytes),
                            },
                        ) if h == client => {
                            assert_eq!(c.op, Some(read));
                            assert_eq!(bytes.as_slice(), [reply], "read lost or reordered bytes");
                            events[3] += 1;
                        }
                        (handle, result) => panic!("unexpected {handle:?} {result:?}"),
                    }
                }
            };
        // Pattern 0 queues both requests for one turn, so the write worker preempts a
        // read that may not have reached the kernel yet. Pattern 1 starts the read one
        // turn earlier, racing its kernel entry. Pattern 2 lets the read settle idle.
        let pattern = i % 3;
        if pattern != 0 {
            driver.turn(Timeout::Now, &mut out).expect("start read");
            assert!(out.is_empty(), "no peer bytes yet");
            if pattern == 2 {
                std::thread::sleep(Duration::from_millis(2)); // test synchronization only
            }
        }
        let write = driver
            .write(client, WriteBuf::Owned(vec![byte]), Token(2))
            .expect("client write");
        driver
            .read(server, ReadBuf::Pooled, Token(3))
            .expect("server read");
        let deadline = driver.now() + Duration::from_secs(5);
        while events != [1; 4] {
            assert!(
                driver.now() < deadline,
                "preemption pattern {pattern} stalled: {events:?}"
            );
            driver
                .turn(Timeout::Until(deadline), &mut out)
                .expect("preemption turn");
            collect(&mut driver, &mut out, &mut events, write);
            assert!(events.iter().all(|count| *count <= 1));
        }
        checked[pattern] += 1;
    }
    assert_eq!(checked, [128; 3]);
}

#[test]
fn duplex_cancellation_is_per_direction_and_close_drop_join_both_workers() {
    let _guard = HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut checked = 0;
    for cancel_write in [false, true] {
        for close in [false, true] {
            for _ in 0..8 {
                let mut input = [0xa5; 2];
                let mut payload = vec![0x33; 1024 * 1024]; // exceeds the unconsumed pipe quota
                let (mut driver, client, server) =
                    turnloop_contract::native_surface::synchronous_duplex_pair();
                // SAFETY: first byte remains exclusive until completion or driver drop.
                let buf = unsafe { IoBufMut::from_raw_parts(input.as_mut_ptr(), 1) };
                let read = driver
                    .read(client, ReadBuf::Provided(buf), Token(1))
                    .expect("idle read");
                // SAFETY: immutable allocation stays fixed through cancellation/drop.
                let buf = unsafe { IoBuf::from_raw_parts(payload.as_ptr(), payload.len()) };
                let write = driver
                    .write(client, WriteBuf::Provided(buf), Token(2))
                    .expect("blocked write");
                let mut out = Completions::with_capacity(1);
                driver
                    .turn(Timeout::Now, &mut out)
                    .expect("start both workers");
                assert!(out.is_empty(), "neither direction has peer progress");
                let deadline = driver.now() + Duration::from_secs(5);
                let mut drained = 0;
                if !cancel_write {
                    // Starting a request does not schedule its worker thread. Before the
                    // read's cancellation, pin the state under test: peer bytes prove the
                    // write is inside WriteFile, still blocked on the rest of 1 MiB.
                    driver
                        .read(server, ReadBuf::Pooled, Token(5))
                        .expect("first surviving write bytes");
                    while drained == 0 {
                        assert!(driver.now() < deadline, "write never reached the peer");
                        driver
                            .turn(Timeout::Until(deadline), &mut out)
                            .expect("write enters the kernel");
                        for c in out.drain() {
                            let OpResult::Read {
                                n,
                                lease: Some(bytes),
                            } = c.result
                            else {
                                panic!("before cancellation: {:?}", c.result);
                            };
                            assert_eq!(c.handle, Some(server));
                            assert!(bytes.as_slice().iter().all(|byte| *byte == 0x33));
                            drained += n;
                        }
                    }
                    assert!(drained < payload.len());
                }
                let cancelled = if cancel_write { write } else { read };
                assert!(driver.cancel(cancelled), "cancel one direction");
                let mut acks = 0;
                while acks == 0 {
                    assert!(
                        driver.now() < deadline,
                        "per-direction cancellation watchdog"
                    );
                    driver
                        .turn(Timeout::Until(deadline), &mut out)
                        .expect("cancel acknowledgement");
                    for c in out.drain() {
                        assert_eq!(c.op, Some(cancelled));
                        assert!(c.terminal && matches!(c.result, OpResult::Cancelled));
                        acks += 1;
                    }
                }
                assert_eq!(acks, 1);
                let expected_read = if cancel_write {
                    read
                } else {
                    // SAFETY: cancelled read acknowledged; a separate exclusive byte.
                    let buf = unsafe { IoBufMut::from_raw_parts(input.as_mut_ptr().add(1), 1) };
                    driver
                        .read(client, ReadBuf::Provided(buf), Token(3))
                        .expect("new read beside blocked write")
                };
                driver
                    .write(server, WriteBuf::Owned(vec![0x77]), Token(4))
                    .expect("peer response");
                // cancel_write: the preempted idle read resumes once the write's
                // cancellation is acknowledged. Otherwise the blocked write survived the
                // read's cancellation and must deliver every byte. Windows serializes all
                // I/O on one synchronous file object, and a write waiting for the peer to
                // drain holds it (synchronous_pipe_write_waits_behind_idle_read_until_the_
                // read_is_cancelled; sync_io.rs), so the new read completes only after it.
                let (mut reads, mut responses) = (0, 0);
                let mut wrote = cancel_write;
                if cancel_write {
                    drained = payload.len();
                } else {
                    driver
                        .read(server, ReadBuf::Pooled, Token(5))
                        .expect("peer drains the surviving write");
                }
                let mut trace = Vec::new(); // failure context: completion order
                while reads == 0 || responses == 0 || !wrote || drained < payload.len() {
                    assert!(
                        driver.now() < deadline,
                        "cancel_write={cancel_write}: {trace:?}"
                    );
                    driver
                        .turn(Timeout::Until(deadline), &mut out)
                        .expect("other direction survives cancellation");
                    for c in out.drain() {
                        trace.push(match &c.result {
                            OpResult::Read { n, .. } => format!("{:?}:Read({n})", c.token),
                            other => format!("{:?}:{other:?}", c.token),
                        });
                        match c.result {
                            OpResult::Read { n: 1, lease: None } => {
                                assert_eq!(c.op, Some(expected_read));
                                assert!(
                                    wrote,
                                    "read overtook the write it queued behind: close={close} {trace:?}"
                                );
                                reads += 1;
                            }
                            OpResult::Read {
                                n,
                                lease: Some(bytes),
                            } => {
                                assert!(!cancel_write);
                                assert_eq!(c.handle, Some(server));
                                assert!(n > 0 && drained + n <= payload.len());
                                assert!(bytes.as_slice().iter().all(|byte| *byte == 0x33));
                                drained += n;
                                if drained < payload.len() {
                                    driver
                                        .read(server, ReadBuf::Pooled, Token(5))
                                        .expect("more surviving write bytes");
                                }
                            }
                            OpResult::Wrote(1) => {
                                assert_eq!(c.handle, Some(server));
                                responses += 1;
                            }
                            OpResult::Wrote(n) if n == payload.len() => {
                                assert!(!cancel_write && !wrote);
                                assert_eq!((c.handle, c.op), (Some(client), Some(write)));
                                wrote = true;
                            }
                            other => panic!("cross-direction cancellation: {other:?}"),
                        }
                    }
                }
                assert_eq!((reads, responses, drained), (1, 1, payload.len()));
                let expected = if cancel_write {
                    [0x77, 0xa5]
                } else {
                    [0xa5, 0x77]
                };
                assert_eq!(input, expected);
                // Both directions also retain queued operations during close/drop.
                for i in 0..2 {
                    // SAFETY: disjoint input bytes remain fixed until driver destruction.
                    let buf = unsafe { IoBufMut::from_raw_parts(input.as_mut_ptr().add(i), 1) };
                    driver
                        .read(client, ReadBuf::Provided(buf), Token(10 + i as u64))
                        .expect("pending/queued reads");
                    // SAFETY: immutable shared payload remains live through all acknowledgements.
                    let buf = unsafe { IoBuf::from_raw_parts(payload.as_ptr(), payload.len()) };
                    driver
                        .write(client, WriteBuf::Provided(buf), Token(20 + i as u64))
                        .expect("pending/queued writes");
                }
                driver
                    .turn(Timeout::Now, &mut out)
                    .expect("start teardown subjects");
                assert!(out.is_empty());
                if close {
                    driver
                        .close(client, Token(30))
                        .expect("close both directions");
                    let mut cancellations = 0;
                    let mut closed = 0;
                    while closed == 0 {
                        assert!(driver.now() < deadline);
                        driver
                            .turn(Timeout::Until(deadline), &mut out)
                            .expect("duplex close acknowledgements");
                        for c in out.drain() {
                            assert_eq!(c.handle, Some(client));
                            assert!(c.terminal);
                            match c.result {
                                OpResult::Cancelled => cancellations += 1,
                                OpResult::Closed => {
                                    // Pending/queued reads and writes; phase one's write
                                    // was cancelled or completed above.
                                    assert_eq!(cancellations, 4);
                                    closed += 1;
                                }
                                other => panic!("unexpected close {other:?}"),
                            }
                        }
                    }
                    assert_eq!(closed, 1);
                }
                drop(driver);
                assert_eq!(input, expected);
                input.fill(0x5a);
                payload.fill(0x99);
                assert_eq!(input, [0x5a; 2]);
                assert!(payload.iter().all(|byte| *byte == 0x99));
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 32);
}
