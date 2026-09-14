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
    let _guard = HANDLES.lock().expect("handle test lock");
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut completed = 0;
    for group in [false, true] {
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
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
    let _guard = HANDLES.lock().expect("handle test lock");
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
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
    let _guard = HANDLES.lock().expect("handle test lock");
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
    let _guard = HANDLES.lock().expect("handle test lock");
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
    let _guard = HANDLES.lock().expect("handle test lock");
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
    let _guard = HANDLES.lock().expect("handle test lock");
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
    let _guard = HANDLES.lock().expect("handle test lock");
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
    let _guard = HANDLES.lock().expect("handle test lock");
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
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
