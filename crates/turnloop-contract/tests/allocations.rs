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
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::{Duration, Instant},
};
use turnloop::*;
struct Counting;
thread_local! { static ACTIVE: Cell<bool> = const { Cell::new(false) }; static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
fn record() {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
    }
}
// SAFETY: all allocation calls are forwarded unchanged to System. Counters only
// access already initialized thread-local Cells and never allocate themselves.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: GlobalAlloc's caller supplies a valid layout, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: GlobalAlloc's caller supplies a valid layout, forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        // SAFETY: pointer/layout and new size are forwarded under GlobalAlloc's contract.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: pointer and layout belong to a previous System allocation.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;
fn exchange(
    l: &mut Loop,
    a: Handle,
    b: Handle,
    output: &mut [u8; 64],
    out: &mut Completions,
    pooled: bool,
) -> usize {
    static INPUT: [u8; 64] = [0x9b; 64];
    let buf = if pooled {
        ReadBuf::Pooled
    } else {
        // SAFETY: the output region stays in place and is not read until completion.
        ReadBuf::Provided(unsafe { IoBufMut::from_raw_parts(output.as_mut_ptr(), output.len()) })
    };
    l.read(b, buf, Token(1)).expect("read");
    // SAFETY: INPUT is static immutable memory, valid through every completion.
    let buf = unsafe { IoBuf::from_raw_parts(INPUT.as_ptr(), INPUT.len()) };
    l.write(a, WriteBuf::Provided(buf), Token(2))
        .expect("write");
    let h = l
        .timer(Instant::now() + Duration::from_secs(10), None, Token(3))
        .expect("timer");
    assert!(l.cancel(l.timer_op(h).expect("timer op")));
    l.close(h, Token(4)).expect("close timer");
    let mut read = 0;
    let mut wrote = 0;
    let mut cancelled = 0;
    let mut closed = 0;
    let until = Instant::now() + Duration::from_secs(2);
    while read < 64 || wrote == 0 || cancelled == 0 || closed == 0 {
        assert!(Instant::now() < until);
        l.turn(Timeout::Until(until), out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease } => {
                    assert!(n > 0);
                    if let Some(data) = lease {
                        assert!(data.as_slice().iter().all(|&v| v == 0x9b));
                    } else {
                        assert!(output[read..read + n].iter().all(|&v| v == 0x9b));
                    }
                    read += n;
                    if read < 64 {
                        let buf = if pooled {
                            ReadBuf::Pooled
                        } else {
                            // SAFETY: previous read completed; the remaining exclusive
                            // region stays fixed until this next completion arrives.
                            ReadBuf::Provided(unsafe {
                                IoBufMut::from_raw_parts(output[read..].as_mut_ptr(), 64 - read)
                            })
                        };
                        l.read(b, buf, Token(1)).expect("continue");
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, 64);
                    wrote += 1;
                }
                OpResult::Cancelled => cancelled += 1,
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((read, wrote, cancelled, closed), (64, 1, 1, 1));
    read
}
fn timer_batch(l: &mut Loop, out: &mut Completions) -> usize {
    let mut handles = [None; 1000];
    let at = Instant::now() + Duration::from_secs(30);
    for (i, slot) in handles.iter_mut().enumerate() {
        let h = l
            .timer(at + Duration::from_nanos(i as u64), None, Token(20))
            .expect("batch timer");
        *slot = Some(h);
    }
    for h in handles.iter().flatten() {
        assert!(l.cancel(l.timer_op(*h).expect("batch op")));
        l.close(*h, Token(21)).expect("close batch timer");
    }
    let mut cancelled = 0;
    let mut closed = 0;
    while cancelled + closed < 2000 {
        l.turn(Timeout::Now, out).expect("batch delivery");
        assert!(!out.is_empty());
        for c in out.drain() {
            match c.result {
                OpResult::Cancelled => cancelled += 1,
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(cancelled, 1000);
    assert_eq!(closed, 1000);
    cancelled
}
#[test]
fn steady_read_write_timer_and_accept_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let (_, a, b) = turnloop_contract::pair(&mut l);
    let mut output = [0; 64];
    let mut out = Completions::default();
    for _ in 0..10 {
        exchange(&mut l, a, b, &mut output, &mut out, false);
        exchange(&mut l, a, b, &mut output, &mut out, true);
    }
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    let mut bytes = 0;
    for _ in 0..1000 {
        bytes += exchange(&mut l, a, b, &mut output, &mut out, false);
        bytes += exchange(&mut l, a, b, &mut output, &mut out, true);
    }
    ACTIVE.with(|v| v.set(false));
    let allocations = ALLOCS.with(Cell::get);
    assert_eq!(bytes, 128_000);
    assert_eq!(allocations, 0, "steady read/write/timer allocations");
    for _ in 0..3 {
        timer_batch(&mut l, &mut out);
    }
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    let mut timers = 0;
    for _ in 0..10 {
        timers += timer_batch(&mut l, &mut out);
    }
    ACTIVE.with(|v| v.set(false));
    let allocations = ALLOCS.with(Cell::get);
    assert_eq!(timers, 10_000);
    assert_eq!(
        allocations, 0,
        "steady batches of 1000 timers allocate nothing"
    );
    let listener = l
        .tcp_listen("127.0.0.1:0".parse().expect("addr"), &ListenOpts::default())
        .expect("listen");
    let addr = l.local_addr(listener).expect("addr");
    let mut accepted = 0;
    for i in 0..101 {
        let client = std::net::TcpStream::connect(addr).expect("client");
        ALLOCS.with(|v| v.set(0));
        ACTIVE.with(|v| v.set(true));
        l.accept(listener, Token(10)).expect("accept");
        let until = Instant::now() + Duration::from_secs(1);
        let mut conn = None;
        while conn.is_none() {
            assert!(Instant::now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                if let OpResult::Accepted { conn: h, .. } = c.result {
                    conn = Some(h);
                } else {
                    panic!("unexpected accept result");
                }
            }
        }
        ACTIVE.with(|v| v.set(false));
        let allocations = ALLOCS.with(Cell::get);
        if i > 0 {
            assert_eq!(allocations, 0, "steady accept allocations");
            accepted += 1;
        }
        l.close(conn.expect("accepted"), Token(11)).expect("close");
        l.turn(Timeout::Now, &mut out).expect("release");
        assert!(matches!(out[0].result, OpResult::Closed));
        drop(client);
    }
    assert_eq!(accepted, 100);
}

#[test]
fn cancellation_reserves_survive_a_full_event_backlog() {
    let mut l = Loop::new(Config {
        max_handles: 16,
        max_operations: 16,
        events_per_turn: 4,
        post_capacity: 128,
        ..Config::default()
    })
    .expect("loop");
    let mut handles = [None; 16];
    for h in &mut handles {
        *h = Some(
            l.timer(l.now(), Some(Duration::from_nanos(1)), Token(1))
                .expect("repeating timer"),
        );
    }
    let poster = l.poster();
    let mut out = Completions::with_capacity(1);
    let mut timers = 0;
    let mut posts = 0;
    for _ in 0..20 {
        for _ in 0..4 {
            poster.post(Token(2), Payload::U64(42)).expect("post");
        }
        l.turn(Timeout::Now, &mut out).expect("build backlog");
        for c in out.drain() {
            match c.result {
                OpResult::Timer => timers += 1,
                OpResult::Posted(Payload::U64(42)) => posts += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    for h in handles.into_iter().flatten() {
        l.close(h, Token(3)).expect("cancel and close");
    }
    let mut cancelled = 0;
    let mut closed = 0;
    for _ in 0..1024 {
        if cancelled == 16 && closed == 16 && posts == 80 {
            break;
        }
        l.turn(Timeout::Now, &mut out).expect("drain backlog");
        for c in out.drain() {
            match c.result {
                OpResult::Timer => timers += 1,
                OpResult::Posted(Payload::U64(42)) => posts += 1,
                OpResult::Cancelled => cancelled += 1,
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    ACTIVE.with(|v| v.set(false));
    let allocations = ALLOCS.with(Cell::get);
    assert!(timers > 0);
    assert_eq!((cancelled, closed, posts), (16, 16, 80));
    assert_eq!(allocations, 0, "cancel/close reserves under backpressure");
}

#[test]
fn ipc_handle_transfer_and_external_waits_allocate_nothing_after_setup() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let path = std::env::temp_dir().join(format!("tl-alloc-ipc-{}.sock", std::process::id()));
    let (_, a, b) = turnloop_contract::native_surface::pipe_pair(&mut l, &PipeName(path.clone()));
    let (_, source, _peer) = turnloop_contract::pair(&mut l);
    let condition = WaitCondition::new(0).expect("wait condition");
    // Start worker/helper infrastructure before steady state.
    let wait = l
        .external_wait(&condition, 0, None, Token(30))
        .expect("warm wait");
    assert!(l.cancel(wait));
    let mut out = Completions::default();
    while l
        .turn(Timeout::Now, &mut out)
        .expect("warm cancellation")
        .completions
        == 0
    {}
    let mut bytes = [0; 64];
    for _ in 0..4 {
        exchange(&mut l, a, b, &mut bytes, &mut out, true);
    }
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    let mut transferred = 0;
    let mut waits = 0;
    for _ in 0..200 {
        exchange(&mut l, a, b, &mut bytes, &mut out, true);
        l.send_handle(a, source, Token(10))
            .expect("send descriptor");
        l.recv_handle(b, Token(11)).expect("receive descriptor");
        let wait = l
            .external_wait(&condition, 0, None, Token(12))
            .expect("external wait");
        assert!(l.cancel(wait));
        let mut received = None;
        let mut sent = false;
        let mut cancelled = false;
        let deadline = l.now() + Duration::from_secs(2);
        while received.is_none() || !sent || !cancelled {
            assert!(l.now() < deadline);
            l.turn(Timeout::Until(deadline), &mut out)
                .expect("transfer turn");
            for c in out.drain() {
                match c.result {
                    OpResult::HandleSent => sent = true,
                    OpResult::HandleReceived { handle } => received = Some(handle),
                    OpResult::Cancelled => {
                        assert_eq!(c.op, Some(wait));
                        cancelled = true;
                        waits += 1;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        l.close(received.expect("received fd"), Token(13))
            .expect("close received fd");
        l.turn(Timeout::Now, &mut out).expect("close turn");
        assert!(out.iter().any(|c| matches!(c.result, OpResult::Closed)));
        transferred += 1;
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "IPC and external waits steady allocations"
    );
    assert_eq!((transferred, waits), (200, 200));
    std::fs::remove_file(path).expect("remove socket path");
}

#[test]
fn regular_file_jobs_reuse_pool_storage() {
    use std::{
        io::{Seek, SeekFrom, Write},
        os::fd::OwnedFd,
    };
    let path = std::env::temp_dir().join(format!("tl-alloc-file-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("file");
    file.write_all(&[9; 64]).expect("file bytes");
    let mut l = Loop::new(Config::default()).expect("loop");
    let fd: OwnedFd = file.try_clone().expect("clone file").into();
    let h = l
        .attach(Detached::from_fd(fd).expect("file transport"), Token(1))
        .expect("file attach");
    let mut out = Completions::default();
    let mut count = 0;
    let mut writes = 0;
    for i in 0..201 {
        file.seek(SeekFrom::Start(0))
            .expect("seek after completed read");
        if i == 1 {
            ALLOCS.with(|v| v.set(0));
            ACTIVE.with(|v| v.set(true));
        }
        l.read(h, ReadBuf::Pooled, Token(2)).expect("file read");
        let deadline = l.now() + Duration::from_secs(2);
        loop {
            assert!(l.now() < deadline);
            l.turn(Timeout::Until(deadline), &mut out)
                .expect("file turn");
            if !out.is_empty() {
                assert_eq!(out.len(), 1);
                let OpResult::Read {
                    n,
                    lease: Some(ref data),
                } = out[0].result
                else {
                    panic!("missing file bytes");
                };
                assert_eq!(n, 64);
                assert_eq!(data.as_slice(), [9; 64]);
                count += 1;
                break;
            }
        }
        file.seek(SeekFrom::Start(0)).expect("rewind for write");
        static OUTPUT: [u8; 64] = [9; 64];
        // SAFETY: static immutable bytes remain valid through native acknowledgement.
        let bytes = unsafe { IoBuf::from_raw_parts(OUTPUT.as_ptr(), OUTPUT.len()) };
        l.write(h, WriteBuf::Provided(bytes), Token(3))
            .expect("file write");
        loop {
            assert!(l.now() < deadline);
            l.turn(Timeout::Until(deadline), &mut out)
                .expect("file write turn");
            if !out.is_empty() {
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].token, Token(3));
                assert!(matches!(out[0].result, OpResult::Wrote(64)));
                writes += 1;
                break;
            }
        }
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "reusable file jobs allocate nothing"
    );
    assert_eq!((count, writes), (201, 201));
    std::fs::remove_file(path).expect("remove file");
}

#[test]
fn file_readiness_survives_pool_backpressure_without_allocations_or_spin() {
    use std::{io::Write, os::fd::OwnedFd};
    let path = std::env::temp_dir().join(format!("tl-file-backpressure-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("fixture");
    file.write_all(&[37; 64]).expect("fixture bytes");
    let mut l = Loop::new(Config {
        pooled_buffers: 1,
        pooled_buffer_size: 64,
        ..Config::default()
    })
    .expect("loop");
    let mut handles = [None; 3];
    for h in &mut handles {
        let fd: OwnedFd = std::fs::File::open(&path)
            .expect("independent file offset")
            .into();
        *h = Some(
            l.attach(Detached::from_fd(fd).expect("file"), Token(0))
                .expect("attach"),
        );
    }
    let mut out = Completions::with_capacity(1);
    l.read(handles[0].expect("first"), ReadBuf::Pooled, Token(1))
        .expect("first read");
    let until = l.now() + Duration::from_secs(5);
    let lease = loop {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out)
            .expect("warm file job");
        if let Some(c) = out.drain().next() {
            let OpResult::Read {
                n: 64,
                lease: Some(b),
            } = c.result
            else {
                panic!("first read did not execute");
            };
            assert_eq!(b.as_slice(), [37; 64]);
            break b;
        }
    };
    // The worker publishes bytes before sending its wake. Finish that warm-up
    // wake before the quiet deadline measurement (while retaining the lease).
    let warm = l
        .timer(l.now() + Duration::from_millis(10), None, Token(0))
        .expect("warm timer");
    loop {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out)
            .expect("settle warm-up wake");
        if !out.is_empty() {
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Timer));
            break;
        }
    }
    l.close(warm, Token(0)).expect("close warm timer");
    l.turn(Timeout::Now, &mut out).expect("drain warm close");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Closed));
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    let cancelled = l
        .read(handles[1].expect("second"), ReadBuf::Pooled, Token(2))
        .expect("blocked read");
    l.read(handles[2].expect("third"), ReadBuf::Pooled, Token(3))
        .expect("surviving read");
    let at = l.now() + Duration::from_millis(2);
    let timer = l.timer(at, None, Token(4)).expect("timer");
    assert_eq!(ALLOCS.with(Cell::get), 0, "file submission storage");
    let info = l
        .turn(Timeout::Until(at), &mut out)
        .expect("pool exhaustion wait");
    assert_eq!(info.os_waits, 1);
    assert_eq!(info.zero_event_waits, 1);
    assert!(l.now() >= at);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Timer));
    assert_eq!(ALLOCS.with(Cell::get), 0, "pool-blocked parking storage");
    assert!(l.cancel(cancelled));
    l.turn(Timeout::Now, &mut out)
        .expect("cancel without a lease");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].op, Some(cancelled));
    assert!(matches!(out[0].result, OpResult::Cancelled));
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "pool-blocked cancellation storage"
    );
    drop(lease);
    assert_eq!(ALLOCS.with(Cell::get), 0, "lease return storage");
    let mut reads = 0;
    while reads == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out)
            .expect("resume after lease release");
        for c in out.drain() {
            assert_eq!(c.token, Token(3));
            let OpResult::Read {
                n: 64,
                lease: Some(b),
            } = c.result
            else {
                panic!("surviving read did not execute");
            };
            assert_eq!(b.as_slice(), [37; 64]);
            reads += 1;
        }
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(reads, 1);
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "ready and pool-blocked file queues allocate nothing"
    );
    l.close(timer, Token(5)).expect("close timer");
    std::fs::remove_file(path).expect("remove fixture");
}

#[cfg(feature = "executor")]
#[test]
fn executor_steady_io_poll_and_sleep_allocate_nothing() {
    use futures_io::{AsyncRead, AsyncWrite};
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use turnloop::executor::LocalExecutor;
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let (_, a, b) = turnloop_contract::pair(&mut ex.driver());
    let mut a = ex.handle().io(a);
    let mut b = ex.handle().io(b);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut count = 0;
    for i in 0..1001 {
        if i == 1 {
            ALLOCS.with(|v| v.set(0));
            ACTIVE.with(|v| v.set(true));
        }
        let mut input = [0; 64];
        let output = [27; 64];
        let Poll::Ready(wrote) = Pin::new(&mut a).poll_write(&mut cx, &output) else {
            panic!("fresh write buffer")
        };
        assert_eq!(wrote.expect("buffer write"), 64);
        assert!(Pin::new(&mut a).poll_flush(&mut cx).is_pending());
        assert!(Pin::new(&mut b).poll_read(&mut cx, &mut input).is_pending());
        let mut sleep = ex.handle().sleep(Duration::ZERO);
        assert!(Pin::new(&mut sleep).poll(&mut cx).is_pending());
        let mut wrote = false;
        let mut read = false;
        let mut slept = false;
        let deadline = ex.driver().now() + Duration::from_secs(2);
        while !wrote || !read || !slept {
            assert!(ex.driver().now() < deadline);
            ex.turn(Timeout::Until(deadline)).expect("executor turn");
            if !wrote && let Poll::Ready(r) = Pin::new(&mut a).poll_flush(&mut cx) {
                r.expect("write completion");
                wrote = true;
            }
            if !read && let Poll::Ready(r) = Pin::new(&mut b).poll_read(&mut cx, &mut input) {
                assert_eq!(r.expect("read"), 64);
                assert_eq!(input, output);
                read = true;
            }
            if !slept && let Poll::Ready(r) = Pin::new(&mut sleep).poll(&mut cx) {
                r.expect("sleep");
                slept = true;
            }
        }
        count += 1;
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "executor steady I/O and timer polls allocate nothing"
    );
    assert_eq!(count, 1001);
}

#[test]
fn signal_exit_and_external_notification_delivery_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let signal = l.signal_start(Signal::Usr1, Token(1)).expect("signal");
    let condition = WaitCondition::new(0).expect("condition");
    let mut children = [None; 16];
    for child in &mut children {
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.args.push("sleep".into());
        *child = Some(l.spawn(&spec, Token(2)).expect("child"));
    }
    let mut out = Completions::default();
    // Warm notifier/TLS paths before measuring completion delivery, leaving the
    // already registered children alive. Process creation is resource setup.
    l.turn(Timeout::Now, &mut out).expect("warm services");
    assert!(out.is_empty());
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    for child in children.iter().flatten() {
        l.kill(child.handle, Signal::Kill).expect("kill child");
    }
    let mut exits = 0;
    let until = l.now() + Duration::from_secs(5);
    while exits != 16 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("exit turn");
        for c in out.drain() {
            assert_eq!(c.token, Token(2));
            let OpResult::Exited(status) = c.result else {
                panic!("missing exit")
            };
            assert_eq!(status.signal, Some(libc::SIGKILL));
            exits += 1;
        }
    }
    let (mut signals, mut waits) = (0, 0);
    for _ in 0..200 {
        let op = l
            .external_wait(&condition, 0, None, Token(3))
            .expect("wait");
        condition.notify();
        // SAFETY: SIGUSR1 has a live subscription and getpid names this process.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
        let (mut signaled, mut notified) = (false, false);
        let until = l.now() + Duration::from_secs(2);
        while !signaled || !notified {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out)
                .expect("service turn");
            for c in out.drain() {
                match c.result {
                    OpResult::Signal(Signal::Usr1) => {
                        assert_eq!(c.token, Token(1));
                        assert!(!signaled);
                        signaled = true;
                        signals += 1;
                    }
                    OpResult::ExternalWait(WaitResult::Notified) => {
                        assert_eq!(c.op, Some(op));
                        assert!(!notified);
                        notified = true;
                        waits += 1;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 0, "service completion allocations");
    assert_eq!((exits, signals, waits), (16, 200, 200));
    l.signal_stop(signal, Token(4)).expect("stop");
}

#[test]
fn quiet_deadline_waits_have_identical_accounting_without_allocations() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let mut expiries = 0;
    ALLOCS.with(|v| v.set(0));
    ACTIVE.with(|v| v.set(true));
    for duration in [
        Duration::from_micros(500),
        Duration::from_millis(2),
        Duration::from_millis(10),
    ] {
        for _ in 0..20 {
            let at = l.now() + duration;
            let token = Token(expiries);
            let h = l.timer(at, None, token).expect("timer");
            let op = l.timer_op(h).expect("timer operation");
            let info = l.turn(Timeout::Until(at), &mut out).expect("quiet wait");
            assert_eq!((info.os_waits, info.zero_event_waits), (1, 1));
            assert!(l.now() >= at);
            assert_eq!(out.len(), 1);
            assert_eq!((out[0].handle, out[0].op, out[0].token), (Some(h), Some(op), token));
            assert!(matches!(out[0].result, OpResult::Timer));
            l.close(h, token).expect("close timer");
            let info = l.turn(Timeout::Now, &mut out).expect("queued close");
            assert_eq!((info.os_waits, info.zero_event_waits), (0, 0));
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Closed));
            expiries += 1;
        }
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(expiries, 60);
    assert_eq!(ALLOCS.with(Cell::get), 0, "deadline waits reuse reserved storage");
}
