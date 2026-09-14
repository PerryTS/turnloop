use super::*;
pub fn detach_inflight<B: Backend>() {
    let mut source = Driver::<B>::new(Config::default()).expect("loop");
    let (_, a, b) = pair(&mut source);
    let mut untouched = [0x7d; 16];
    // SAFETY: region stays fixed and inaccessible until cancellation delivery.
    let buf = unsafe { IoBufMut::from_raw_parts(untouched.as_mut_ptr(), untouched.len()) };
    let read = source
        .read(b, ReadBuf::Provided(buf), Token(4))
        .expect("pending read");
    let first = source.detach(b);
    assert!(matches!(
        first,
        Err(Error {
            kind: ErrorKind::WouldBlock,
            ..
        })
    ));
    let mut out = Completions::default();
    source.turn(Timeout::Now, &mut out).expect("cancel turn");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].op, Some(read));
    assert!(matches!(out[0].result, OpResult::Cancelled));
    assert_eq!(untouched, [0x7d; 16]);
    let d = source.detach(b).expect("quiescent detach");
    assert!(source.read(b, ReadBuf::Pooled, Token(5)).is_err());
    let worker = thread::spawn(move || {
        let mut l = Driver::<B>::new(Config::default()).expect("destination");
        let h = l.attach(d, Token(8)).expect("attach");
        l.read(h, ReadBuf::Pooled, Token(9)).expect("read");
        let mut out = Completions::default();
        let until = Instant::now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        let mut wrote = 0;
        while wrote == 0 {
            assert!(Instant::now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                match c.result {
                    OpResult::Read {
                        n,
                        lease: Some(data),
                    } => {
                        assert!(n > 0);
                        bytes.extend_from_slice(data.as_slice());
                        if bytes.len() == 4 {
                            assert_eq!(bytes, b"move");
                            l.write(h, WriteBuf::Owned(bytes.clone()), Token(10))
                                .expect("write");
                        } else {
                            l.read(h, ReadBuf::Pooled, Token(9)).expect("continue");
                        }
                    }
                    OpResult::Wrote(n) => {
                        assert_eq!(n, 4);
                        wrote += 1;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        assert_eq!(wrote, 1);
    });
    source
        .write(a, WriteBuf::Owned(b"move".to_vec()), Token(6))
        .expect("write");
    source.read(a, ReadBuf::Pooled, Token(7)).expect("read");
    let mut bytes = Vec::new();
    let until = Instant::now() + Duration::from_secs(3);
    while bytes.len() < 4 {
        assert!(Instant::now() < until);
        source.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read {
                    n,
                    lease: Some(data),
                } => {
                    assert!(n > 0);
                    bytes.extend_from_slice(data.as_slice());
                    if bytes.len() < 4 {
                        source.read(a, ReadBuf::Pooled, Token(7)).expect("continue");
                    }
                }
                OpResult::Wrote(n) => assert_eq!(n, 4),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(bytes, b"move");
    worker.join().expect("destination");
    assert_eq!(untouched, [0x7d; 16]);
}
pub fn pool_and_dns<B: Backend>() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let owner = thread::current().id();
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = ran.clone();
    l.blocking(
        move || {
            assert_ne!(thread::current().id(), owner);
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(Payload::U64(123))
        },
        Token(1),
    )
    .expect("blocking");
    l.resolve(
        DnsRequest {
            host: "localhost".into(),
            port: 8080,
        },
        Token(2),
    )
    .expect("resolve");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let counter = ran.clone();
    let op = l
        .blocking(
            move || {
                started_tx.send(()).expect("start");
                release_rx.recv().expect("release");
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(Payload::U64(999))
            },
            Token(3),
        )
        .expect("cancellable job");
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("job actually started");
    assert!(l.cancel(op));
    assert!(!l.cancel(op));
    release_tx.send(()).expect("release");
    let until = Instant::now() + Duration::from_secs(5);
    let mut seen = [false; 3];
    let mut out = Completions::default();
    while !seen.iter().all(|v| *v) {
        assert!(Instant::now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert_eq!(thread::current().id(), owner);
            let i = c.token.0 as usize - 1;
            assert!(!seen[i]);
            seen[i] = true;
            match c.result {
                OpResult::Blocking(Payload::U64(123)) => assert_eq!(i, 0),
                OpResult::Resolved(a) => {
                    assert_eq!(i, 1);
                    assert!(!a.is_empty());
                    assert!(a.iter().all(|a| a.ip().is_loopback() && a.port() == 8080));
                }
                OpResult::Cancelled => assert_eq!(i, 2),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(ran.load(Ordering::Relaxed), 2);
    assert!(!l.alive());
    for _ in 0..5 {
        l.turn(Timeout::Now, &mut out).expect("drain");
        assert!(out.is_empty());
    }
}
pub fn reuse_port<B: Backend>() {
    let mut a = Driver::<B>::new(Config::default()).expect("loop a");
    let mut b = Driver::<B>::new(Config::default()).expect("loop b");
    let opts = ListenOpts {
        reuse_port: true,
        ..ListenOpts::default()
    };
    let ah = a.tcp_listen(localhost(), &opts).expect("listen a");
    let addr = a.local_addr(ah).expect("addr");
    let bh = b.tcp_listen(addr, &opts).expect("listen b same port");
    a.accept_start(ah, Token(1)).expect("accept a");
    b.accept_start(bh, Token(2)).expect("accept b");
    let clients: Vec<_> = (0..32)
        .map(|_| std::net::TcpStream::connect(addr).expect("connect"))
        .collect();
    let mut count = [0usize; 2];
    let mut out = Completions::default();
    let until = Instant::now() + Duration::from_secs(3);
    while count.iter().sum::<usize>() < clients.len() {
        assert!(Instant::now() < until);
        for l in [&mut a, &mut b] {
            l.turn(Timeout::Now, &mut out).expect("turn");
            for c in out.drain() {
                assert!(matches!(c.result, OpResult::Accepted { .. }));
                count[c.token.0 as usize - 1] += 1;
            }
        }
    }
    assert_eq!(count.iter().sum::<usize>(), 32);
    // DESIGN §5a explicitly says macOS does not kernel-balance SO_REUSEPORT.
    if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        assert!(
            count.iter().all(|n| *n > 0),
            "kernel distribution {count:?}"
        );
    }
    eprintln!("reuse-port accepts: {count:?}");
}
pub fn handoff_distribution<B: Backend>() {
    use std::io::{Read, Write};
    const WORKERS: usize = 4;
    const CONNECTIONS: usize = 16;
    let mut primary = Driver::<B>::new(Config::default()).expect("primary");
    let listener = primary
        .tcp_listen(localhost(), &ListenOpts::default())
        .expect("listen");
    primary
        .accept_start(listener, Token(0))
        .expect("accept_start");
    let addr = primary.local_addr(listener).expect("addr");
    let mut senders = Vec::new();
    let mut workers = Vec::new();
    for _ in 0..WORKERS {
        let (tx, rx) = std::sync::mpsc::channel::<B::Detached>();
        senders.push(tx);
        workers.push(thread::spawn(move || {
            let mut l = Driver::<B>::new(Config::default()).expect("worker loop");
            let mut completed = 0;
            for _ in 0..CONNECTIONS / WORKERS {
                let h = l
                    .attach(
                        rx.recv_timeout(Duration::from_secs(5)).expect("handoff"),
                        Token(0),
                    )
                    .expect("attach");
                l.read(h, ReadBuf::Pooled, Token(1)).expect("read");
                let mut out = Completions::default();
                let until = Instant::now() + Duration::from_secs(5);
                let mut wrote = false;
                while !wrote {
                    assert!(Instant::now() < until);
                    l.turn(Timeout::Until(until), &mut out).expect("turn");
                    for c in out.drain() {
                        match c.result {
                            OpResult::Read { n, lease: Some(b) } => {
                                assert_eq!(n, 1);
                                l.write(h, WriteBuf::Owned(b.as_slice().to_vec()), Token(2))
                                    .expect("echo");
                            }
                            OpResult::Wrote(n) => {
                                assert_eq!(n, 1);
                                wrote = true;
                                completed += 1;
                            }
                            other => panic!("unexpected {other:?}"),
                        }
                    }
                }
                l.close(h, Token(3)).expect("close");
                l.turn(Timeout::Now, &mut out).expect("release");
                assert!(matches!(out[0].result, OpResult::Closed));
            }
            completed
        }));
    }
    let clients: Vec<_> = (0..CONNECTIONS)
        .map(|i| {
            thread::spawn(move || {
                let mut s = std::net::TcpStream::connect(addr).expect("client");
                s.set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("timeout");
                s.write_all(&[i as u8]).expect("write");
                let mut b = [0];
                s.read_exact(&mut b).expect("echo");
                assert_eq!(b, [i as u8]);
            })
        })
        .collect();
    let mut accepted = 0;
    let mut out = Completions::default();
    let until = Instant::now() + Duration::from_secs(5);
    while accepted < CONNECTIONS {
        assert!(Instant::now() < until);
        primary.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            if let OpResult::Accepted { conn, .. } = c.result {
                let d = primary.detach(conn).expect("detach accepted");
                assert!(senders[accepted % WORKERS].send(d).is_ok());
                accepted += 1;
            } else {
                panic!("unexpected accept completion");
            }
        }
    }
    for c in clients {
        c.join().expect("client bytes verified");
    }
    for w in workers {
        assert_eq!(w.join().expect("worker"), CONNECTIONS / WORKERS);
    }
    assert_eq!(accepted, CONNECTIONS);
}
