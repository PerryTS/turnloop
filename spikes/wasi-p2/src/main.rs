#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};
use wasi::{clocks::monotonic_clock as clock, sockets::network::ErrorCode};
use turnloop_wasi_p2_spike::{BYTES, CAPACITY, Completion, Driver, ResultKind, Timeout};
struct CountAllocator;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
// SAFETY: allocation and deallocation are delegated without changing pointers or layouts.
unsafe impl GlobalAlloc for CountAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: caller returns a System allocation with its original layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountAllocator = CountAllocator;
fn turn(d: &mut Driver, out: &mut Vec<Completion>) -> Result<(), ErrorCode> {
    let before = d.waits;
    d.turn(Timeout::After(100_000_000), out)?;
    assert!(d.waits - before <= 1);
    Ok(())
}
fn timers() -> Result<(), ErrorCode> {
    let mut d = Driver::new();
    let mut out = Vec::with_capacity(CAPACITY * 2);
    for ns in [100_000, 500_000, 1_000_000, 5_000_000] {
        let mut samples = [0; 32];
        for (i, late) in samples.iter_mut().enumerate() {
            let at = clock::now() + ns;
            d.timer(at, i as u64)?;
            let limit = clock::now() + 1_000_000_000;
            loop {
                turn(&mut d, &mut out)?;
                if !out.is_empty() {
                    break;
                }
                assert!(clock::now() < limit);
            }
            assert_eq!(
                out,
                [Completion {
                    token: i as u64,
                    result: ResultKind::Timer
                }]
            );
            *late = clock::now() - at;
            assert!(*late < 100_000_000, "timer exceeded scheduler tolerance");
        }
        samples.sort();
        println!(
            "timer requested_ns={ns} samples=32 lateness_ns min={} median={} max={}",
            samples[0], samples[16], samples[31]
        );
    }
    let id = d.timer(clock::now() + 10_000_000_000, 999)?;
    assert!(d.cancel(id));
    assert!(!d.cancel(id));
    let waits = d.waits;
    d.turn(Timeout::Forever, &mut out)?;
    assert_eq!(d.waits, waits);
    assert_eq!(
        out,
        [Completion {
            token: 999,
            result: ResultKind::Cancelled
        }]
    );
    d.turn(Timeout::Now, &mut out)?;
    assert!(out.is_empty());
    assert!(d.waits > 0);
    println!("timers PASS turns={} waits={}", d.turns, d.waits);
    Ok(())
}
fn echo() -> Result<(), ErrorCode> {
    const N: usize = 64;
    let mut d = Driver::new();
    let mut out = Vec::with_capacity(CAPACITY * 2);
    let listener = d.listen(1)?;
    let limit = clock::now() + 30_000_000_000;
    let port = loop {
        turn(&mut d, &mut out)?;
        if let Some(Completion {
            result: ResultKind::Listening(p),
            ..
        }) = out.first()
        {
            break *p;
        }
        assert!(clock::now() < limit);
    };
    let mut clients = [0; N];
    let mut servers = [0; N];
    for (i, c) in clients.iter_mut().enumerate() {
        *c = d.connect(port, 100 + i as u64)?;
    }
    d.accept(listener, 2)?;
    let mut connected = [false; N];
    let mut accepted = 0;
    while accepted != N || connected.iter().any(|x| !x) {
        turn(&mut d, &mut out)?;
        for c in &out {
            match c.result {
                ResultKind::Connected => {
                    let i = (c.token - 100) as usize;
                    assert!(!connected[i]);
                    connected[i] = true;
                }
                ResultKind::Accepted(id) => {
                    servers[accepted] = id;
                    accepted += 1;
                    if accepted < N {
                        d.accept(listener, 2)?;
                    }
                }
                other => panic!("unexpected connection completion {other:?}"),
            }
        }
        assert!(clock::now() < limit, "connect/accept deadline");
    }
    // All 64 clients and all 64 accepted peers are live before any echo starts.
    let payload: [u8; BYTES] = std::array::from_fn(|i| ((i * 73 + 19) % 256) as u8);
    for i in 0..N {
        d.write(clients[i], &payload, 1000 + i as u64)?;
        d.read(servers[i], 2000 + i as u64)?;
    }
    let mut sent = [0; N];
    let mut received = [0; N];
    let mut server_bytes = [[0u8; BYTES]; N];
    let mut echoed = [0; N];
    let mut verified = [0; N];
    let mut completions = 0;
    while verified.iter().any(|n| *n < BYTES) {
        turn(&mut d, &mut out)?;
        completions += out.len();
        for c in &out {
            let i = (c.token % 1000) as usize;
            match (c.token / 1000, c.result) {
                (1, ResultKind::Wrote(n)) => {
                    sent[i] += n;
                    if sent[i] < BYTES {
                        d.write(clients[i], &payload[sent[i]..], c.token)?;
                    } else {
                        d.read(clients[i], 4000 + i as u64)?;
                    }
                }
                (2, ResultKind::Read(n)) => {
                    assert_eq!(
                        &d.data(servers[i]).expect("live server")[..n],
                        &payload[received[i]..received[i] + n]
                    );
                    server_bytes[i][received[i]..received[i] + n]
                        .copy_from_slice(&d.data(servers[i]).expect("live server")[..n]);
                    received[i] += n;
                    // Buffer the actual received bytes, then echo exactly those bytes.
                    if received[i] < BYTES {
                        d.read(servers[i], c.token)?;
                    } else {
                        d.write(servers[i], &server_bytes[i], 3000 + i as u64)?;
                    }
                }
                (3, ResultKind::Wrote(n)) => {
                    echoed[i] += n;
                    if echoed[i] < BYTES {
                        d.write(servers[i], &server_bytes[i][echoed[i]..], c.token)?;
                    }
                }
                (4, ResultKind::Read(n)) => {
                    assert_eq!(
                        &d.data(clients[i]).expect("live client")[..n],
                        &payload[verified[i]..verified[i] + n]
                    );
                    verified[i] += n;
                    if verified[i] < BYTES {
                        d.read(clients[i], c.token)?;
                    }
                }
                other => panic!("unexpected echo completion {other:?}"),
            }
        }
        assert!(clock::now() < limit, "echo deadline");
    }
    assert_eq!(sent, [BYTES; N]);
    assert_eq!(received, [BYTES; N]);
    assert_eq!(echoed, [BYTES; N]);
    assert!(completions >= 4 * N);
    assert!(d.io_attempts > 0);
    // Pending reads are cancelled once, then Closed; pollables must drop before parents.
    for (i, id) in clients.into_iter().chain(servers).enumerate() {
        d.read(id, 5000 + i as u64)?;
        assert!(d.close(id, 6000 + i as u64));
        assert!(!d.close(id, 7000));
    }
    let waits = d.waits;
    d.turn(Timeout::Forever, &mut out)?;
    assert_eq!(d.waits, waits);
    assert_eq!(out.len(), 4 * N);
    for (i, pair) in out.as_chunks::<2>().0.iter().enumerate() {
        assert_eq!(
            *pair,
            [
                Completion {
                    token: 5000 + i as u64,
                    result: ResultKind::Cancelled
                },
                Completion {
                    token: 6000 + i as u64,
                    result: ResultKind::Closed
                }
            ]
        );
    }
    d.turn(Timeout::Now, &mut out)?;
    assert!(out.is_empty());
    d.read(listener, 8000)?;
    d.turn(Timeout::Now, &mut out)?;
    assert_eq!(
        out,
        [Completion {
            token: 8000,
            result: ResultKind::Error(ErrorCode::InvalidState)
        }]
    );
    d.turn(Timeout::Now, &mut out)?;
    assert!(out.is_empty());
    assert!(d.close(listener, 9));
    d.turn(Timeout::Now, &mut out)?;
    assert_eq!(
        out,
        [Completion {
            token: 9,
            result: ResultKind::Closed
        }]
    );
    d.turn(Timeout::Now, &mut out)?;
    assert!(out.is_empty());
    println!(
        "echo PASS concurrent={N} bytes_verified={} completions={completions} turns={} waits={} close_cancelled={} closed={}",
        N * BYTES,
        d.turns,
        d.waits,
        2 * N,
        2 * N + 1
    );
    Ok(())
}
fn bench(mode: &str, n: usize) -> Result<(), ErrorCode> {
    let mut d = Driver::new();
    let mut out = Vec::with_capacity(CAPACITY * 2);
    let mut sum = 0u64;
    d.turn(Timeout::Now, &mut out)?;
    let before = ALLOCS.load(Ordering::Relaxed);
    for i in 0..n {
        match mode {
            "idle" => d.turn(Timeout::Now, &mut out)?,
            "poll" | "allocation-gate" => d.turn(Timeout::After(0), &mut out)?,
            "timer-cancel" => {
                let id = d.timer(clock::now() + 1_000_000_000, i as u64)?;
                assert!(d.cancel(id));
                d.turn(Timeout::Now, &mut out)?;
                assert_eq!(out.len(), 1);
            }
            "control" => {}
            _ => panic!("unknown benchmark"),
        }
        sum = std::hint::black_box(sum.wrapping_add(i as u64));
    }
    let allocs = ALLOCS.load(Ordering::Relaxed) - before;
    assert!(n > 0);
    assert_eq!(sum, (n as u64) * (n as u64 - 1) / 2);
    if mode != "control" {
        assert_eq!(d.turns, n as u64 + 1);
    }
    println!(
        "bench mode={mode} iterations={n} turns={} waits={} allocations={allocs} control={sum}",
        d.turns, d.waits
    );
    if mode == "allocation-gate" {
        assert_eq!(allocs, 0, "DESIGN section 10 zero-allocation gate");
    }
    Ok(())
}
fn main() -> Result<(), ErrorCode> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() > 1 {
        return bench(
            &args[1],
            args.get(2).and_then(|x| x.parse().ok()).unwrap_or(1000),
        );
    }
    timers()?;
    echo()
}
