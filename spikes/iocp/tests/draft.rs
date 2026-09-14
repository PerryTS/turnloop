#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    io,
    net::{TcpListener, TcpStream},
    os::windows::io::OwnedSocket,
    sync::Arc,
    time::{Duration, Instant},
};
use windlass_iocp_spike::backend_draft::*;

thread_local! { static TRACK: Cell<bool> = const { Cell::new(false) }; static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
struct Counting;
// SAFETY: all allocation/deallocation is delegated unchanged to System; TLS only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
        // SAFETY: unchanged allocation contract forwarded to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout from matching System allocation.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
        // SAFETY: unchanged reallocation contract forwarded to System.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
struct CountScope;
impl CountScope {
    fn start() -> Self {
        ALLOCS.set(0);
        TRACK.set(true);
        Self
    }
}
impl Drop for CountScope {
    fn drop(&mut self) {
        TRACK.set(false);
    }
}

#[test]
fn zero_allocation_transfers_and_cancel_before_closed() -> io::Result<()> {
    // Buffers declared before backend, so teardown drains before they go out of scope.
    let payload = *b"windlass";
    let mut buffer = [0u8; 8];
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let client = TcpStream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    let mut backend = IocpBackend::new(2, 8)?;
    // SAFETY: std TCP streams are overlapped-capable and have no existing IOCP association.
    let (client, server) = unsafe {
        (
            backend.register(Resource::Socket(OwnedSocket::from(client)))?,
            backend.register(Resource::Socket(OwnedSocket::from(server)))?,
        )
    };
    let mut completions = [Completion::default(); 2];
    let mut total = 0;
    for measured in [false, true] {
        let count = measured.then(CountScope::start);
        let repetitions = if measured { 256 } else { 1 };
        for _ in 0..repetitions {
            // SAFETY: read buffer exclusive and payload immutable until both completions;
            // backend is dropped before either buffer on all error/unwind paths.
            unsafe {
                backend.submit(
                    server,
                    Token(1),
                    Request::Read {
                        buffer: buffer.as_mut_ptr(),
                        len: 8,
                    },
                )?;
                backend.submit(
                    client,
                    Token(2),
                    Request::Write {
                        buffer: payload.as_ptr(),
                        len: 8,
                    },
                )?;
            }
            let mut read = false;
            let mut wrote = false;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !read || !wrote {
                assert!(Instant::now() < deadline);
                let info = backend.turn(Some(Duration::from_millis(50)), &mut completions)?;
                assert!(info.waits <= 1);
                for done in &completions[..info.completions] {
                    match done.token.0 {
                        1 => {
                            assert!(!read);
                            assert_eq!(done.result, ResultKind::Read(8));
                            read = true;
                        }
                        2 => {
                            assert!(!wrote);
                            assert_eq!(done.result, ResultKind::Wrote(8));
                            wrote = true;
                        }
                        _ => panic!("unknown completion"),
                    }
                }
            }
            assert_eq!(buffer, payload);
            total += buffer.len();
        }
        drop(count);
        if measured {
            assert_eq!(ALLOCS.get(), 0, "steady-state allocations");
        }
    }
    assert_eq!(total, 257 * 8);
    // SAFETY: buffer stays exclusive until cancellation completion; backend drops first.
    let op = unsafe {
        backend.submit(
            server,
            Token(3),
            Request::Read {
                buffer: buffer.as_mut_ptr(),
                len: 8,
            },
        )
    }?;
    backend.close(server, Token(4))?;
    let mut sequence = [ResultKind::Closed; 2];
    let mut n = 0;
    let mut one = [Completion::default(); 1];
    let deadline = Instant::now() + Duration::from_secs(3);
    while n < 2 {
        assert!(Instant::now() < deadline);
        let info = backend.turn(Some(Duration::from_millis(50)), &mut one)?;
        if info.completions != 0 {
            sequence[n] = one[0].result;
            n += 1;
        }
    }
    assert_eq!(sequence, [ResultKind::Cancelled, ResultKind::Closed]);
    assert!(
        !backend.cancel(op)?,
        "stale operation must not cancel a reused slot"
    );
    backend.close(client, Token(5))?;
    let info = backend.turn(None, &mut completions)?;
    assert_eq!(info.waits, 0, "queued close must not wait");
    assert_eq!(info.completions, 1);
    assert!(!info.alive);
    Ok(())
}

#[test]
fn notifier_running_fast_path_and_parked_wake() -> io::Result<()> {
    let mut backend = IocpBackend::new(1, 1)?;
    let notifier = Arc::new(backend.notifier());
    for _ in 0..10000 {
        notifier.notify()?;
    }
    assert_eq!(notifier.syscall_count(), 0);
    let mut out = [Completion::default(); 1];
    let info = backend.turn(None, &mut out)?;
    assert_eq!(info.waits, 0);
    assert!(info.notified);
    let sender = Arc::clone(&notifier);
    let thread = std::thread::spawn(move || -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !sender.is_parked() {
            if Instant::now() > deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "loop did not park"));
            }
            std::thread::yield_now();
        }
        sender.notify()
    });
    let info = backend.turn(Some(Duration::from_secs(3)), &mut out)?;
    thread
        .join()
        .map_err(|_| io::Error::other("notifier thread panicked"))??;
    assert!(info.notified);
    assert_eq!(info.waits, 1);
    assert_eq!(notifier.syscall_count(), 1);
    Ok(())
}

#[test]
fn event_notifier_wakes_before_the_first_turn() -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0, UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx,
    };
    let mut backend = IocpBackend::new(1, 1)?;
    let notifier = backend.notifier();
    let event = backend.integration()?;
    notifier.notify()?;
    // SAFETY: backend owns the helper/event for the entire wait.
    let ready = unsafe { MsgWaitForMultipleObjectsEx(1, &event, 2000, 0, 0) };
    assert_eq!(ready, WAIT_OBJECT_0);
    let mut out = [Completion::default(); 1];
    let info = backend.turn(Some(Duration::ZERO), &mut out)?;
    assert_eq!(info.waits, 0);
    assert!(info.notified);
    assert_eq!(notifier.syscall_count(), 1);
    Ok(())
}
