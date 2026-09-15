#![deny(unsafe_op_in_unsafe_fn)]
//! Shared contract scenarios. Platform lanes instantiate these with their Backend;
//! assertions and test logic are identical on all capable backends.
use std::{
    net::{Ipv4Addr, SocketAddr},
    thread,
    time::{Duration, Instant},
};
use turnloop::{backend::Backend, *};
fn localhost() -> SocketAddr {
    (Ipv4Addr::LOCALHOST, 0).into()
}
pub fn bounded_turn<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    let start = Instant::now();
    let info = l
        .turn(Timeout::After(Duration::from_millis(12)), &mut out)
        .expect("turn");
    assert_eq!(info.os_waits, 1, "the wait must actually run");
    assert_eq!(info.discovery_polls, 0);
    assert!(start.elapsed() >= Duration::from_millis(10));
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(info.completions, 0);
}
pub fn notify_parked<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let n = l.notifier();
    let other = n.clone();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !other.is_parked() {
            assert!(Instant::now() < deadline, "loop never parked");
            thread::yield_now();
        }
        other.notify().expect("wake");
    });
    let mut out = Completions::default();
    let start = Instant::now();
    let info = l
        .turn(Timeout::After(Duration::from_secs(3)), &mut out)
        .expect("turn");
    worker.join().expect("producer");
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(info.os_waits, 1);
    assert_eq!(info.discovery_polls, 0);
    assert!(n.wake_syscalls() > 0);
}
pub fn notify_running<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let n = l.notifier();
    let before = n.wake_syscalls();
    for _ in 0..1000 {
        n.notify().expect("notify");
    }
    let start = Instant::now();
    l.turn(
        Timeout::After(Duration::from_secs(2)),
        &mut Completions::default(),
    )
    .expect("turn");
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(n.wake_syscalls(), before);
}
/// Establish a connection with both endpoints driven by this loop.
pub fn pair<B: Backend>(l: &mut Driver<B>) -> (Handle, Handle, Handle) {
    let server = l
        .tcp_listen(localhost(), &ListenOpts::default())
        .expect("listen");
    l.accept(server, Token(1)).expect("accept");
    let client = l
        .tcp_connect(
            l.local_addr(server).expect("addr"),
            &TcpOpts { nodelay: true },
            Token(2),
        )
        .expect("connect");
    let mut conn = None;
    let mut connected = false;
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(3);
    while conn.is_none() || !connected {
        assert!(l.now() < until, "connect/accept timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Accepted { conn: h, .. } => conn = Some(h),
                OpResult::Connected => connected = true,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    (server, client, conn.expect("accepted"))
}
pub fn tcp_echo<B: Backend>(connections: usize) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut pairs = Vec::new();
    for _ in 0..connections {
        let (_, a, b) = pair(&mut l);
        pairs.push((a, b));
    }
    let data = b"turnloop echo: every byte matters";
    for (i, &(a, b)) in pairs.iter().enumerate() {
        l.read(b, ReadBuf::Pooled, Token(100 + i as u64))
            .expect("read");
        l.write(a, WriteBuf::Owned(data.to_vec()), Token(1000 + i as u64))
            .expect("write");
    }
    let until = l.now() + Duration::from_secs(5);
    let mut received = 0;
    let mut wrote = 0;
    let mut echoed = 0;
    let mut accumulated = vec![Vec::new(); connections];
    let mut returns = vec![Vec::new(); connections];
    let mut out = Completions::default();
    while echoed != connections || wrote != 2 * connections {
        assert!(
            l.now() < until,
            "echo timed out: received={received} echoed={echoed} wrote={wrote}"
        );
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease: Some(b) } => {
                    assert!(n > 0);
                    assert_eq!(n, b.as_slice().len());
                    if c.token.0 < 200 {
                        let i = c.token.0 as usize - 100;
                        accumulated[i].extend_from_slice(b.as_slice());
                        if accumulated[i].len() < data.len() {
                            l.read(pairs[i].1, ReadBuf::Pooled, c.token)
                                .expect("continue read");
                        } else {
                            assert_eq!(accumulated[i], data);
                            received += 1;
                            l.read(pairs[i].0, ReadBuf::Pooled, Token(200 + i as u64))
                                .expect("echo read");
                            l.write(
                                pairs[i].1,
                                WriteBuf::Owned(accumulated[i].clone()),
                                Token(2000 + i as u64),
                            )
                            .expect("echo write");
                        }
                    } else {
                        let i = c.token.0 as usize - 200;
                        returns[i].extend_from_slice(b.as_slice());
                        if returns[i].len() < data.len() {
                            l.read(pairs[i].0, ReadBuf::Pooled, c.token)
                                .expect("continue echo");
                        } else {
                            assert_eq!(returns[i], data);
                            echoed += 1;
                        }
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, data.len());
                    wrote += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(received, connections);
    assert_eq!(echoed, connections);
}
#[cfg(all(
    test,
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    )
))]
mod native {
    use super::*;
    type B = turnloop::backend::Platform;
    #[test]
    fn terminal_delivery_liveness() {
        ready_timer_liveness::<B>();
    }
    #[test]
    fn timer_backlog_io_and_post_progress() {
        io_and_posts_progress_with_repeating_timers::<B>();
    }
    #[test]
    fn queued_core_work_makes_no_native_calls() {
        queued_core_work::<B>();
    }
    #[test]
    fn queued_post_with_idle_native_io_never_waits() {
        queued_post_idle_io::<B>();
    }
    #[test]
    fn queued_terminals_with_idle_native_io_never_wait() {
        queued_terminals_idle_io::<B>();
    }
    #[test]
    fn sustained_posts_preserve_io_progress_without_waits() {
        sustained_posts_idle_io::<B>();
    }
    #[test]
    fn retained_pool_lease() {
        pooled_lease_backpressure::<B>();
    }
    #[test]
    fn external_waiter() {
        integration_fd::<B>();
    }
    #[test]
    fn vectored_stream_shutdown() {
        writev_and_shutdown::<B>();
    }
    #[test]
    fn bounded_capacity_and_stale_ids() {
        capacity_and_stale_ids::<B>();
    }
    #[test]
    fn cancellation_close() {
        cancel_close_ordering::<B>();
    }
    #[test]
    fn connection_error() {
        refused_connect_once::<B>();
    }
    #[test]
    fn liveness() {
        ref_unref::<B>();
    }
    #[test]
    fn idle_socket_timers_do_not_spin() {
        no_spin::<B>();
    }
    #[test]
    fn quiet_deadlines_account_identically_idle_and_registered() {
        quiet_deadline_accounting::<B>();
    }
    #[test]
    fn timer_bounds() {
        timer_precision::<B>();
    }
    #[test]
    fn udp_echo() {
        udp_round_trip::<B>();
    }
    #[test]
    fn many_loops_post() {
        cross_post::<B>(4, 1000);
    }
    #[test]
    fn transfer_inflight() {
        detach_inflight::<B>();
    }
    #[test]
    fn shared_pool() {
        pool_and_dns::<B>();
    }
    #[test]
    fn reuse_port_distribution() {
        reuse_port::<B>();
    }
    #[test]
    fn accept_handoff_distribution() {
        handoff_distribution::<B>();
    }

    #[test]
    fn bounded() {
        bounded_turn::<B>();
    }
    #[test]
    fn parked_notify() {
        notify_parked::<B>();
    }
    #[test]
    fn running_notify() {
        notify_running::<B>();
    }
    #[test]
    fn echo_one() {
        tcp_echo::<B>(1);
    }
    #[test]
    fn echo_64() {
        tcp_echo::<B>(64);
    }
}

pub fn cancel_close_ordering<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (_, a, b) = pair(&mut l);
    let mut memory = [0xa5; 32];
    // SAFETY: memory stays unmoved and inaccessible until Cancelled is delivered.
    let provided = unsafe { IoBufMut::from_raw_parts(memory.as_mut_ptr(), memory.len()) };
    let first = l
        .read(b, ReadBuf::Provided(provided), Token(10))
        .expect("read");
    let second = l.read_start(b, Token(11)).expect("read_start");
    l.turn(Timeout::Now, &mut Completions::default())
        .expect("arm reads");
    assert!(l.cancel(first));
    assert!(!l.cancel(first));
    l.close(b, Token(12)).expect("close");
    assert!(
        l.read(b, ReadBuf::Pooled, Token(13)).is_err(),
        "closing handle rejects ops"
    );
    let mut out = Completions::with_capacity(1);
    let mut terminal = Vec::new();
    let mut closed = 0;
    let until = l.now() + Duration::from_secs(2);
    while closed == 0 {
        assert!(l.now() < until);
        let info = l.turn(Timeout::Until(until), &mut out).expect("turn");
        assert!(info.os_waits + info.discovery_polls <= 1);
        for c in out.drain() {
            match c.result {
                OpResult::Cancelled => {
                    assert!(c.terminal);
                    terminal.push(c.op.expect("op"));
                }
                OpResult::Closed => {
                    assert_eq!(terminal.len(), 2);
                    assert!(terminal.contains(&first));
                    assert!(terminal.contains(&second));
                    closed += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(memory, [0xa5; 32]);
    assert_eq!(closed, 1);
    assert!(!l.cancel(first));
    assert!(!l.cancel(second));
    l.write(a, WriteBuf::Owned(vec![1; 32]), Token(14))
        .expect("write to closed peer");
    let until = l.now() + Duration::from_secs(1);
    let mut seen = 0;
    while seen == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            if c.token == Token(14) {
                assert!(c.terminal);
                seen += 1;
            } else {
                panic!("duplicate terminal: {c:?}");
            }
        }
    }
    for _ in 0..5 {
        l.turn(Timeout::Now, &mut out).expect("drain");
        assert!(out.is_empty());
    }
    assert_eq!(seen, 1);
}
pub fn refused_connect_once<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let h = l
        .tcp_listen(localhost(), &ListenOpts::default())
        .expect("reserve");
    let addr = l.local_addr(h).expect("addr");
    l.close(h, Token(1)).expect("close");
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("release port");
    assert!(matches!(out[0].result, OpResult::Closed));
    // Local port is now unbound; an asynchronous refused connection must terminate once.
    let conn = l
        .tcp_connect(addr, &TcpOpts::default(), Token(7))
        .expect("accepted connect op");
    let until = l.now() + Duration::from_secs(2);
    let mut count = 0;
    while count == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert_eq!(c.token, Token(7));
            assert!(
                matches!(
                    c.result,
                    OpResult::Err(Error {
                        kind: ErrorKind::ConnectionRefused,
                        ..
                    })
                ),
                "unexpected refused-connect result: {:?}",
                c.result
            );
            assert!(c.terminal);
            count += 1;
        }
    }
    for _ in 0..3 {
        l.turn(Timeout::Now, &mut out).expect("drain");
        assert!(out.is_empty());
    }
    l.close(conn, Token(8)).expect("close");
    assert_eq!(count, 1);
}
pub fn ref_unref<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    assert!(!l.alive());
    let h = l.timer(l.now(), None, Token(1)).expect("timer");
    assert!(l.alive());
    l.set_ref(h, false).expect("unref");
    assert!(!l.alive());
    l.set_ref(h, true).expect("ref");
    assert!(l.alive());
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("turn");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Timer));
    assert!(!l.alive());
    let s = l
        .tcp_listen(localhost(), &ListenOpts::default())
        .expect("listener");
    let op = l.accept_start(s, Token(2)).expect("accept");
    assert!(l.alive());
    l.set_ref(s, false).expect("unref listener and its ops");
    assert!(!l.alive());
    l.set_ref(s, true).expect("ref");
    assert!(l.alive());
    assert!(l.stop(op));
    l.close(s, Token(3)).expect("close");
    l.turn(Timeout::Now, &mut out).expect("turn");
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0].result, OpResult::Stopped));
    assert!(matches!(out[1].result, OpResult::Closed));
    assert!(!l.alive());
}
/// Timer-precision gate for [`timer_precision`], measured against this host.
///
/// Wake precision is as much a property of the host as of the backend. The M1
/// Windows spike measured p50/p95/max lateness of 275.2/285.8/380.9 us for a
/// 250 us deadline on real hardware (`spikes/iocp/WINDOWS_RESULTS.md`), so a fixed
/// 500 us bound left the Windows backend under a factor of two of clear air and a
/// loaded CI VM crossed it with no backend regression at all: 756.2 us and 594.2 us
/// on windows-2025 within one hour, and 2.31 ms against the 2 ms Wasmtime bound on
/// WASI 0.3, whose samples that run spanned 1.07-6.44 ms (issue #30).
///
/// Two independent requirements replace the single fixed median, and every attempt
/// reports the whole distribution of both measurements:
///
/// 1. **Capability.** The backend's second-best expiry of twenty must still meet the
///    platform floor. Load only ever makes a sample later, never earlier, so this is
///    immune to a loaded or mismeasured host -- while a wait floor or a deadline
///    rounded up to milliseconds raises *every* sample and fails it. This is the
///    clause the mutation proof in `docs/lanes/flakes.md` exercises.
/// 2. **Typical case.** The median must stay within the larger of that same floor
///    and twice what this host achieves with its own sleep, measured interleaved
///    with the expiries so a load spike moves the bound as well as its subject.
///
/// The calibration never touches the `Driver`: `std::thread::sleep` waits on the OS
/// primitive directly -- a `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` timer on Windows
/// (the same object the backend arms), `nanosleep` on Unix and a `wasi:clocks`
/// monotonic pollable on WASI -- so a backend that loses precision cannot move it.
/// It is a bound, not a verdict: a host whose own sleep is coarser than its timers
/// (macos-15 runners coalesce `nanosleep` to ~2 ms while the loop resolves 234 us)
/// can only relax the median clause, never the capability clause.
#[cfg(any(not(target_os = "wasi"), not(debug_assertions)))]
mod precision {
    use super::*;
    /// Short enough that any millisecond rounding or wait floor dominates.
    const DELAY: Duration = Duration::from_micros(250);
    /// Samples of each kind per attempt.
    const SAMPLES: usize = 20;
    /// A demonstrably loaded host may retry; a wait floor is systematic and fails
    /// every attempt.
    const ATTEMPTS: usize = 3;
    /// The lateness a quiet host must meet whatever the calibration says. A host
    /// that beats it cannot buy itself a looser bound.
    #[cfg(not(target_os = "wasi"))]
    const FLOOR: Duration = Duration::from_micros(500);
    #[cfg(target_os = "wasi")]
    const FLOOR: Duration = Duration::from_millis(2);

    /// Sorted lateness samples, with the statistics every attempt reports.
    struct Samples(Vec<Duration>);
    impl Samples {
        fn new(mut values: Vec<Duration>) -> Self {
            assert_eq!(values.len(), SAMPLES, "every sample must be recorded");
            values.sort_unstable();
            Self(values)
        }
        /// The capability statistic: the second-best sample, so one lucky expiry
        /// cannot certify a backend, and one stalled one cannot condemn it.
        fn best(&self) -> Duration {
            self.0[1]
        }
        /// The typical-case statistic: a few stalled wakes cannot move it.
        fn median(&self) -> Duration {
            self.0[SAMPLES / 2]
        }
        fn report(&self) -> String {
            format!(
                "min={:?} 2nd={:?} p50={:?} p90={:?} max={:?}",
                self.0[0],
                self.best(),
                self.median(),
                self.0[SAMPLES * 9 / 10],
                self.0[SAMPLES - 1]
            )
        }
    }

    /// One loop expiry: arm a `DELAY` timer, turn until it fires, then release it.
    /// The 100 ms ceiling still rejects a wake that never arrives.
    fn expiry<B: Backend>(l: &mut Driver<B>, out: &mut Completions) -> Duration {
        let at = l.now() + DELAY;
        let h = l.timer(at, None, Token(77)).expect("precision sample");
        let until = at + Duration::from_millis(100);
        let late = loop {
            assert!(l.now() < until, "precision sample missed maximum bound");
            l.turn(Timeout::Until(until), out).expect("sample turn");
            if out
                .iter()
                .any(|c| c.token == Token(77) && matches!(c.result, OpResult::Timer))
            {
                break l.now().duration_since(at);
            }
        };
        l.close(h, Token(78)).expect("close sample");
        l.turn(Timeout::Now, out).expect("release sample");
        late
    }

    /// The same measurement with no loop in it: what this host resolves on its own.
    fn calibration<B: Backend>(l: &Driver<B>) -> Duration {
        let at = l.now() + DELAY;
        thread::sleep(DELAY);
        l.now().duration_since(at)
    }

    /// Require the loop to still reach the platform floor, and to be typical of
    /// what this host can do at all.
    pub(super) fn check<B: Backend>(l: &mut Driver<B>, out: &mut Completions) {
        let mut attempts = Vec::with_capacity(ATTEMPTS);
        for _ in 0..ATTEMPTS {
            let mut expiries = Vec::with_capacity(SAMPLES);
            let mut sleeps = Vec::with_capacity(SAMPLES);
            // Interleaved, so a load spike moves the bound as well as its subject.
            for _ in 0..SAMPLES {
                expiries.push(expiry(l, out));
                sleeps.push(calibration(l));
            }
            let (expiries, sleeps) = (Samples::new(expiries), Samples::new(sleeps));
            let allowance = FLOOR.max(sleeps.median() * 2);
            let capable = expiries.best() <= FLOOR;
            let typical = expiries.median() <= allowance;
            let attempt = format!(
                "loop {} | host sleep {} | floor {FLOOR:?} {} | allowance {allowance:?} {}",
                expiries.report(),
                sleeps.report(),
                if capable { "met" } else { "MISSED" },
                if typical { "met" } else { "MISSED" }
            );
            println!("turnloop timer precision: {attempt}");
            attempts.push(attempt);
            if capable && typical {
                return;
            }
            // A host whose own sleep stays inside half the quiet-host floor is not
            // loaded, so the loop is the only explanation: retrying such an attempt
            // could only hide a real regression.
            if sleeps.median() * 2 <= FLOOR {
                break;
            }
        }
        panic!(
            "timer precision missed its bound in {} attempt(s): {}",
            attempts.len(),
            attempts.join(" ;; ")
        );
    }
}
pub fn timer_precision<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    let mut fired = 0;
    for micros in [100, 500, 2000] {
        let at = l.now() + Duration::from_micros(micros);
        let h = l.timer(at, None, Token(micros)).expect("timer");
        let until = at + Duration::from_millis(100);
        while fired
            < if micros == 100 {
                1
            } else if micros == 500 {
                2
            } else {
                3
            }
        {
            assert!(l.now() < until, "timer missed precision bound");
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                if matches!(c.result, OpResult::Timer) {
                    assert_eq!(c.token, Token(micros));
                    assert!(l.now() >= at, "timer fired early");
                    fired += 1;
                }
            }
        }
        assert!(l.now().duration_since(at) < Duration::from_millis(100));
        l.close(h, Token(0)).expect("close");
        l.turn(Timeout::Now, &mut out).expect("close timer");
    }
    assert_eq!(fired, 3);
    let h = l
        .timer(
            l.now() + Duration::from_secs(1),
            Some(Duration::from_micros(200)),
            Token(44),
        )
        .expect("repeat");
    assert!(l.timer_reset(h, l.now()));
    let op = l.timer_op(h).expect("op");
    let mut repeats = 0;
    let until = l.now() + Duration::from_secs(1);
    while repeats < 3 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert!(!c.terminal);
            assert!(matches!(c.result, OpResult::Timer));
            repeats += 1;
        }
    }
    assert!(l.stop(op));
    l.turn(Timeout::Now, &mut out).expect("stop");
    assert!(matches!(out[0].result, OpResult::Stopped));
    // WASI wake precision is host-dependent (DESIGN §7.4); debug builds exercise
    // every timer semantic above and the independent mandatory no-spin contract,
    // and leave the measured precision gate to release.
    #[cfg(any(not(target_os = "wasi"), not(debug_assertions)))]
    precision::check(&mut l, &mut out);
}
pub fn udp_round_trip<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let a = l
        .udp_bind(localhost(), &UdpOpts::default())
        .expect("bind a");
    let b = l
        .udp_bind(localhost(), &UdpOpts::default())
        .expect("bind b");
    let aa = l.local_addr(a).expect("addr");
    let ba = l.local_addr(b).expect("addr");
    assert_ne!(aa, ba, "default UDP endpoints must be distinct");
    let bytes = b"datagram checked byte for byte";
    l.recv(b, ReadBuf::Pooled, Token(1)).expect("recv");
    l.send_to(a, WriteBuf::Owned(bytes.to_vec()), ba, Token(2))
        .expect("send");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(2);
    let mut received = 0;
    let mut writes = 0;
    while received < 2 || writes < 2 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::RecvFrom {
                    n,
                    from,
                    lease: Some(data),
                } => {
                    assert!(
                        matches!(c.token, Token(1) | Token(3)),
                        "unexpected UDP receive token"
                    );
                    assert_eq!(
                        from,
                        if c.token == Token(1) { aa } else { ba },
                        "unexpected UDP sender"
                    );
                    assert_eq!(n, bytes.len());
                    assert_eq!(data.as_slice(), bytes);
                    received += 1;
                    if c.token == Token(1) {
                        l.recv(a, ReadBuf::Pooled, Token(3)).expect("recv return");
                        l.send_to(b, WriteBuf::Owned(data.as_slice().to_vec()), from, Token(4))
                            .expect("return send");
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, bytes.len());
                    writes += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(received, 2);
    assert_eq!(writes, 2);
}
pub fn cross_post<B: Backend>(threads: usize, per_peer: usize) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut sends = Vec::new();
    let mut workers = Vec::new();
    for id in 0..threads {
        let tx = tx.clone();
        let (send, receive) = std::sync::mpsc::channel::<Vec<Poster>>();
        sends.push(send);
        workers.push(thread::spawn(move || {
            let mut l = Driver::<B>::new(Config {
                post_capacity: threads * per_peer,
                ..Config::default()
            })
            .expect("worker loop");
            let owner = thread::current().id();
            tx.send((id, l.poster())).expect("register poster");
            let posters = receive.recv().expect("peers");
            for (target, p) in posters.iter().enumerate() {
                for seq in 0..per_peer {
                    p.post(
                        Token(target as u64),
                        Payload::U64((id * per_peer + seq) as u64),
                    )
                    .expect("post");
                }
            }
            let mut seen = vec![false; threads * per_peer];
            let mut count = 0;
            let until = l.now() + Duration::from_secs(10);
            let mut out = Completions::default();
            while count < seen.len() {
                assert!(l.now() < until, "post delivery timed out");
                l.turn(Timeout::Until(until), &mut out).expect("turn");
                for c in out.drain() {
                    assert_eq!(thread::current().id(), owner);
                    assert_eq!(c.token, Token(id as u64));
                    if let OpResult::Posted(Payload::U64(n)) = c.result {
                        assert!(!seen[n as usize]);
                        seen[n as usize] = true;
                        count += 1;
                    } else {
                        panic!("wrong completion");
                    }
                }
            }
            assert!(seen.into_iter().all(|v| v));
            count
        }));
    }
    let mut posters = vec![None; threads];
    for _ in 0..threads {
        let (id, p) = rx.recv().expect("poster");
        posters[id] = Some(p);
    }
    let posters: Vec<_> = posters
        .into_iter()
        .map(|p| p.expect("all posters"))
        .collect();
    for s in sends {
        s.send(posters.clone()).expect("distribute");
    }
    let total: usize = workers.into_iter().map(|w| w.join().expect("worker")).sum();
    assert_eq!(total, threads * threads * per_peer);
}
mod extended;
pub use extended::*;

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
))]
mod integration;
#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
))]
pub use integration::integration_fd;

pub fn writev_and_shutdown<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (_, a, b) = pair(&mut l);
    const HALF: usize = 256 * 1024;
    let v = WriteVectored::new([
        WriteBuf::Owned(vec![0xa3; HALF]),
        WriteBuf::Owned(vec![0x5c; HALF]),
    ])
    .expect("iovecs");
    let read = l.read_start(b, Token(1)).expect("read_start");
    l.writev(a, v, Token(2)).expect("writev");
    l.shutdown(a, Token(3)).expect("shutdown after write");
    let mut count = 0;
    let mut writes = 0;
    let mut shutdowns = 0;
    let mut eof = false;
    let mut chunks = 0;
    let until = l.now() + Duration::from_secs(5);
    let mut out = Completions::default();
    while !eof || writes == 0 || shutdowns == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read {
                    n,
                    lease: Some(data),
                } => {
                    assert!(!c.terminal);
                    assert_eq!(c.op, Some(read));
                    assert!(n > 0);
                    chunks += 1;
                    for &byte in data.as_slice() {
                        assert_eq!(byte, if count < HALF { 0xa3 } else { 0x5c });
                        count += 1;
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, 2 * HALF);
                    writes += 1;
                }
                OpResult::Shutdown => {
                    shutdowns += 1;
                }
                OpResult::Eof => {
                    assert!(c.terminal);
                    assert_eq!(c.op, Some(read));
                    assert_eq!(count, 2 * HALF);
                    eof = true;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(chunks > 1);
    assert_eq!(writes, 1);
    assert_eq!(shutdowns, 1);
    assert!(!l.cancel(read));
}

pub fn capacity_and_stale_ids<B: Backend>() {
    let config = Config {
        max_handles: 4,
        max_operations: 2,
        events_per_turn: 1,
        ..Config::default()
    };
    let mut a = Driver::<B>::new(config).expect("a");
    let mut b = Driver::<B>::new(config).expect("b");
    let at = a.now() + Duration::from_secs(30);
    let h = a.timer(at, None, Token(1)).expect("timer");
    let op = a.timer_op(h).expect("op");
    assert!(a.cancel(op));
    a.close(h, Token(2)).expect("close");
    let other = b.timer(at, None, Token(3)).expect("other loop timer");
    assert!(!a.cancel(b.timer_op(other).expect("op")));
    assert!(a.set_ref(other, false).is_err());
    let h2 = a.timer(at, None, Token(4)).expect("second credit");
    let op2 = a.timer_op(h2).expect("op");
    assert!(a.cancel(op2));
    assert!(
        matches!(
            a.timer(at, None, Token(5)),
            Err(Error {
                kind: ErrorKind::ResourceLimit,
                ..
            })
        ),
        "undelivered completions retain credits"
    );
    let mut out = Completions::with_capacity(1);
    let mut count = 0;
    while count < 3 {
        let info = a
            .turn(Timeout::Forever, &mut out)
            .expect("queued work never waits");
        assert_eq!((info.os_waits, info.discovery_polls), (0, 0));
        assert_eq!(out.len(), 1);
        count += 1;
    }
    let h3 = a.timer(at, None, Token(6)).expect("reused storage");
    assert_ne!(h3, h);
    assert!(a.set_ref(h, true).is_err());
    assert!(!a.cancel(op));
    assert!(a.cancel(a.timer_op(h3).expect("op")));
    assert_eq!(count, 3);
}

pub fn pooled_lease_backpressure<B: Backend>() {
    let mut l = Driver::<B>::new(Config {
        pooled_buffers: 1,
        pooled_buffer_size: 1,
        ..Config::default()
    })
    .expect("loop");
    let (_, a, b) = pair(&mut l);
    let op = l.read_start(b, Token(1)).expect("read");
    l.write(a, WriteBuf::Owned(b"ab".to_vec()), Token(2))
        .expect("write");
    let mut out = Completions::default();
    let mut held = None;
    let mut writes = 0;
    let until = l.now() + Duration::from_secs(2);
    while held.is_none() || writes == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read {
                    n,
                    lease: Some(data),
                } => {
                    assert_eq!(n, 1);
                    assert_eq!(data.as_slice(), b"a");
                    assert!(held.is_none());
                    held = Some(data);
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, 2);
                    writes += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    for _ in 0..3 {
        l.turn(Timeout::Now, &mut out).expect("backpressure");
        assert!(out.is_empty());
        assert_eq!(held.as_ref().expect("live lease").as_slice(), b"a");
    }
    drop(held.take());
    l.turn(Timeout::Now, &mut out)
        .expect("resume cached readiness");
    assert_eq!(out.len(), 1);
    let c = out.drain().next().expect("resumed read");
    if let OpResult::Read {
        n,
        lease: Some(data),
    } = c.result
    {
        assert_eq!(n, 1);
        assert_eq!(data.as_slice(), b"b");
    } else {
        panic!("expected second byte");
    }
    assert!(l.stop(op));
    l.turn(Timeout::Now, &mut out).expect("stop");
    assert!(matches!(out[0].result, OpResult::Stopped));
    assert_eq!(writes, 1);
}

pub fn ready_timer_liveness<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let at = l.now();
    for i in 0..3 {
        l.timer(at, None, Token(i)).expect("timer");
    }
    let mut out = Completions::with_capacity(1);
    let mut delivered = 0;
    while l.alive() {
        assert!(delivered < 3, "liveness never clears");
        l.turn(Timeout::Now, &mut out).expect("turn");
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].result, OpResult::Timer));
        delivered += 1;
    }
    assert_eq!(
        delivered, 3,
        "all referenced results delivered before loop can exit"
    );
    let at = l.now();
    let a = l.timer(at, None, Token(10)).expect("timer");
    let b = l.timer(at, None, Token(11)).expect("timer");
    l.turn(Timeout::Now, &mut out).expect("first result");
    assert_eq!(out.len(), 1);
    assert!(l.alive());
    let pending = if out[0].handle == Some(a) { b } else { a };
    l.set_ref(pending, false).expect("unref queued result");
    assert!(!l.alive());
    l.set_ref(pending, true).expect("ref queued result");
    assert!(l.alive());
    l.turn(Timeout::Now, &mut out).expect("last result");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].handle, Some(pending));
    assert!(matches!(out[0].result, OpResult::Timer));
    assert!(!l.alive());

    let h = l
        .timer(l.now() + Duration::from_secs(30), None, Token(20))
        .expect("timer to close");
    l.close(h, Token(21)).expect("cancel and close");
    let mut cancelled = 0;
    let mut closed = 0;
    while l.alive() {
        assert!(cancelled + closed < 2);
        l.turn(Timeout::Now, &mut out).expect("close delivery");
        assert_eq!(out.len(), 1);
        match out[0].result {
            OpResult::Cancelled => {
                cancelled += 1;
                assert!(l.alive(), "Closed still retains its reference");
                l.set_ref(h, false).expect("unref closing timer");
                assert!(!l.alive());
                l.set_ref(h, true).expect("ref closing timer");
                assert!(l.alive());
            }
            OpResult::Closed => {
                assert_eq!(cancelled, 1);
                closed += 1;
            }
            ref other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((cancelled, closed), (1, 1));
}
/// DESIGN §10.3: queued core work (posts, timer Cancelled/Closed results) makes
/// no native call at all when no native operation is pending.
pub fn queued_core_work<B: Backend>() {
    let mut driver = Driver::<B>::new(Config::default()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let mut delivered = 0;
    for timeout in [
        Timeout::Now,
        Timeout::After(Duration::from_secs(1)),
        Timeout::Forever,
    ] {
        // Web only permits Now; validation is separately tested there.
        if cfg!(all(target_arch = "wasm32", target_os = "unknown"))
            && !matches!(timeout, Timeout::Now)
        {
            continue;
        }
        driver
            .poster()
            .post(Token(1), Payload::U64(42))
            .expect("post");
        let info = driver.turn(timeout, &mut out).expect("post turn");
        assert_eq!(
            (info.os_waits, info.discovery_polls, info.zero_event_waits),
            (0, 0, 0)
        );
        assert_eq!(info.completions, 1);
        assert_eq!(out[0].token, Token(1));
        assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(42))));
        delivered += 1;
        let timer = driver
            .timer(driver.now() + Duration::from_secs(30), None, Token(2))
            .expect("timer");
        driver.close(timer, Token(3)).expect("queue terminals");
        for token in [Token(2), Token(3)] {
            let info = driver.turn(timeout, &mut out).expect("terminal turn");
            assert_eq!(
                (info.os_waits, info.discovery_polls, info.zero_event_waits),
                (0, 0, 0)
            );
            assert_eq!(info.completions, 1);
            assert_eq!(out[0].handle, Some(timer));
            assert_eq!(out[0].token, token);
            assert!(out[0].terminal);
            assert!(if token == Token(2) {
                matches!(out[0].result, OpResult::Cancelled)
            } else {
                matches!(out[0].result, OpResult::Closed)
            });
            delivered += 1;
        }
        assert!(!driver.alive());
    }
    assert!(delivered >= 3);
}

/// DESIGN §10.3: queued posts permit one nonblocking discovery poll with idle I/O.
pub fn queued_post_idle_io<B: Backend>() {
    let mut driver = Driver::<B>::new(Config::default()).expect("loop");
    let socket = driver
        .udp_bind(localhost(), &UdpOpts::default())
        .expect("UDP");
    let read = driver
        .recv(socket, ReadBuf::Pooled, Token(1))
        .expect("idle receive");
    let mut out = Completions::default();
    driver
        .turn(Timeout::Now, &mut out)
        .expect("arm idle receive");
    assert!(out.is_empty(), "receive must actually remain pending");
    driver
        .poster()
        .post(Token(2), Payload::U64(42))
        .expect("post accepted");
    let info = driver
        .turn(Timeout::Forever, &mut out)
        .expect("deliver post");
    assert_eq!(info.completions, 1);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].token, Token(2));
    assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(42))));
    eprintln!(
        "queued post with idle UDP: waits={}, discovery_polls={}, delivered={}",
        info.os_waits, info.discovery_polls, info.completions
    );

    let address = driver.local_addr(socket).expect("UDP address");
    let write = driver
        .send_to(socket, WriteBuf::Owned(vec![0x49]), address, Token(3))
        .expect("later send");
    let until = driver.now() + Duration::from_secs(2);
    let (mut reads, mut writes) = (0, 0);
    while reads + writes < 2 {
        assert!(driver.now() < until, "queued post starved later I/O");
        let info = driver
            .turn(Timeout::Until(until), &mut out)
            .expect("later I/O progress");
        assert!(info.os_waits + info.discovery_polls <= 1);
        for c in out.drain() {
            assert_eq!(c.handle, Some(socket));
            assert!(c.terminal);
            match c.result {
                OpResult::RecvFrom {
                    n,
                    from,
                    lease: Some(lease),
                } => {
                    assert_eq!(c.op, Some(read));
                    assert_eq!(c.token, Token(1));
                    assert_eq!(from, address);
                    assert_eq!(n, 1);
                    assert_eq!(lease.as_slice(), &[0x49]);
                    reads += 1;
                }
                OpResult::Wrote(1) => {
                    assert_eq!(c.op, Some(write));
                    assert_eq!(c.token, Token(3));
                    writes += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((reads, writes), (1, 1));
    eprintln!("later UDP progress: reads={reads}, writes={writes}");
    assert_eq!(
        info.os_waits, 0,
        "DESIGN §10.3: queued post must avoid blocking wait"
    );
    assert_eq!(info.discovery_polls, 1, "idle native discovery must run");
    assert!(info.zero_event_waits <= info.discovery_polls);
}

/// Terminal results must retain the no-wait guarantee under capacity-one output.
pub fn queued_terminals_idle_io<B: Backend>() {
    let mut driver = Driver::<B>::new(Config::default()).expect("loop");
    let socket = driver
        .udp_bind(localhost(), &UdpOpts::default())
        .expect("UDP");
    let read = driver
        .recv(socket, ReadBuf::Pooled, Token(1))
        .expect("idle receive");
    let mut out = Completions::with_capacity(1);
    driver
        .turn(Timeout::Now, &mut out)
        .expect("arm idle receive");
    assert!(out.is_empty());
    let timer = driver
        .timer(driver.now() + Duration::from_secs(30), None, Token(2))
        .expect("timer");
    driver
        .close(timer, Token(3))
        .expect("queue Cancelled and Closed");
    let mut waits = 0;
    let mut discovery_polls = 0;
    for token in [Token(2), Token(3)] {
        let info = driver
            .turn(Timeout::Forever, &mut out)
            .expect("terminal delivery");
        waits += info.os_waits;
        assert_eq!(info.os_waits, 0);
        assert!(info.discovery_polls <= 1);
        discovery_polls += info.discovery_polls;
        assert_eq!(info.completions, 1);
        assert_eq!(
            out.len(),
            1,
            "output must fill without losing the next result"
        );
        assert_eq!(out[0].handle, Some(timer));
        assert_eq!(out[0].token, token);
        assert!(out[0].terminal);
        if token == Token(2) {
            assert!(out[0].op.is_some());
            assert!(matches!(out[0].result, OpResult::Cancelled));
        } else {
            assert!(out[0].op.is_none());
            assert!(matches!(out[0].result, OpResult::Closed));
        }
    }
    assert_eq!(
        discovery_polls, 2,
        "both queued terminal turns discover idle I/O"
    );
    // A due timer is not queued work: with native I/O pending its zero effective
    // wait is one discovery poll, never a blocking wait.
    let due = driver
        .timer(driver.now(), None, Token(4))
        .expect("due timer");
    let info = driver.turn(Timeout::Forever, &mut out).expect("due timer");
    assert_eq!((info.os_waits, info.discovery_polls), (0, 1));
    discovery_polls += info.discovery_polls;
    assert_eq!((out.len(), out[0].handle), (1, Some(due)));
    assert!(matches!(out[0].result, OpResult::Timer));
    driver.close(due, Token(5)).expect("close due timer");
    let info = driver.turn(Timeout::Forever, &mut out).expect("due close");
    assert_eq!((info.os_waits, info.discovery_polls), (0, 1));
    discovery_polls += info.discovery_polls;
    assert!(matches!(out[0].result, OpResult::Closed));
    assert!(driver.cancel(read), "UDP remained pending throughout");
    eprintln!(
        "queued terminals with idle UDP: delivered=2, waits={waits}, discovery_polls={discovery_polls}"
    );
    assert_eq!(
        waits, 0,
        "DESIGN §10.3 includes queued terminal completions"
    );
}

/// Replenishing a post before every turn leaves no post-free discovery turn.
/// A separate driver sends only after the receiver has exhausted cached readiness.
pub fn sustained_posts_idle_io<B: Backend>() {
    let mut receiver = Driver::<B>::new(Config::default()).expect("receiver loop");
    let mut sender = Driver::<B>::new(Config::default()).expect("sender loop");
    let rx = receiver
        .udp_bind(localhost(), &UdpOpts::default())
        .expect("receiver UDP");
    let tx = sender
        .udp_bind(localhost(), &UdpOpts::default())
        .expect("sender UDP");
    let destination = receiver.local_addr(rx).expect("destination");
    let source = sender.local_addr(tx).expect("source");
    assert_ne!(source, destination);
    let read = receiver
        .recv(rx, ReadBuf::Pooled, Token(1))
        .expect("idle receive");
    let mut out = Completions::with_capacity(1);
    receiver
        .turn(Timeout::Now, &mut out)
        .expect("exhaust cached readiness");
    assert!(out.is_empty());
    let write = sender
        .send_to(tx, WriteBuf::Owned(vec![0x49]), destination, Token(2))
        .expect("send after idle");
    let until = sender.now() + Duration::from_secs(2);
    loop {
        assert!(sender.now() < until, "sender did not run");
        sender
            .turn(Timeout::Until(until), &mut out)
            .expect("sender turn");
        if let Some(c) = out.drain().next() {
            assert_eq!(c.op, Some(write));
            assert!(matches!(c.result, OpResult::Wrote(1)));
            break;
        }
    }
    let poster = receiver.poster();
    let mut discovery_polls = 0;
    let (mut waits, mut posts, mut reads, mut turns) = (0, 0, 0, 0);
    let until = receiver.now() + Duration::from_secs(2);
    // Keep replenishing until I/O arrives, with the existing fairness contract's
    // wall-clock deadline. The minimum count proves sustained producer traffic.
    while turns < 64 || reads == 0 {
        assert!(
            receiver.now() < until,
            "fresh I/O starved: turns={turns}, posts={posts}, reads={reads}, waits={waits}, discovery_polls={discovery_polls}"
        );
        poster
            .post(Token(3), Payload::U64(42))
            .expect("replenish producer");
        let info = receiver
            .turn(Timeout::Now, &mut out)
            .expect("receiver turn");
        waits += info.os_waits;
        assert_eq!(info.os_waits, 0);
        assert!(info.discovery_polls <= 1);
        if reads != 0 {
            assert_eq!(info.discovery_polls, 0, "no native operations remain");
        }
        discovery_polls += info.discovery_polls;
        turns += 1;
        assert_eq!(out.len(), 1);
        for c in out.drain() {
            match c.result {
                OpResult::Posted(Payload::U64(value)) => {
                    assert_eq!(c.token, Token(3));
                    assert_eq!(value, 42);
                    posts += 1;
                }
                OpResult::RecvFrom {
                    n,
                    from,
                    lease: Some(lease),
                } => {
                    assert_eq!(c.op, Some(read));
                    assert_eq!(c.handle, Some(rx));
                    assert!(c.terminal);
                    assert_eq!(from, source);
                    assert_eq!(n, 1);
                    assert_eq!(lease.as_slice(), &[0x49]);
                    reads += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    eprintln!(
        "sustained producer: turns={turns}, posts={posts}, reads={reads}, waits={waits}, discovery_polls={discovery_polls}"
    );
    assert_eq!(
        posts,
        turns - 1,
        "posts must make progress with full output"
    );
    assert_eq!(
        reads, 1,
        "fresh native readiness must progress through queued posts"
    );
    assert!(discovery_polls > 0, "fresh I/O discovery must run");
    let info = receiver.turn(Timeout::Now, &mut out).expect("last post");
    assert_eq!((info.os_waits, info.discovery_polls), (0, 0));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].token, Token(3));
    assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(42))));
    assert_eq!(
        waits, 0,
        "DESIGN §10.3 also holds under sustained producer traffic"
    );
}

pub fn io_and_posts_progress_with_repeating_timers<B: Backend>() {
    let mut l = Driver::<B>::new(Config {
        max_handles: 8,
        max_operations: 8,
        events_per_turn: 2,
        ..Config::default()
    })
    .expect("loop");
    let (_, a, b) = pair(&mut l);
    for i in 0..4 {
        l.timer(l.now(), Some(Duration::from_nanos(1)), Token(i))
            .expect("repeat");
    }
    let poster = l.poster();
    let mut out = Completions::with_capacity(1);
    for _ in 0..10 {
        l.turn(Timeout::Now, &mut out).expect("build timer backlog");
    }
    poster
        .post(Token(99), Payload::U64(42))
        .expect("post amid timers");
    l.read(b, ReadBuf::Pooled, Token(100)).expect("read");
    l.write(a, WriteBuf::Owned(vec![0xb7; 64]), Token(101))
        .expect("write");
    let mut received = false;
    let mut timers = 0;
    let mut read = 0;
    let mut wrote = 0;
    let until = l.now() + Duration::from_secs(2);
    while !received || read < 64 || wrote == 0 {
        assert!(l.now() < until, "timer backlog starved I/O or posts");
        l.turn(Timeout::Now, &mut out).expect("bounded progress");
        for c in out.drain() {
            match c.result {
                OpResult::Posted(Payload::U64(42)) => {
                    assert_eq!(c.token, Token(99));
                    assert!(!received);
                    received = true;
                }
                OpResult::Timer => timers += 1,
                OpResult::Read { n, lease } => {
                    let lease = lease.expect("pooled read");
                    assert!(n > 0);
                    assert_eq!(lease.as_slice().len(), n);
                    assert!(lease.as_slice().iter().all(|&v| v == 0xb7));
                    read += n;
                    if read < 64 {
                        l.read(b, ReadBuf::Pooled, Token(100)).expect("read rest");
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, 64);
                    wrote += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(received, "posts must progress through a timer backlog");
    assert!(timers > 0);
    assert_eq!((read, wrote), (64, 1));
}

/// Run the three design delays (0.5 ms, 2 ms, 10 ms) twenty times each against
/// `l`, reusing the caller's storage, and return
/// `[os_waits, discovery_polls, zero_event_waits, expiries]`.
///
/// DESIGN §10 rules 3, 4 and 4a. A deadline with nothing else ready is *one*
/// native call that observes *no* native event, and the timer is the only
/// completion. A backend that reported its own private timeout source as native
/// work would record `zero_event_waits == 0` here: epoll normalizes its timerfd,
/// IOCP its deadline packet, WASI 0.2 its deadline pollable and WASI 0.3 its
/// deadline subtask. Each of the sixty measured expiries is a real blocking
/// wait; a round whose own setup outran the delay is already due at turn entry,
/// which DESIGN rule 3 lets spend its one call on a zero-timeout poll instead,
/// so it is asserted in that shape and re-run rather than measured (a cold or
/// preempted process does that a few times; sixteen consecutive failures mean
/// the delay itself can no longer be measured). `native_pending` states whether a
/// native operation is registered, the only thing allowed to differ afterwards:
/// the queued `Closed` turn may then spend its one call on a discovery poll.
pub fn quiet_deadlines<B: Backend>(
    l: &mut Driver<B>,
    out: &mut Completions,
    native_pending: bool,
) -> [u32; 4] {
    let mut totals = [0; 4];
    let mut late = Duration::ZERO;
    for delay in [
        Duration::from_micros(500),
        Duration::from_millis(2),
        Duration::from_millis(10),
    ] {
        for round in 0..20 {
            let token = Token(500 + delay.as_micros() as u64 + round);
            let mut measured = false;
            let mut attempt = 0;
            while !measured {
                attempt += 1;
                assert!(
                    attempt <= 16,
                    "{delay:?}: the timer setup never finished inside its own delay"
                );
                let at = l.now() + delay;
                let h = l.timer(at, None, token).expect("timer");
                let op = l.timer_op(h).expect("timer operation");
                let info = l.turn(Timeout::Until(at), out).expect("quiet wait");
                // One native call, and it observed no native event at all.
                assert_eq!(
                    (info.os_waits + info.discovery_polls, info.zero_event_waits),
                    (1, 1),
                    "{delay:?} deadline: one native call observing no native event"
                );
                // A blocking wait is the measurable case. A preempted setup that
                // outran its own delay leaves the expiry already due at turn
                // entry, which DESIGN rule 3 lets spend the one call on a
                // zero-timeout poll; that round is re-run, never counted, so a
                // backend that only ever polls can never reach sixty expiries.
                let quiet = info.os_waits == 1;
                // Exactly one turn per expiry: the completion is already here.
                assert_eq!(out.len(), 1, "{delay:?} deadline needed a second turn");
                assert_eq!(
                    (out[0].handle, out[0].op, out[0].token),
                    (Some(h), Some(op), token)
                );
                assert!(matches!(out[0].result, OpResult::Timer));
                // The host deadline is honoured exactly, with no wait floor
                // rounding it up: sub-millisecond delays expire like the others.
                assert!(l.now() >= at, "{delay:?} deadline fired early");
                late = late.max(l.now().saturating_duration_since(at));
                assert!(
                    l.now().saturating_duration_since(at) < Duration::from_millis(100),
                    "{delay:?} deadline overshot its wait"
                );
                if quiet {
                    totals[0] += info.os_waits;
                    totals[1] += info.discovery_polls;
                    totals[2] += info.zero_event_waits;
                    totals[3] += 1;
                    measured = true;
                }
                l.close(h, token).expect("close timer");
                let info = l.turn(Timeout::Now, out).expect("queued close");
                assert_eq!(info.os_waits, 0, "a queued close must not block");
                assert!(
                    info.discovery_polls <= u32::from(native_pending),
                    "queued close polled without a pending native operation"
                );
                assert!(info.zero_event_waits <= info.discovery_polls);
                assert_eq!(out.len(), 1);
                assert!(matches!(out[0].result, OpResult::Closed));
            }
        }
    }
    // Never print here: allocation gates call this with their counter armed.
    assert!(late < Duration::from_millis(100));
    totals
}

/// DESIGN §10 rule 4a and the `PollInfo` contract: quiet deadline accounting is
/// the same whether the loop is idle or has registered-but-idle native services.
pub fn quiet_deadline_accounting<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let idle = quiet_deadlines(&mut l, &mut out, false);
    let (_, sender, receiver) = pair(&mut l);
    let read = l
        .read(receiver, ReadBuf::Pooled, Token(2))
        .expect("idle read");
    // Submit the read before measuring: the registered phase must differ from the
    // idle one by a pending native operation only, not by a first-submission turn.
    let info = l.turn(Timeout::Now, &mut out).expect("arm the idle read");
    assert!(out.is_empty(), "the registered read must stay idle");
    assert_eq!(info.os_waits, 0, "a zero timeout never blocks");
    let registered = quiet_deadlines(&mut l, &mut out, true);
    assert_eq!(
        idle, registered,
        "registered-but-idle services changed quiet-deadline accounting"
    );
    assert_eq!(
        idle,
        [60, 0, 60, 60],
        "sixty expiries, each one blocking wait that saw no native event"
    );
    // The same registered read now takes real bytes inside a deadline-bounded
    // wait. Normalizing the private deadline must not empty a call that found
    // native work: this is the control the accounting above needs.
    l.write(sender, WriteBuf::Owned(vec![0x2a]), Token(3))
        .expect("write");
    let until = l.now() + Duration::from_secs(5);
    let (mut calls, mut empty, mut bytes, mut wrote) = (0, 0, 0, 0);
    while bytes == 0 || wrote == 0 {
        assert!(l.now() < until, "the loopback exchange stalled");
        let info = l.turn(Timeout::Until(until), &mut out).expect("exchange");
        assert!(info.os_waits + info.discovery_polls <= 1);
        calls += info.os_waits + info.discovery_polls;
        empty += info.zero_event_waits;
        for c in out.drain() {
            match c.result {
                OpResult::Wrote(1) => wrote += 1,
                OpResult::Read {
                    n: 1,
                    lease: Some(b),
                } => {
                    assert_eq!(c.op, Some(read), "the idle read survived every expiry");
                    assert_eq!(b.as_slice(), [0x2a]);
                    bytes += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(calls > 0, "the exchange must make a native call");
    assert_eq!(
        empty, 0,
        "a native call that carried real I/O is not a zero-event wait"
    );
    eprintln!(
        "quiet deadlines idle={idle:?} registered={registered:?}; exchange calls={calls} empty={empty}"
    );
}

/// DESIGN §10 rule 4a: an idle registered stream must not turn timers into polling.
pub fn no_spin<B: Backend>() {
    let mut driver = Driver::<B>::new(Config::default()).expect("loop");
    let (_, _sender, receiver) = pair(&mut driver);
    let read = driver
        .read(receiver, ReadBuf::Pooled, Token(90))
        .expect("idle read");
    let mut out = Completions::default();
    let mut expiries = 0;
    let mut waits = 0;
    for micros in [500, 2_000, 10_000] {
        for _ in 0..20 {
            let deadline = driver.now() + Duration::from_micros(micros);
            let timer = driver.timer(deadline, None, Token(91)).expect("timer");
            let mut turns = 0;
            let mut zero_events = 0;
            loop {
                turns += 1;
                assert!(turns <= 2, "{micros} us deadline spun before expiry");
                let info = driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("turn");
                assert!(info.os_waits + info.discovery_polls <= 1);
                waits += info.os_waits + info.discovery_polls;
                zero_events += info.zero_event_waits;
                assert!(zero_events <= 1, "{micros} us: repeated empty OS waits");
                if !out.is_empty() {
                    assert_eq!(out.len(), 1);
                    assert_eq!(out[0].token, Token(91));
                    assert_eq!(out[0].handle, Some(timer));
                    assert!(matches!(out[0].result, OpResult::Timer));
                    assert!(driver.now() >= deadline, "timer fired early");
                    expiries += 1;
                    break;
                }
            }
            driver
                .close(timer, Token(92))
                .expect("release timer handle");
            driver.turn(Timeout::Now, &mut out).expect("drain close");
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Closed));
        }
    }
    assert_eq!(expiries, 60);
    assert!(waits >= 60, "timer waits must actually execute");
    assert!(
        driver.cancel(read),
        "idle socket read remained pending throughout"
    );
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub mod filesystem;
pub mod native_surface;
pub mod sockopts;

#[cfg(feature = "executor")]
pub mod executor_contract;

/// Revision-2 contracts for a single owning WASI/web agent.
#[cfg(target_arch = "wasm32")]
pub mod single_agent {
    use std::{
        sync::{Arc, atomic::AtomicU64},
        time::Duration,
    };
    use turnloop::{
        backend::{Backend, Operation, Request},
        *,
    };

    pub fn unsupported_native<B: Backend>() {
        let mut l = Driver::<B>::new(Config::default()).expect("loop");
        let path = PipeName("unsupported.sock".into());
        for _ in 0..16 {
            assert_eq!(
                l.pipe_connect(&path, Token(1)).expect_err("local IPC").kind,
                ErrorKind::Unsupported
            );
            assert_eq!(
                l.pipe_listen(&path, &ListenOpts::default())
                    .expect_err("listener")
                    .kind,
                ErrorKind::Unsupported
            );
            let mut spec = ProcessSpec::new("unavailable");
            spec.stdio = [ProcessStdio::Pipe; 3];
            assert_eq!(
                l.spawn(&spec, Token(2)).expect_err("process").kind,
                ErrorKind::Unsupported
            );
            assert_eq!(
                l.signal_start(Signal::Int, Token(3))
                    .expect_err("signal")
                    .kind,
                ErrorKind::Unsupported
            );
            assert!(!l.alive(), "rejected setup leaked core credits");
        }
        let h = l
            .timer(l.now() + Duration::from_secs(1), None, Token(4))
            .expect("identity");
        let op = l.timer_op(h).expect("operation identity");
        assert_eq!(
            l.kill(h, Signal::Kill).expect_err("kill").kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.kill_group(h, Signal::Kill).expect_err("group").kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.tty_set_mode(h, TtyMode::Raw).expect_err("TTY").kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.tty_window_size(h).expect_err("window size").kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.tty_resize_start(h, Token(5)).expect_err("resize").kind,
            ErrorKind::Unsupported
        );
        let mut backend = B::new(&Config::default(), BufferPool::new(2, 64)).expect("backend");
        for operation in [
            Operation::ProcessExit,
            Operation::WatchSignal,
            Operation::SendHandle(h),
            Operation::RecvHandle,
        ] {
            assert_eq!(
                backend
                    .submit(Request {
                        op,
                        handle: h,
                        operation
                    })
                    .expect_err("native operation")
                    .kind,
                ErrorKind::Unsupported
            );
        }
        assert!(!backend.has_work(), "unsupported requests retained no work");
    }

    pub fn waits<B: Backend>() {
        let cfg = Config {
            max_operations: 4,
            ..Config::default()
        };
        let mut a = Driver::<B>::new(cfg).expect("first loop");
        let mut b = Driver::<B>::new(cfg).expect("second loop");
        let condition = WaitCondition::from_atomic(Arc::new(AtomicU64::new(7))).expect("condition");
        let mut out = Completions::with_capacity(1);
        let mut delivered = 0;
        for round in 0..32 {
            let mismatch = a
                .external_wait(&condition, 8, None, Token(1))
                .expect("mismatch");
            a.turn(Timeout::Now, &mut out).expect("mismatch turn");
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].op, Some(mismatch));
            assert!(matches!(
                out[0].result,
                OpResult::ExternalWait(WaitResult::NotEqual)
            ));
            let first = a
                .external_wait(&condition, 7, None, Token(2))
                .expect("wait A");
            let second = b
                .external_wait(&condition, 7, None, Token(3))
                .expect("wait B");
            assert!(!a.cancel(second), "foreign identity rejected");
            assert!(a.alive() && b.alive());
            condition.notify(); // Same-value notification must complete both registrations.
            if round % 2 == 0 {
                assert!(a.cancel(first));
            }
            for (driver, id, token) in [(&mut a, first, Token(2)), (&mut b, second, Token(3))] {
                driver.turn(Timeout::Now, &mut out).expect("notify turn");
                assert_eq!(out.len(), 1);
                assert_eq!((out[0].op, out[0].token), (Some(id), token));
                if token == Token(2) && round % 2 == 0 {
                    assert!(matches!(out[0].result, OpResult::Cancelled));
                } else {
                    assert!(matches!(
                        out[0].result,
                        OpResult::ExternalWait(WaitResult::Notified)
                    ));
                }
                assert!(!driver.cancel(id));
                assert!(!driver.alive());
            }
            let stopped = a
                .external_wait(&condition, 7, None, Token(4))
                .expect("stop wait");
            assert!(a.stop(stopped));
            a.turn(Timeout::Now, &mut out).expect("stop turn");
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Stopped));
            let expired = a
                .external_wait(&condition, 7, Some(a.now()), Token(5))
                .expect("deadline");
            a.turn(Timeout::Now, &mut out).expect("expiry turn");
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].op, Some(expired));
            assert!(matches!(
                out[0].result,
                OpResult::ExternalWait(WaitResult::TimedOut)
            ));
            assert_eq!(a.next_deadline(), None);
            delivered += 5;
        }
        for _ in 0..4 {
            a.external_wait(&condition, 7, None, Token(6))
                .expect("drop wait");
        }
        assert_eq!(
            a.external_wait(&condition, 7, None, Token(7))
                .expect_err("bounded")
                .kind,
            ErrorKind::ResourceLimit
        );
        drop(a);
        let live = b
            .external_wait(&condition, 7, None, Token(8))
            .expect("survivor");
        condition.store(9);
        b.turn(Timeout::Now, &mut out).expect("survivor turn");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op, Some(live));
        assert!(matches!(
            out[0].result,
            OpResult::ExternalWait(WaitResult::Notified)
        ));
        b.turn(Timeout::Now, &mut out).expect("no duplicate");
        assert!(out.is_empty());
        assert_eq!(condition.load(), 9);
        assert_eq!(delivered, 160);
        assert!(!b.alive());
    }
}
