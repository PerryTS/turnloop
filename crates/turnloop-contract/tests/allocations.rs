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
#[cfg(not(target_os = "wasi"))]
use std::cell::Cell;
#[cfg(not(target_os = "wasi"))]
thread_local! { static ACTIVE: Cell<bool> = const { Cell::new(false) }; static ALLOCS: Cell<usize> = const { Cell::new(0) }; }
#[cfg(not(target_os = "wasi"))]
fn record() {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
    }
}
// WASI components have one agent. These counters also work before p3 std has
// initialized its thread-local area, when the harness allocates argument strings.
#[cfg(target_os = "wasi")]
mod single_agent_counter {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    pub struct Active(AtomicBool);
    pub struct Allocations(AtomicUsize);
    pub static ACTIVE: Active = Active(AtomicBool::new(false));
    pub static ALLOCS: Allocations = Allocations(AtomicUsize::new(0));
    impl Active {
        pub fn with<T>(&self, f: impl FnOnce(&Self) -> T) -> T {
            f(self)
        }
        pub fn set(&self, value: bool) {
            self.0.store(value, Ordering::Relaxed);
        }
    }
    impl Allocations {
        pub fn with<T>(&self, f: impl FnOnce(&Self) -> T) -> T {
            f(self)
        }
        pub fn set(&self, value: usize) {
            self.0.store(value, Ordering::Relaxed);
        }
        pub fn get(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }
    pub fn record() {
        if ACTIVE.0.load(Ordering::Relaxed) {
            ALLOCS.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}
#[cfg(target_os = "wasi")]
use single_agent_counter::{ACTIVE, ALLOCS, record};
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
    let to = l.local_addr(b).expect("destination");
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
                        n, lease: Some(b), ..
                    } => {
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
    let mut sockets = Vec::new();
    for l in &mut loops {
        let mut group = Vec::new();
        for _ in 0..N {
            let h = l
                .udp_bind("[::1]:0".parse().expect("IPv6"), &UdpOpts::default())
                .expect("UDP");
            group.push((h, l.local_addr(h).expect("local address")));
        }
        sockets.push(group);
    }
    let mut out = Completions::with_capacity(1);
    let mut received = 0;
    let mut cancelled = 0;
    let mut allocations = 0;
    for round in 0..=ROUNDS {
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
                l.recv(h, buf, Token(i as u64)).expect("receive");
                // SAFETY: immutable retained payload lives until both loops drop.
                let input = unsafe { IoBuf::from_raw_parts(payload.as_ptr(), LENGTHS[i]) };
                l.send_to(h, WriteBuf::Provided(input), to, Token((N + i) as u64))
                    .expect("send");
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
                    assert!(!seen[j][index], "duplicate UDP result");
                    seen[j][index] = true;
                    completed += 1;
                    match c.result {
                        OpResult::RecvFrom { n, from, lease } => {
                            assert_eq!(n, LENGTHS[index]);
                            assert_eq!(from, sockets[j][index].1);
                            let bytes = lease
                                .as_ref()
                                .map_or(&outputs[index][..n], BufLease::as_slice);
                            assert_eq!(bytes, &payload[..n]);
                            received += usize::from(round != 0);
                        }
                        OpResult::Wrote(n) => assert_eq!(n, LENGTHS[index - N]),
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
