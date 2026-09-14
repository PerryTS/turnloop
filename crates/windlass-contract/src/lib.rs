#![deny(unsafe_op_in_unsafe_fn)]
//! Shared contract scenarios. Platform lanes instantiate these with their Backend;
//! assertions and test logic are identical on all capable backends.
use std::{
    net::{Ipv4Addr, SocketAddr},
    thread,
    time::{Duration, Instant},
};
use windlass::{backend::Backend, *};
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
    let data = b"windlass echo: every byte matters";
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
    type B = windlass::backend::Platform;
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
        assert!(info.os_waits <= 1);
        for c in out.drain() {
            match c.result {
                OpResult::Cancelled => {
                    assert!(c.terminal);
                    terminal.push(c.op.expect("op"));
                }
                OpResult::Closed => {
                    assert_eq!(terminal, [first, second]);
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
            assert!(matches!(
                c.result,
                OpResult::Err(Error {
                    kind: ErrorKind::ConnectionRefused,
                    ..
                })
            ));
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
                    assert_eq!(n, bytes.len());
                    assert_eq!(data.as_slice(), bytes);
                    received += 1;
                    if c.token == Token(1) {
                        assert_eq!(from, aa);
                        l.recv(a, ReadBuf::Pooled, Token(3)).expect("recv return");
                        l.send_to(b, WriteBuf::Owned(data.as_slice().to_vec()), from, Token(4))
                            .expect("return send");
                    } else {
                        assert_eq!(from, ba);
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
        assert_eq!(info.os_waits, 0);
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
