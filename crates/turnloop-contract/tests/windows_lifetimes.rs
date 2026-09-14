#![cfg(all(windows, not(loom)))]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    os::windows::io::{FromRawHandle, OwnedHandle},
    ptr,
    time::Duration,
};
use turnloop::*;
use windows_sys::Win32::System::{
    Pipes::CreatePipe,
    Threading::{GetCurrentProcess, GetProcessHandleCount},
};

#[test]
fn imported_overlapped_pipe_routes_away_from_its_existing_port() {
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
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: two output slots; noninherited anonymous synchronous pipe.
    assert_ne!(
        // SAFETY: two output slots; noninherited anonymous synchronous pipe.
        unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 4096) },
        0
    );
    // SAFETY: successful CreatePipe transferred two independent handle owners.
    let (read, _write) = unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    };
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
        assert_eq!(memory, [0xa5; 64]);
    }
}
#[test]
fn loop_drop_and_stale_wakers_release_windows_handles() {
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
