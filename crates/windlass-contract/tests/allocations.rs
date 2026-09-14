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
use windlass::*;
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
#[test]
fn steady_read_write_timer_and_accept_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let (_, a, b) = windlass_contract::pair(&mut l);
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
