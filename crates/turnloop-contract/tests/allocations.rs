//! On p3, run this binary with --release. The pinned compiler's debug custom
//! allocator traps in pre-main get-arguments lowering; see docs/wasm.md. CI
//! requires release allocation counts and runs semantic/no-spin tests in both profiles.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "windows",
        all(
            target_os = "wasi",
            any(
                target_env = "p2",
                all(target_env = "p3", feature = "wasi-p3-experimental")
            )
        )
    )
))]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    time::{Duration, Instant},
};
use turnloop::*;
struct Counting;
use std::cell::Cell;
thread_local! { static ACTIVE: Cell<bool> = const { Cell::new(false) }; static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
#[cfg(windows)]
static ALL_THREADS_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(windows)]
static ALL_THREADS_ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn record() {
    #[cfg(windows)]
    if ALL_THREADS_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        ALL_THREADS_ALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
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
    let allocations = ALLOCS.with(|n| n.get());
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
    let allocations = ALLOCS.with(|n| n.get());
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
        let allocations = ALLOCS.with(|n| n.get());
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
    let allocations = ALLOCS.with(|n| n.get());
    assert!(timers > 0);
    assert_eq!((cancelled, closed, posts), (16, 16, 80));
    assert_eq!(allocations, 0, "cancel/close reserves under backpressure");
}

#[test]
fn steady_udp_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let addr = "127.0.0.1:0".parse().expect("address");
    let a = l.udp_bind(addr, &UdpOpts::default()).expect("UDP a");
    let b = l.udp_bind(addr, &UdpOpts::default()).expect("UDP b");
    let from = l.local_addr(a).expect("source");
    let to = l.local_addr(b).expect("destination");
    assert_ne!(from, to, "default UDP endpoints must be distinct");
    static BYTES: [u8; 64] = [0x42; 64];
    let mut out = Completions::default();
    let mut bytes = 0;
    let mut total = 0;
    for i in 0..101 {
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(i != 0));
        l.recv(b, ReadBuf::Pooled, Token(1)).expect("receive");
        // SAFETY: static immutable input stays alive through terminal completion.
        let input = unsafe { IoBuf::from_raw_parts(BYTES.as_ptr(), BYTES.len()) };
        l.send_to(a, WriteBuf::Provided(input), to, Token(2))
            .expect("send");
        let mut read = 0;
        let mut wrote = 0;
        let until = l.now() + Duration::from_secs(2);
        while read != 64 || wrote != 64 {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                match c.result {
                    OpResult::RecvFrom {
                        n,
                        from: actual,
                        lease: Some(b),
                    } => {
                        assert_eq!(actual, from, "unexpected UDP sender");
                        assert_eq!(n, BYTES.len());
                        assert_eq!(b.as_slice(), BYTES);
                        read += n;
                    }
                    OpResult::Wrote(n) => wrote += n,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        ACTIVE.with(|v| v.set(false));
        if i != 0 {
            bytes += read;
            total += ALLOCS.with(|n| n.get());
        }
    }
    assert_eq!(bytes, 6400, "UDP allocation subject ran");
    assert_eq!(total, 0, "steady UDP allocations");
}

#[test]
fn steady_deadline_poll_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    let mut expiries = 0;
    ALLOCS.with(|n| n.set(0));
    ACTIVE.with(|v| v.set(true));
    for _ in 0..20 {
        let at = l.now() + Duration::from_millis(2);
        let h = l.timer(at, None, Token(3)).expect("timer");
        while l.now() < at || expiries == 0 {
            l.turn(Timeout::Until(at), &mut out).expect("deadline poll");
            if out.iter().any(|c| matches!(c.result, OpResult::Timer)) {
                expiries += 1;
                break;
            }
        }
        // A clock crossing between checks must still deliver the timer.
        l.turn(Timeout::Now, &mut out).expect("deadline drain");
        expiries += out
            .iter()
            .filter(|c| matches!(c.result, OpResult::Timer))
            .count();
        l.close(h, Token(4)).expect("close");
        l.turn(Timeout::Now, &mut out).expect("close drain");
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(expiries, 20);
    assert_eq!(ALLOCS.with(|n| n.get()), 0, "deadline poll return lists");
}

#[test]
fn concurrent_udp_returns_survive_cancellation_and_loop_drop_without_allocating() {
    const N: usize = 8;
    const ROUNDS: usize = 20;
    const LENGTHS: [usize; N] = [0, 1, 7, 64, 513, 2048, 4096, 8192];
    let config = Config {
        pooled_buffer_size: 65536,
        pooled_buffers: 16,
        ..Config::default()
    };
    let mut loops = [
        Loop::new(config).expect("first loop"),
        Loop::new(config).expect("second loop"),
    ];
    let mut outputs = [[0u8; 65536]; N];
    // This Mac accepts 8 KiB datagrams and rejects 16 KiB with EMSGSIZE.
    // The maximum canonical buffer size is exercised separately without the OS.
    let payload: Vec<u8> = (0..8192).map(|i| (i % 251) as u8).collect();
    // Keep every socket bound through every round. Default UDP binds must also
    // be exclusive: SO_REUSEADDR used to allow live ephemeral-port collisions
    // on Linux even though none of these sockets had been closed.
    let mut sockets = Vec::new();
    let mut addresses = Vec::with_capacity(N * 2);
    for l in &mut loops {
        let mut group = Vec::new();
        for _ in 0..N {
            let h = l
                .udp_bind("[::1]:0".parse().expect("IPv6"), &UdpOpts::default())
                .expect("UDP");
            let addr = l.local_addr(h).expect("local address");
            assert!(
                !addresses.contains(&addr),
                "duplicate live UDP endpoint {addr}; previous endpoints: {addresses:?}"
            );
            addresses.push(addr);
            group.push((h, addr));
        }
        sockets.push(group);
    }
    eprintln!("concurrent UDP fixture handles/endpoints: {sockets:?}");
    let mut out = Completions::with_capacity(1);
    let mut received = 0;
    let mut cancelled = 0;
    let mut allocations = 0;
    for round in 0..=ROUNDS {
        let mut operations = [[None; N * 2]; 2];
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(round != 0));
        for (j, l) in loops.iter_mut().enumerate() {
            for &(h, _) in &sockets[j] {
                // Cancel an armed empty receive before queuing its successor.
                let op = l
                    .recv(h, ReadBuf::Pooled, Token(100))
                    .expect("cancel receive");
                l.turn(Timeout::Now, &mut out).expect("arm receive");
                assert!(out.is_empty());
                assert!(l.cancel(op));
                l.turn(Timeout::Now, &mut out)
                    .expect("cancel acknowledgement");
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].op, Some(op));
                assert_eq!(out[0].handle, Some(h));
                assert_eq!(out[0].token, Token(100));
                assert!(out[0].terminal);
                assert!(matches!(out[0].result, OpResult::Cancelled));
                cancelled += usize::from(round != 0);
            }
            for (i, &(h, to)) in sockets[j].iter().enumerate() {
                let buf = if j == 0 {
                    // SAFETY: each receive owns its distinct fixed output until
                    // all completions are drained below; storage never moves.
                    ReadBuf::Provided(unsafe {
                        IoBufMut::from_raw_parts(outputs[i].as_mut_ptr(), outputs[i].len())
                    })
                } else {
                    ReadBuf::Pooled
                };
                operations[j][i] = Some(l.recv(h, buf, Token(i as u64)).expect("receive"));
                // SAFETY: immutable retained payload lives until both loops drop.
                let input = unsafe { IoBuf::from_raw_parts(payload.as_ptr(), LENGTHS[i]) };
                operations[j][N + i] = Some(
                    l.send_to(h, WriteBuf::Provided(input), to, Token((N + i) as u64))
                        .expect("send"),
                );
            }
        }
        let mut seen = [[false; N * 2]; 2];
        let mut completed = 0;
        let until = Instant::now() + Duration::from_secs(5);
        while completed < N * 4 {
            assert!(Instant::now() < until, "UDP burst stalled");
            for (j, l) in loops.iter_mut().enumerate() {
                l.turn(Timeout::Now, &mut out).expect("interleave loops");
                for c in out.drain() {
                    let index = c.token.0 as usize;
                    assert!(
                        index < N * 2,
                        "unexpected UDP token: round={round} loop={j} completion={c:?}"
                    );
                    assert!(
                        !seen[j][index],
                        "duplicate UDP result: round={round} loop={j} completion={c:?}"
                    );
                    assert_eq!(
                        c.op, operations[j][index],
                        "UDP operation: round={round} loop={j} token={index}"
                    );
                    assert_eq!(
                        c.handle,
                        Some(sockets[j][index % N].0),
                        "UDP handle: round={round} loop={j} token={index}"
                    );
                    assert!(c.terminal, "UDP operations are one-shot");
                    seen[j][index] = true;
                    completed += 1;
                    match c.result {
                        OpResult::RecvFrom { n, from, lease } => {
                            assert!(index < N, "receive used a send token");
                            assert_eq!(
                                from, sockets[j][index].1,
                                "unexpected UDP sender: round={round} loop={j} token={index} op={:?} handle={:?} n={n}",
                                c.op, c.handle
                            );
                            assert_eq!(
                                n, LENGTHS[index],
                                "UDP length: round={round} loop={j} token={index} from={from} op={:?} handle={:?}",
                                c.op, c.handle
                            );
                            assert_eq!(lease.is_some(), j == 1, "UDP buffer mode");
                            // The other loop may still own outputs[index]. Only
                            // borrow it for its completed provided-buffer read;
                            // map_or would evaluate that borrow for pooled reads too.
                            let bytes = if let Some(lease) = &lease {
                                lease.as_slice()
                            } else {
                                &outputs[index][..n]
                            };
                            assert_eq!(bytes, &payload[..n]);
                            received += usize::from(round != 0);
                        }
                        OpResult::Wrote(n) => {
                            assert!(index >= N, "send used a receive token");
                            assert_eq!(n, LENGTHS[index - N]);
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                }
            }
        }
        ACTIVE.with(|v| v.set(false));
        if round != 0 {
            allocations += ALLOCS.with(|n| n.get());
        }
    }
    assert_eq!(received, ROUNDS * N * 2);
    assert_eq!(cancelled, ROUNDS * N * 2);
    assert_eq!(allocations, 0, "concurrent canonical UDP return storage");
    // Drop with multiple receives pending, then exercise a surviving/new loop.
    for (j, l) in loops.iter_mut().enumerate() {
        for &(h, _) in &sockets[j] {
            l.recv(h, ReadBuf::Pooled, Token(200))
                .expect("pending on drop");
        }
        l.turn(Timeout::Now, &mut out).expect("arm before drop");
        assert!(out.is_empty());
    }
    let [first, mut second] = loops;
    drop(first);
    for &(h, _) in &sockets[1] {
        second.close(h, Token(201)).expect("close surviving loop");
    }
    let mut cancellations = 0;
    let mut closes = 0;
    for _ in 0..N * 4 {
        second.turn(Timeout::Now, &mut out).expect("surviving loop");
        for c in out.drain() {
            match c.result {
                OpResult::Cancelled => cancellations += 1,
                OpResult::Closed => closes += 1,
                other => panic!("unexpected teardown {other:?}"),
            }
        }
    }
    assert_eq!((cancellations, closes), (N, N));
    steady_udp_allocate_nothing();
}

#[cfg(target_env = "p3")]
#[test]
fn wasi_random_scalar_imports_allocate_nothing() {
    let mut bytes = [0u8; 257];
    ALLOCS.with(|n| n.set(0));
    ACTIVE.with(|v| v.set(true));
    for _ in 0..100 {
        turnloop_wasi_random::fill_v03(&mut bytes).expect("entropy 0.3");
        turnloop_wasi_random::fill_v04(&mut bytes).expect("entropy 0.4");
        assert!(bytes.windows(2).any(|b| b[0] != b[1]));
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(ALLOCS.with(|n| n.get()), 0, "200 real entropy fills");
}

#[cfg(not(target_os = "wasi"))]
#[test]
fn ipc_handle_transfer_and_external_waits_allocate_nothing_after_setup() {
    let mut l = Loop::new(Config::default()).expect("loop");
    #[cfg(unix)]
    let path = std::env::temp_dir().join(format!("tl-alloc-ipc-{}.sock", std::process::id()));
    #[cfg(windows)]
    let path = std::path::PathBuf::from(format!(r"\\.\pipe\tl-alloc-ipc-{}", std::process::id()));
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
        ALLOCS.with(|n| n.get()),
        0,
        "IPC and external waits steady allocations"
    );
    assert_eq!((transferred, waits), (200, 200));
    #[cfg(unix)]
    std::fs::remove_file(path).expect("remove socket path");
}

#[cfg(not(target_os = "wasi"))]
#[test]
fn regular_file_jobs_reuse_pool_storage() {
    use std::io::{Seek, SeekFrom, Write};
    let path = std::env::temp_dir().join(format!("tl-alloc-file-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("file");
    file.write_all(&[9; 64]).expect("file bytes");
    let mut l = Loop::new(Config::default()).expect("loop");
    #[cfg(unix)]
    let transport = Detached::from_fd(file.try_clone().expect("clone file").into());
    #[cfg(windows)]
    let transport = Detached::from_handle(file.try_clone().expect("clone file").into());
    let h = l
        .attach(transport.expect("file transport"), Token(1))
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
        ALLOCS.with(|n| n.get()),
        0,
        "reusable file jobs allocate nothing"
    );
    assert_eq!((count, writes), (201, 201));
    std::fs::remove_file(path).expect("remove file");
}

#[cfg(not(target_os = "wasi"))]
#[test]
fn file_readiness_survives_pool_backpressure_without_allocations_or_spin() {
    use std::io::Write;
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
        let file = std::fs::File::open(&path).expect("independent file offset");
        #[cfg(unix)]
        let detached = Detached::from_fd(file.into()).expect("file");
        #[cfg(windows)]
        let detached = Detached::from_handle(file.into()).expect("file");
        *h = Some(l.attach(detached, Token(0)).expect("attach"));
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
        ALLOCS.with(|n| n.get()),
        0,
        "executor steady I/O and timer polls allocate nothing"
    );
    assert_eq!(count, 1001);
}

#[cfg(unix)]
#[test]
fn signal_exit_and_external_notification_delivery_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let signal = l.signal_start(Signal::Usr1, Token(1)).expect("signal");
    let condition = WaitCondition::new(0).expect("condition");
    let mut children = [None; 16];
    for child in &mut children {
        let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
        spec.windows_hide = true;
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
    assert_eq!(
        ALLOCS.with(|n| n.get()),
        0,
        "service completion allocations"
    );
    assert_eq!((exits, signals, waits), (16, 200, 200));
    l.signal_stop(signal, Token(4)).expect("stop");
}

#[cfg(not(target_os = "wasi"))]
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
            assert_eq!(
                (out[0].handle, out[0].op, out[0].token),
                (Some(h), Some(op), token)
            );
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
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "deadline waits reuse reserved storage"
    );
}

#[cfg(target_os = "wasi")]
#[test]
fn single_agent_external_waits_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let condition = WaitCondition::new(0).expect("condition setup");
    let mut out = Completions::with_capacity(1);
    let mut results = 0;
    ALLOCS.with(|n| n.set(0));
    ACTIVE.with(|v| v.set(true));
    for _ in 0..200 {
        for kind in 0..4 {
            let op = l
                .external_wait(&condition, u64::from(kind == 0), Some(l.now()), Token(kind))
                .expect("wait");
            if kind == 1 {
                condition.notify();
            }
            if kind == 2 {
                assert!(l.cancel(op));
            }
            l.turn(Timeout::Now, &mut out).expect("completion");
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].op, Some(op));
            assert!(match kind {
                0 => matches!(out[0].result, OpResult::ExternalWait(WaitResult::NotEqual)),
                1 => matches!(out[0].result, OpResult::ExternalWait(WaitResult::Notified)),
                2 => matches!(out[0].result, OpResult::Cancelled),
                _ => matches!(out[0].result, OpResult::ExternalWait(WaitResult::TimedOut)),
            });
            results += 1;
        }
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(
        ALLOCS.with(|n| n.get()),
        0,
        "single-agent external wait allocations"
    );
    assert_eq!(results, 800);
    assert!(!l.alive());
}

#[cfg(target_os = "wasi")]
#[test]
fn stdio_writes_reuse_stream_storage() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let h = l.open_stdio(Stdio::Stderr).expect("stderr setup");
    let mut out = Completions::with_capacity(1);
    let mut wrote = 0;
    let mut allocations = 0;
    for round in 0..101 {
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(round != 0));
        // SAFETY: static input lives until every completion and loop teardown.
        let bytes = unsafe { IoBuf::from_raw_parts(b".".as_ptr(), 1) };
        l.write(h, WriteBuf::Provided(bytes), Token(1))
            .expect("stdio write");
        let until = l.now() + Duration::from_secs(2);
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("stdio turn");
            if !out.is_empty() {
                assert!(matches!(out[0].result, OpResult::Wrote(1)));
                break;
            }
        }
        ACTIVE.with(|v| v.set(false));
        if round != 0 {
            allocations += ALLOCS.with(|n| n.get());
            wrote += 1;
        }
    }
    assert_eq!(wrote, 100);
    assert_eq!(allocations, 0, "steady stdio writes");
}

#[cfg(target_os = "wasi")]
#[test]
fn stdio_reads_reuse_caller_buffers_through_eof() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let h = l.open_stdio(Stdio::Stdin).expect("stdin setup");
    let expected = b"turnloop revision two stdin fixture\n";
    let mut byte = [0u8; 1];
    let mut out = Completions::with_capacity(1);
    let mut allocations = 0;
    let mut reads = 0;
    for (i, &value) in expected.iter().enumerate() {
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(i != 0));
        // SAFETY: the one-byte caller region remains exclusive and fixed until
        // the terminal read below; it is only inspected after acknowledgement.
        let buf = unsafe { IoBufMut::from_raw_parts(byte.as_mut_ptr(), byte.len()) };
        l.read(h, ReadBuf::Provided(buf), Token(1))
            .expect("stdin read");
        let until = l.now() + Duration::from_secs(2);
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("stdin turn");
            if !out.is_empty() {
                assert!(matches!(
                    out[0].result,
                    OpResult::Read { n: 1, lease: None }
                ));
                break;
            }
        }
        assert_eq!(byte[0], value);
        ACTIVE.with(|v| v.set(false));
        if i != 0 {
            allocations += ALLOCS.with(|n| n.get());
        }
        reads += 1;
    }
    let mut eofs = 0;
    ALLOCS.with(|n| n.set(0));
    ACTIVE.with(|v| v.set(true));
    for _ in 0..2 {
        l.read(h, ReadBuf::Pooled, Token(2)).expect("EOF read");
        let until = l.now() + Duration::from_secs(2);
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("EOF turn");
            if !out.is_empty() {
                assert!(matches!(out[0].result, OpResult::Eof));
                eofs += 1;
                break;
            }
        }
    }
    ACTIVE.with(|v| v.set(false));
    allocations += ALLOCS.with(|n| n.get());
    assert_eq!((reads, eofs), (36, 2));
    assert_eq!(allocations, 0, "stdio reads and repeated CLI EOF results");
}

#[cfg(windows)]
#[test]
fn windows_kill_repeat_and_immediate_close_allocate_nothing_after_setup() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.windows_hide = true;
    spec.args.push("sleep".into());
    spec.stdio = [ProcessStdio::Null; 3];
    // Cover a plain process, a process-only kill in a job, and a whole-job kill.
    let children: [_; 18] = std::array::from_fn(|i| {
        spec.new_process_group = i % 3 != 0;
        driver.spawn(&spec, Token(i as u64)).expect("child")
    });
    let waits: [OwnedHandle; 18] = std::array::from_fn(|i| {
        // SAFETY: driver's owned child pins the PID; open only a wait handle.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, children[i].pid) };
        assert!(!raw.is_null());
        // SAFETY: successful OpenProcess transferred this handle's ownership.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    });
    let mut out = Completions::with_capacity(1);
    driver.turn(Timeout::Now, &mut out).expect("warm services");
    assert!(out.is_empty());
    let mut cancelled = [false; 18];
    let mut closed = [false; 18];
    let (mut kills, mut repeats, mut cancellations, mut closes) = (0, 0, 0, 0);
    let deadline = driver.now() + Duration::from_secs(10);
    ALLOCS.with(|count| count.set(0));
    ACTIVE.with(|active| active.set(true));
    for (i, child) in children.iter().enumerate() {
        assert_eq!(
            // SAFETY: owned duplicate, nonblocking query proves a live kill subject.
            unsafe { WaitForSingleObject(waits[i].as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        if i % 3 == 2 {
            driver
                .kill_group(child.handle, Signal::Kill)
                .expect("kill job");
            assert_eq!(
                driver
                    .kill_group(child.handle, Signal::Kill)
                    .expect_err("repeat job kill")
                    .kind,
                ErrorKind::NotFound
            );
        } else {
            driver
                .kill(child.handle, Signal::Kill)
                .expect("kill process");
            assert_eq!(
                driver
                    .kill(child.handle, Signal::Kill)
                    .expect_err("repeat process kill")
                    .kind,
                ErrorKind::NotFound
            );
        }
        kills += 1;
        repeats += 1;
        driver
            .close(child.handle, Token(i as u64))
            .expect("immediate close");
    }
    while driver.alive() {
        assert!(driver.now() < deadline, "termination deadline");
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("close delivery");
        for completion in out.drain() {
            let i = completion.token.0 as usize;
            assert!(i < children.len());
            assert_eq!(completion.handle, Some(children[i].handle));
            assert!(completion.terminal);
            match completion.result {
                OpResult::Cancelled => {
                    assert!(completion.op.is_some());
                    assert!(!closed[i]);
                    assert!(!std::mem::replace(&mut cancelled[i], true));
                    cancellations += 1;
                }
                OpResult::Closed => {
                    assert!(completion.op.is_none());
                    assert!(cancelled[i]);
                    assert!(!std::mem::replace(&mut closed[i], true));
                    assert_eq!(
                        // SAFETY: owned duplicate survives Closed; child must have exited.
                        unsafe { WaitForSingleObject(waits[i].as_raw_handle(), 0) },
                        WAIT_OBJECT_0
                    );
                    closes += 1;
                }
                other => panic!("unexpected child completion: {other:?}"),
            }
        }
    }
    driver
        .turn(Timeout::Now, &mut out)
        .expect("no duplicate completion");
    assert!(out.is_empty());
    ACTIVE.with(|active| active.set(false));
    assert_eq!(ALLOCS.with(Cell::get), 0, "kill/repeat/close allocations");
    assert_eq!((kills, repeats, cancellations, closes), (18, 18, 18, 18));
}

#[cfg(windows)]
#[test]
fn windows_child_watch_cancel_exit_and_close_allocate_nothing_after_setup() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
    spec.windows_hide = true;
    spec.args.push("sleep".into());
    spec.stdio = [ProcessStdio::Null; 3];
    let children: [_; 16] =
        std::array::from_fn(|i| driver.spawn(&spec, Token(i as u64)).expect("child"));
    let waits: [OwnedHandle; 16] = std::array::from_fn(|i| {
        // SAFETY: driver's owned process handle prevents PID reuse; wait-only access.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, children[i].pid) };
        assert!(!raw.is_null());
        // SAFETY: successful OpenProcess transfers unique ownership.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    });
    let mut out = Completions::default();
    driver.turn(Timeout::Now, &mut out).expect("warm services");
    assert!(out.is_empty());
    let mut terminal = [false; 16];
    let mut closed = [false; 16];
    let (mut cancellations, mut exits, mut closes) = (0, 0, 0);
    ALLOCS.with(|count| count.set(0));
    ACTIVE.with(|active| active.set(true));
    for child in &children[..8] {
        assert_eq!(
            driver
                .detach(child.handle)
                .expect_err("cancel exit watch")
                .kind,
            ErrorKind::WouldBlock
        );
    }
    let info = driver
        .turn(Timeout::After(Duration::from_secs(2)), &mut out)
        .expect("cancel watch turn");
    assert_eq!(info.os_waits, 0);
    assert_eq!(out.len(), 8);
    for completion in out.drain() {
        let i = completion.token.0 as usize;
        assert!(i < 8);
        assert_eq!(completion.handle, Some(children[i].handle));
        assert!(completion.terminal && completion.op.is_some());
        assert!(matches!(completion.result, OpResult::Cancelled));
        assert!(!std::mem::replace(&mut terminal[i], true));
        cancellations += 1;
    }
    for (child, wait) in children.iter().zip(&waits) {
        assert_eq!(
            // SAFETY: live process query proves cancellation did not kill it.
            unsafe { WaitForSingleObject(wait.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        driver.kill(child.handle, Signal::Kill).expect("kill");
    }
    // Cancelled watches no longer report exit; the fixture explicitly waits on
    // all owned identities before asking close to release those process resources.
    for wait in &waits {
        assert_eq!(
            // SAFETY: owned wait handle, bounded native wait after termination request.
            unsafe { WaitForSingleObject(wait.as_raw_handle(), 10_000) },
            WAIT_OBJECT_0
        );
    }
    let deadline = driver.now() + Duration::from_secs(10);
    while exits < 8 {
        assert!(driver.now() < deadline);
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("exit delivery");
        for completion in out.drain() {
            let i = completion.token.0 as usize;
            assert!((8..16).contains(&i));
            assert_eq!(completion.handle, Some(children[i].handle));
            assert!(completion.terminal && completion.op.is_some());
            assert!(matches!(
                completion.result,
                OpResult::Exited(ExitStatus {
                    code: Some(1),
                    signal: None
                })
            ));
            assert!(!std::mem::replace(&mut terminal[i], true));
            exits += 1;
        }
    }
    for (i, child) in children.iter().enumerate() {
        driver.close(child.handle, Token(i as u64)).expect("close");
    }
    while driver.alive() {
        assert!(driver.now() < deadline);
        driver
            .turn(Timeout::Until(deadline), &mut out)
            .expect("close delivery");
        for completion in out.drain() {
            let i = completion.token.0 as usize;
            assert!(i < 16 && terminal[i]);
            assert_eq!(completion.handle, Some(children[i].handle));
            assert!(completion.terminal && completion.op.is_none());
            assert!(matches!(completion.result, OpResult::Closed));
            assert!(!std::mem::replace(&mut closed[i], true));
            closes += 1;
        }
    }
    ACTIVE.with(|active| active.set(false));
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "Windows service completion allocations"
    );
    assert_eq!((cancellations, exits, closes), (8, 8, 16));
}

#[cfg(windows)]
#[test]
fn windows_backlog_accept_rearm_and_busy_deadlines_allocate_nothing() {
    const N: usize = 8;
    let name = PipeName(format!(r"\\.\pipe\tl-backlog-alloc-{}", std::process::id()).into());
    let mut server = Loop::new(Config::default()).expect("server");
    let listener = server
        .pipe_listen(
            &name,
            &ListenOpts {
                backlog: N as u32,
                ..ListenOpts::default()
            },
        )
        .expect("backlog");
    let mut out = Completions::with_capacity(1);
    let mut accepts = 0;
    for _ in 0..16 {
        let _clients: [std::fs::File; N] = std::array::from_fn(|_| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name.0)
                .expect("fill backlog before accepts")
        });
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(true));
        for _ in 0..N {
            let op = server.accept(listener, Token(1)).expect("accept");
            let until = server.now() + Duration::from_secs(5);
            let conn = loop {
                assert!(server.now() < until);
                server
                    .turn(Timeout::Until(until), &mut out)
                    .expect("accept/rearm");
                if out.is_empty() {
                    continue;
                }
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].op, Some(op));
                let OpResult::PipeAccepted { conn } = out[0].result else {
                    panic!("accept missing")
                };
                break conn;
            };
            server.close(conn, Token(2)).expect("close accepted pipe");
            server
                .turn(Timeout::Now, &mut out)
                .expect("close completion");
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Closed));
            accepts += 1;
        }
        ACTIVE.with(|v| v.set(false));
        assert_eq!(ALLOCS.with(Cell::get), 0, "backlog accept/rearm storage");
    }
    assert_eq!(accepts, 128);
    let _occupied: [std::fs::File; N] = std::array::from_fn(|_| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name.0)
            .expect("occupy backlog")
    });
    let mut client = Loop::new(Config::default()).expect("client");
    let mut expiries = 0;
    for _ in 0..16 {
        let at = client.now() + Duration::from_millis(20);
        let h = client
            .pipe_connect_until(&name, at, Token(3))
            .expect("busy open setup");
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(true));
        let mut waits = 0;
        loop {
            assert!(client.now() < at + Duration::from_secs(3));
            waits += client
                .turn(Timeout::Until(at + Duration::from_secs(3)), &mut out)
                .expect("busy wait expiry")
                .os_waits;
            if out.is_empty() {
                continue;
            }
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].handle, Some(h));
            assert!(matches!(
                out[0].result,
                OpResult::Err(Error {
                    kind: ErrorKind::TimedOut,
                    ..
                })
            ));
            break;
        }
        client.close(h, Token(4)).expect("close timed-out pipe");
        client
            .turn(Timeout::Now, &mut out)
            .expect("release wait state");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Closed));
        ACTIVE.with(|v| v.set(false));
        assert!(waits > 0, "availability/deadline wait subject ran");
        assert_eq!(
            ALLOCS.with(Cell::get),
            0,
            "busy wait/cancel/deadline storage"
        );
        expiries += 1;
    }
    assert_eq!(expiries, 16);
}

#[cfg(windows)]
#[test]
fn windows_worker_file_fifo_allocate_nothing_after_setup() {
    use std::io::{Read, Seek};
    let path = std::env::temp_dir().join(format!("tl-file-fifo-alloc-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("file");
    let payload: [[u8; 32]; 64] = std::array::from_fn(|i| [i as u8; 32]);
    let mut driver = Loop::new(Config::default()).expect("loop");
    let h = driver
        .attach(
            Detached::from_handle(file.try_clone().expect("duplicate").into())
                .expect("synchronous file"),
            Token(0),
        )
        .expect("worker setup");
    let mut out = Completions::with_capacity(1);
    let mut writes = 0;
    for round in 0..5 {
        file.rewind().expect("rewind completed file");
        ALLOCS.with(|n| n.set(0));
        ACTIVE.with(|v| v.set(round != 0));
        let ops: [_; 64] = std::array::from_fn(|i| {
            // SAFETY: immutable fixed payload outlives the driver on success/unwind.
            let bytes = unsafe { IoBuf::from_raw_parts(payload[i].as_ptr(), 32) };
            driver
                .write(h, WriteBuf::Provided(bytes), Token(i as u64))
                .expect("FIFO write")
        });
        let mut count = 0;
        let deadline = driver.now() + Duration::from_secs(5);
        while count < 64 {
            assert!(driver.now() < deadline);
            driver
                .turn(Timeout::Until(deadline), &mut out)
                .expect("FIFO delivery");
            for c in out.drain() {
                assert_eq!(
                    (c.op, c.token, c.handle, c.terminal),
                    (Some(ops[count]), Token(count as u64), Some(h), true)
                );
                assert!(matches!(c.result, OpResult::Wrote(32)));
                count += 1;
            }
        }
        ACTIVE.with(|v| v.set(false));
        assert_eq!(ALLOCS.with(Cell::get), 0, "worker FIFO allocation gate");
        file.rewind().expect("rewind for independent content check");
        let mut bytes = [[0u8; 32]; 64];
        for chunk in &mut bytes {
            file.read_exact(chunk).expect("file bytes");
        }
        assert_eq!(bytes, payload);
        writes += if round == 0 { 0 } else { count };
    }
    assert_eq!(writes, 256);
    drop(driver);
    drop(file);
    std::fs::remove_file(path).expect("remove file");
}

#[cfg(windows)]
#[test]
fn windows_console_control_delivery_allocates_nothing_on_any_thread() {
    use std::{
        os::windows::{io::AsRawHandle, process::CommandExt},
        sync::atomic::Ordering,
    };
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::{
            Console::*,
            Threading::{CREATE_NEW_CONSOLE, WaitForSingleObject},
        },
    };
    const NAME: &str = "windows_console_control_delivery_allocates_nothing_on_any_thread";
    if std::env::var("TURNLOOP_CONSOLE_ALLOCATION_TEST").as_deref() != Ok(NAME) {
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
                .env("TURNLOOP_CONSOLE_ALLOCATION_TEST", NAME)
                .creation_flags(CREATE_NEW_CONSOLE)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("isolated console allocator child");
        // SAFETY: owned child handle, bounded wait; this child owns its console.
        let waited = unsafe { WaitForSingleObject(child.as_raw_handle(), 30_000) };
        if waited != WAIT_OBJECT_0 {
            child.kill().expect("watchdog kill");
        }
        let output = child.wait_with_output().expect("child output");
        assert_eq!(waited, WAIT_OBJECT_0, "console fixture timeout");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("fixture stdout");
        assert!(stdout.contains("1 passed"));
        assert!(stdout.contains("console allocation subject: 200 deliveries, 2 stops, 2 closes"));
        return;
    }
    let mut driver = Loop::new(Config::default()).expect("loop");
    // SAFETY: this test's isolated console; enable real Ctrl-C delivery.
    assert_ne!(unsafe { SetConsoleCtrlHandler(None, 0) }, 0);
    let signals = [
        driver.signal_start(Signal::Int, Token(1)).expect("Ctrl-C"),
        driver
            .signal_start(Signal::Break, Token(2))
            .expect("Ctrl-Break"),
    ];
    let mut out = Completions::with_capacity(1);
    let mut delivered = 0;
    for round in 0..101 {
        if round == 1 {
            ALL_THREADS_ACTIVE.store(true, Ordering::SeqCst);
            let calibration = std::hint::black_box(Box::new([0u8; 32]));
            drop(std::hint::black_box(calibration));
            assert!(
                ALL_THREADS_ALLOCS.load(Ordering::SeqCst) > 0,
                "global allocator calibration ran"
            );
            ALL_THREADS_ALLOCS.store(0, Ordering::SeqCst);
        }
        for (i, (control, signal)) in [
            (CTRL_C_EVENT, Signal::Int),
            (CTRL_BREAK_EVENT, Signal::Break),
        ]
        .into_iter()
        .enumerate()
        {
            // SAFETY: private console and live subscription; OS invokes the real
            // handler on its own thread, covered by the process-wide counter.
            assert_ne!(unsafe { GenerateConsoleCtrlEvent(control, 0) }, 0);
            let until = driver.now() + Duration::from_secs(5);
            loop {
                assert!(driver.now() < until);
                driver
                    .turn(Timeout::Until(until), &mut out)
                    .expect("console delivery");
                if out.is_empty() {
                    continue;
                }
                assert_eq!(out.len(), 1);
                let c = &out[0];
                assert_eq!(
                    (c.handle, c.token, c.terminal),
                    (Some(signals[i]), Token(i as u64 + 1), false)
                );
                assert!(matches!(c.result, OpResult::Signal(actual) if actual == signal));
                delivered += usize::from(round != 0);
                break;
            }
        }
    }
    for signal in signals {
        driver
            .signal_stop(signal, Token(3))
            .expect("stop subscription");
    }
    let (mut stops, mut closes) = (0, 0);
    let until = driver.now() + Duration::from_secs(5);
    while driver.alive() {
        assert!(driver.now() < until);
        driver
            .turn(Timeout::Until(until), &mut out)
            .expect("join handlers on stop");
        for c in out.drain() {
            match c.result {
                OpResult::Stopped => stops += 1,
                OpResult::Closed => closes += 1,
                other => panic!("duplicate/unexpected {other:?}"),
            }
        }
    }
    ALL_THREADS_ACTIVE.store(false, Ordering::SeqCst);
    assert_eq!(
        ALL_THREADS_ALLOCS.load(Ordering::SeqCst),
        0,
        "console handler/delivery/stop Rust allocations on any thread"
    );
    assert_eq!((delivered, stops, closes), (200, 2, 2));
    println!("console allocation subject: 200 deliveries, 2 stops, 2 closes");
}

#[cfg(windows)]
#[test]
fn windows_sync_pipe_character_and_disk_workers_reuse_classification_without_allocating() {
    use std::{
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
        sync::atomic::Ordering,
    };
    use windows_sys::Win32::{
        Storage::FileSystem::{FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType},
        System::Pipes::CreatePipe,
    };
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: valid outputs for non-inherited synchronous anonymous pipe ends.
    assert_ne!(
        // SAFETY: valid outputs for non-inherited synchronous anonymous pipe ends.
        unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 4096) },
        0
    );
    // SAFETY: successful CreatePipe transfers two distinct owned handles.
    let (read, write) = unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    };
    let null = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("NUL")
        .expect("character device");
    let path = std::env::temp_dir().join(format!("turnloop-sync-kinds-{}", std::process::id()));
    let disk = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("disk file");
    for (handle, kind) in [
        (read.as_raw_handle(), FILE_TYPE_PIPE),
        (write.as_raw_handle(), FILE_TYPE_PIPE),
        (null.as_raw_handle(), FILE_TYPE_CHAR),
        (disk.as_raw_handle(), FILE_TYPE_DISK),
    ] {
        // SAFETY: all handles remain owned and quiescent during classification.
        assert_eq!(unsafe { GetFileType(handle) }, kind);
    }
    let mut byte = [0xa5];
    let mut driver = Loop::new(Config::default()).expect("loop");
    let mut adopt = |handle| {
        driver
            .attach(
                Detached::from_handle(handle).expect("sync classification"),
                Token(0),
            )
            .expect("adoption")
    };
    let read = adopt(read);
    let write = adopt(write);
    let null = adopt(null.into());
    let disk = adopt(disk.into());
    let mut out = Completions::with_capacity(1);
    ALL_THREADS_ALLOCS.store(0, Ordering::SeqCst);
    ALL_THREADS_ACTIVE.store(true, Ordering::SeqCst);
    let calibration = Box::new([0u8; 1024]);
    std::hint::black_box(&calibration);
    ALL_THREADS_ACTIVE.store(false, Ordering::SeqCst);
    assert!(
        ALL_THREADS_ALLOCS.load(Ordering::SeqCst) > 0,
        "allocator calibration"
    );
    drop(calibration);
    let mut completions = [0; 3];
    for round in 0..129 {
        ALL_THREADS_ALLOCS.store(0, Ordering::SeqCst);
        ALL_THREADS_ACTIVE.store(round != 0, Ordering::SeqCst);
        for (kind, (reader, writer)) in [(read, write), (null, null), (disk, disk)]
            .into_iter()
            .enumerate()
        {
            // NUL and the regular file return EOF at their current position;
            // anonymous pipe input stays pending until its separate end writes.
            // SAFETY: fixed byte stays exclusive until the read acknowledgement.
            let buffer = unsafe { IoBufMut::from_raw_parts(byte.as_mut_ptr(), 1) };
            let input = driver
                .read(reader, ReadBuf::Provided(buffer), Token(1))
                .expect("read");
            let deadline = driver.now() + Duration::from_secs(3);
            if kind != 0 {
                loop {
                    assert!(driver.now() < deadline);
                    driver
                        .turn(Timeout::Until(deadline), &mut out)
                        .expect("EOF turn");
                    if !out.is_empty() {
                        break;
                    }
                }
                assert_eq!(
                    (out[0].handle, out[0].op, out[0].token, out[0].terminal),
                    (Some(reader), Some(input), Token(1), true)
                );
                assert!(matches!(out[0].result, OpResult::Eof));
            }
            // SAFETY: static immutable payload lives through every acknowledgement.
            let buffer = unsafe { IoBuf::from_raw_parts(b"X".as_ptr(), 1) };
            let output = driver
                .write(writer, WriteBuf::Provided(buffer), Token(2))
                .expect("write");
            let (mut reads, mut writes) = (usize::from(kind != 0), 0);
            while reads != 1 || writes != 1 {
                assert!(driver.now() < deadline);
                driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("worker turn");
                for c in out.drain() {
                    assert!(c.terminal);
                    match c.result {
                        OpResult::Read { n: 1, lease: None } => {
                            assert_eq!(kind, 0);
                            assert_eq!(
                                (c.handle, c.op, c.token),
                                (Some(reader), Some(input), Token(1))
                            );
                            assert_eq!(reads, 0);
                            reads += 1;
                        }
                        OpResult::Wrote(1) => {
                            assert_eq!(
                                (c.handle, c.op, c.token),
                                (Some(writer), Some(output), Token(2))
                            );
                            assert_eq!(writes, 0);
                            writes += 1;
                        }
                        other => panic!("unexpected worker result: {other:?}"),
                    }
                }
            }
            if kind == 0 {
                assert_eq!(byte, *b"X");
            }
            if round != 0 {
                completions[kind] += reads + writes;
            }
        }
        ALL_THREADS_ACTIVE.store(false, Ordering::SeqCst);
        assert_eq!(
            ALL_THREADS_ALLOCS.load(Ordering::SeqCst),
            0,
            "all worker threads reuse classification"
        );
    }
    assert_eq!(completions, [256; 3]);
    drop(driver);
    assert_eq!(
        std::fs::read(&path).expect("actual disk bytes"),
        [b'X'; 129]
    );
    std::fs::remove_file(path).expect("remove fixture");
}

#[cfg(windows)]
#[test]
fn windows_duplex_fifos_make_independent_progress_without_allocating() {
    use std::sync::atomic::Ordering;
    ALL_THREADS_ALLOCS.store(0, Ordering::SeqCst);
    ALL_THREADS_ACTIVE.store(true, Ordering::SeqCst);
    let calibration = Box::new([0u8; 1024]);
    std::hint::black_box(&calibration);
    ALL_THREADS_ACTIVE.store(false, Ordering::SeqCst);
    assert!(
        ALL_THREADS_ALLOCS.load(Ordering::SeqCst) > 0,
        "allocator calibration"
    );
    drop(calibration);
    let mut input = [0xa5; 32]; // fixed until every request completes / driver drops
    let output: [u8; 32] = std::array::from_fn(|i| i as u8);
    let reply: [u8; 32] = std::array::from_fn(|i| 255 - i as u8);
    let (mut driver, client, server) = turnloop_contract::native_surface::synchronous_duplex_pair();
    let mut out = Completions::with_capacity(1);
    let mut verified = 0;
    for round in 0..9 {
        input.fill(0xa5);
        ALL_THREADS_ALLOCS.store(0, Ordering::SeqCst);
        ALL_THREADS_ACTIVE.store(round != 0, Ordering::SeqCst);
        let reads: [_; 32] = std::array::from_fn(|i| {
            // SAFETY: disjoint one-byte buffers stay exclusive through completion.
            let buf = unsafe { IoBufMut::from_raw_parts(input.as_mut_ptr().add(i), 1) };
            driver
                .read(client, ReadBuf::Provided(buf), Token(i as u64))
                .expect("queued duplex read")
        });
        // No peer writes exist yet. Every client read is deliberately idle.
        driver
            .turn(Timeout::Now, &mut out)
            .expect("start idle reader");
        assert!(out.is_empty());
        let writes: [_; 32] = std::array::from_fn(|i| {
            // SAFETY: immutable fixed output remains live through completion/drop.
            let buf = unsafe { IoBuf::from_raw_parts(output.as_ptr().add(i), 1) };
            driver
                .write(client, WriteBuf::Provided(buf), Token(100 + i as u64))
                .expect("queued duplex write")
        });
        driver
            .read(server, ReadBuf::Pooled, Token(200))
            .expect("server receives writes");
        let (mut wrote, mut received) = (0, 0);
        let deadline = driver.now() + Duration::from_secs(5);
        while wrote < 32 || received < 32 {
            assert!(driver.now() < deadline, "idle read blocked duplex writes");
            driver
                .turn(Timeout::Until(deadline), &mut out)
                .expect("independent write progress");
            for c in out.drain() {
                match c.result {
                    OpResult::Wrote(1) => {
                        assert_eq!(
                            (c.handle, c.op, c.token, c.terminal),
                            (
                                Some(client),
                                Some(writes[wrote]),
                                Token(100 + wrote as u64),
                                true
                            )
                        );
                        wrote += 1;
                    }
                    OpResult::Read {
                        n,
                        lease: Some(bytes),
                    } => {
                        assert_eq!(c.handle, Some(server));
                        assert!(n > 0 && received + n <= 32);
                        assert_eq!(bytes.as_slice(), &output[received..received + n]);
                        received += n;
                        if received < 32 {
                            driver
                                .read(server, ReadBuf::Pooled, Token(200))
                                .expect("more server bytes");
                        }
                    }
                    other => panic!("read completed before any reply: {other:?}"),
                }
            }
        }
        // SAFETY: fixed immutable reply remains live until its completion.
        let buf = unsafe { IoBuf::from_raw_parts(reply.as_ptr(), reply.len()) };
        let reply_op = driver
            .write(server, WriteBuf::Provided(buf), Token(201))
            .expect("reply after writes");
        let (mut read, mut replied) = (0, false);
        while read < 32 || !replied {
            assert!(driver.now() < deadline);
            driver
                .turn(Timeout::Until(deadline), &mut out)
                .expect("FIFO replies");
            for c in out.drain() {
                match c.result {
                    OpResult::Read { n: 1, lease: None } => {
                        assert_eq!(
                            (c.handle, c.op, c.token, c.terminal),
                            (Some(client), Some(reads[read]), Token(read as u64), true)
                        );
                        read += 1;
                    }
                    OpResult::Wrote(32) => {
                        assert!(!replied);
                        assert_eq!((c.handle, c.op), (Some(server), Some(reply_op)));
                        replied = true;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        ALL_THREADS_ACTIVE.store(false, Ordering::SeqCst);
        assert_eq!(
            ALL_THREADS_ALLOCS.load(Ordering::SeqCst),
            0,
            "both synchronous workers allocate nothing"
        );
        assert_eq!(input, reply);
        if round != 0 {
            verified += read + wrote;
        }
    }
    assert_eq!(verified, 512);
}
