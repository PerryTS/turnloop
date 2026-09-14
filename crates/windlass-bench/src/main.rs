#![deny(unsafe_op_in_unsafe_fn)]
mod counter;
use counter::Counter;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
use windlass::timer::TimerQueue;

fn report(name: &str, count: usize, before: u64, counter: &Counter) {
    let delta = counter
        .read()
        .expect("read instruction counter")
        .checked_sub(before)
        .expect("monotonic counter");
    assert!(count > 0 && delta > 0, "benchmark must actually execute");
    println!(
        "{{\"name\":\"{name}\",\"operations\":{count},\"total\":{delta},\"per_operation\":{:.2},\"unit\":\"{}\"}}",
        delta as f64 / count as f64,
        counter.unit()
    );
}
fn timers(counter: &Counter) {
    for n in [10, 1000, 100_000] {
        let batches = 100_000 / n;
        let mut queues: Vec<_> = (0..batches).map(|_| TimerQueue::new(n)).collect();
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let base = windlass::Instant::now();
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let base = windlass::Instant::from_duration(Duration::ZERO);
        let times: Vec<_> = (0..n)
            .map(|i| base + Duration::from_nanos(((i * 691) % n) as u64))
            .collect();
        let before = counter.read().expect("counter");
        for q in &mut queues {
            for (i, &at) in times.iter().enumerate() {
                q.insert((1 << 32) | i as u64, at);
            }
        }
        report(&format!("timer_insert_{n}"), n * batches, before, counter);
        assert!(queues.iter().all(|q| q.len() == n));
        let before = counter.read().expect("counter");
        let mut cancelled = 0;
        for q in &mut queues {
            for i in 0..n {
                assert!(q.cancel((1 << 32) | ((i * 691) % n) as u64));
                cancelled += 1;
            }
        }
        report(&format!("timer_cancel_{n}"), cancelled, before, counter);
        assert_eq!(cancelled, n * batches);
        assert!(queues.iter().all(TimerQueue::is_empty));
        for q in &mut queues {
            for (i, &at) in times.iter().enumerate() {
                q.insert((1 << 32) | i as u64, at);
            }
        }
        let before = counter.read().expect("counter");
        let mut expired = 0;
        for q in &mut queues {
            while let Some(e) = q.pop_expired(base + Duration::from_secs(1)) {
                black_box(e);
                expired += 1;
            }
        }
        report(&format!("timer_expire_{n}"), expired, before, counter);
        assert_eq!(expired, n * batches);
    }
}
#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
))]
fn baselines(counter: &Counter) {
    use windlass::*;
    const N: usize = 10_000;
    let mut l = Loop::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    for _ in 0..100 {
        l.turn(Timeout::Now, &mut out).expect("warmup");
    }
    let before = counter.read().expect("counter");
    let mut control = 0u64;
    for i in 0..N {
        control = black_box(control.wrapping_add(black_box(i as u64))).rotate_left(7);
    }
    report("control", N, before, counter);
    black_box(control);
    let before = counter.read().expect("counter");
    let mut waits = 0;
    for _ in 0..N {
        waits += l.turn(Timeout::Now, &mut out).expect("turn").os_waits;
    }
    report("idle_turn", N, before, counter);
    assert_eq!(waits as usize, N);
    let notifier = l.notifier();
    let calls = notifier.wake_syscalls();
    let before = counter.read().expect("counter");
    for _ in 0..N {
        notifier.notify().expect("notify");
        l.turn(Timeout::Now, &mut out).expect("turn");
    }
    report("notify_turn", N, before, counter);
    assert_eq!(notifier.wake_syscalls(), calls);
    let before = counter.read().expect("counter");
    let mut cancelled = 0;
    for _ in 0..N {
        let h = l
            .timer(Instant::now() + Duration::from_secs(60), None, Token(1))
            .expect("timer");
        assert!(l.cancel(l.timer_op(h).expect("op")));
        l.close(h, Token(2)).expect("close timer");
        l.turn(Timeout::Now, &mut out)
            .expect("deliver cancellation");
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].result, OpResult::Cancelled));
        cancelled += 1;
    }
    report(
        "timer_start_cancel_deliver_close",
        cancelled,
        before,
        counter,
    );
    assert_eq!(cancelled, N);
    let mut timer_instructions = 0u64;
    let mut timer_count = 0;
    let at = Instant::now() + Duration::from_secs(60);
    for _ in 0..20 {
        let mut handles = [None; 512];
        let before = counter.read().expect("counter");
        for slot in &mut handles {
            let h = l.timer(at, None, Token(1)).expect("timer");
            assert!(l.cancel(l.timer_op(h).expect("op")));
            *slot = Some(h);
        }
        timer_instructions += counter.read().expect("counter") - before;
        timer_count += handles.len();
        for h in handles {
            l.close(h.expect("timer"), Token(2)).expect("close");
        }
        let mut delivered = 0;
        while delivered < 1024 {
            l.turn(Timeout::Now, &mut out).expect("deliver");
            for c in out.drain() {
                assert!(matches!(c.result, OpResult::Cancelled | OpResult::Closed));
                delivered += 1;
            }
        }
        assert_eq!(delivered, 1024);
    }
    assert_eq!(timer_count, 10240);
    assert!(timer_instructions > 0);
    println!(
        "{{\"name\":\"timer_start_cancel\",\"operations\":{timer_count},\"total\":{timer_instructions},\"per_operation\":{:.2},\"unit\":\"{}\"}}",
        timer_instructions as f64 / timer_count as f64,
        counter.unit()
    );
    let (_, a, b) = windlass_contract::pair(&mut l);
    let input = [0x71; 4096];
    let mut output = [0u8; 4096];
    for batch in [64, 4096] {
        let before = counter.read().expect("counter");
        let mut bytes = 0;
        for _ in 0..1000 {
            // SAFETY: input is fixed immutable storage, alive through completion.
            let w = unsafe { IoBuf::from_raw_parts(input.as_ptr(), batch) };
            // SAFETY: output stays unmoved and untouched until read completion.
            let r = unsafe { IoBufMut::from_raw_parts(output.as_mut_ptr(), batch) };
            l.read(b, ReadBuf::Provided(r), Token(1)).expect("read");
            l.write(a, WriteBuf::Provided(w), Token(2)).expect("write");
            let until = Instant::now() + Duration::from_secs(2);
            let mut read = 0;
            let mut wrote = false;
            while read < batch || !wrote {
                assert!(Instant::now() < until);
                l.turn(Timeout::Until(until), &mut out).expect("turn");
                for c in out.drain() {
                    match c.result {
                        OpResult::Read { n, .. } => {
                            assert!(n > 0);
                            read += n;
                            if read < batch {
                                // SAFETY: the previous read completed; this remaining region
                                // is exclusive until its own next completion.
                                let r = unsafe {
                                    IoBufMut::from_raw_parts(
                                        output[read..].as_mut_ptr(),
                                        batch - read,
                                    )
                                };
                                l.read(b, ReadBuf::Provided(r), Token(1)).expect("continue");
                            }
                        }
                        OpResult::Wrote(n) => {
                            assert_eq!(n, batch);
                            wrote = true;
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                }
            }
            assert_eq!(&output[..batch], &input[..batch]);
            bytes += read;
        }
        report(&format!("tcp_batch_{batch}"), 1000, before, counter);
        assert_eq!(bytes, batch * 1000);
    }
    let listener = l
        .tcp_listen("127.0.0.1:0".parse().expect("addr"), &ListenOpts::default())
        .expect("listen");
    let addr = l.local_addr(listener).expect("addr");
    // Prepare connections before each measured accept batch; client connect and
    // teardown are excluded. Multiple snapshots are amortized over 32 accepts.
    let mut instructions = 0;
    let mut accepts = 0;
    for _ in 0..20 {
        let clients: Vec<_> = (0..32)
            .map(|_| std::net::TcpStream::connect(addr).expect("client"))
            .collect();
        for i in 0..32 {
            l.accept(listener, Token(i)).expect("submit accept");
        }
        let mut handles = [None; 32];
        let mut received = 0;
        let before = counter.read().expect("counter");
        let until = Instant::now() + Duration::from_secs(2);
        while received < 32 {
            assert!(Instant::now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                if let OpResult::Accepted { conn, .. } = c.result {
                    handles[received] = Some(conn);
                    received += 1;
                } else {
                    panic!("unexpected accept result");
                }
            }
        }
        instructions += counter.read().expect("counter") - before;
        accepts += received;
        for h in handles {
            l.close(h.expect("accepted"), Token(33)).expect("close");
        }
        l.turn(Timeout::Now, &mut out).expect("release");
        assert_eq!(out.len(), 32);
        drop(clients);
    }
    assert_eq!(accepts, 640);
    assert!(instructions > 0);
    println!(
        "{{\"name\":\"accept\",\"operations\":{accepts},\"total\":{instructions},\"per_operation\":{:.2},\"unit\":\"{}\"}}",
        instructions as f64 / accepts as f64,
        counter.unit()
    );
}
fn main() {
    let counter = if std::env::args().any(|a| a == "--portable") {
        Counter::Portable(Instant::now())
    } else {
        Counter::new()
    };
    if std::env::args().any(|a| a == "--timers") {
        timers(&counter);
    } else {
        #[cfg(any(
            target_vendor = "apple",
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd"
        ))]
        baselines(&counter);
        #[cfg(not(any(
            target_vendor = "apple",
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd"
        )))]
        timers(&counter);
    }
}
