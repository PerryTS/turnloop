use super::*;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
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
        let until = l.now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        let mut wrote = 0;
        while wrote == 0 {
            assert!(l.now() < until);
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
    let until = source.now() + Duration::from_secs(3);
    while bytes.len() < 4 {
        assert!(source.now() < until);
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
    let until = l.now() + Duration::from_secs(5);
    let mut seen = [false; 3];
    let mut out = Completions::default();
    while !seen.iter().all(|v| *v) {
        assert!(l.now() < until);
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
/// Whether this target's kernel can distribute accepts across listeners, which
/// is what [`ReusePort::Distribute`] promises. Keep in step with
/// `backend::socket::reuse_port_option`.
pub const DISTRIBUTES: bool = cfg!(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd"
));

/// Bind two listeners to one port with `reuse` and count what each one accepts.
///
/// Returns `None` if the *second* bind was refused, which is how a backend
/// without the option reports itself.
fn two_listeners<B: Backend>(reuse: ReusePort, connections: usize) -> Option<[usize; 2]> {
    let mut a = Driver::<B>::new(Config::default()).expect("loop a");
    let mut b = Driver::<B>::new(Config::default()).expect("loop b");
    let opts = ListenOpts {
        reuse_port: reuse,
        ..ListenOpts::default()
    };
    let ah = a.tcp_listen(localhost(), &opts).ok()?;
    let addr = a.local_addr(ah).expect("addr");
    let bh = b
        .tcp_listen(addr, &opts)
        .expect("second bind of a reused port");
    a.accept_start(ah, Token(1)).expect("accept a");
    b.accept_start(bh, Token(2)).expect("accept b");
    let clients: Vec<_> = (0..connections)
        .map(|_| std::net::TcpStream::connect(addr).expect("connect"))
        .collect();
    let mut count = [0usize; 2];
    let mut out = Completions::default();
    let until = a.now() + Duration::from_secs(10);
    while count.iter().sum::<usize>() < clients.len() {
        assert!(a.now() < until, "only {count:?} of {connections} accepted");
        for l in [&mut a, &mut b] {
            l.turn(Timeout::Now, &mut out).expect("turn");
            for c in out.drain() {
                assert!(matches!(c.result, OpResult::Accepted { .. }));
                count[c.token.0 as usize - 1] += 1;
            }
        }
    }
    Some(count)
}

/// [`ReusePort::Share`] permits the duplicate bind and promises nothing else.
///
/// Every connection must still be accepted by *someone*, because both sockets
/// hold the address. Which one is deliberately not asserted: on Linux the kernel
/// spreads them and on macOS the last binder takes all 32, and `Share` is
/// honest about covering both.
pub fn reuse_port_share<B: Backend>() {
    let Some(count) = two_listeners::<B>(ReusePort::Share, 32) else {
        // No SO_REUSEPORT on this backend at all; reuse_port_refused covers it.
        return;
    };
    assert_eq!(count.iter().sum::<usize>(), 32, "share {count:?}");
    eprintln!("reuse-port Share accepts: {count:?}");
}

/// [`ReusePort::Distribute`] either distributes or refuses the listener.
///
/// This is the gate that makes the option honest. There is no third outcome: a
/// backend may not accept the request and then leave a listener starved. The
/// starved case is real and is what this exists to prevent — two loops sharing
/// one port under plain `SO_REUSEPORT` on macOS 15 split 32 connections
/// `[0, 32]`, so a host that developed against Linux would ship a server whose
/// first loop never accepts anything.
pub fn reuse_port_distribute<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let attempt = l.tcp_listen(
        localhost(),
        &ListenOpts {
            reuse_port: ReusePort::Distribute,
            ..ListenOpts::default()
        },
    );
    if !DISTRIBUTES {
        assert!(
            matches!(
                attempt,
                Err(Error {
                    kind: ErrorKind::Unsupported,
                    ..
                })
            ),
            "a platform that cannot distribute must refuse, got {attempt:?}"
        );
        assert!(!l.alive(), "a refused listener leaves nothing behind");
        return;
    }
    assert!(attempt.is_ok(), "this platform distributes: {attempt:?}");
    drop(l);
    let count = two_listeners::<B>(ReusePort::Distribute, 64).expect("distribute binds");
    assert_eq!(count.iter().sum::<usize>(), 64, "distribute {count:?}");
    assert!(
        count.iter().all(|n| *n > 0),
        "every listener must be given work: {count:?}"
    );
    eprintln!("reuse-port Distribute accepts: {count:?}");
}

/// Backends with no address-reuse mechanism refuse both requests outright.
///
/// A silent no-op is the failure mode this rejects: `Unsupported` at listen time
/// is recoverable, a listener that never accepts is not diagnosable.
pub fn reuse_port_refused<B: Backend>(expected: &[ReusePort]) {
    for &reuse in expected {
        let mut l = Driver::<B>::new(Config::default()).expect("loop");
        let attempt = l.tcp_listen(
            localhost(),
            &ListenOpts {
                reuse_port: reuse,
                ..ListenOpts::default()
            },
        );
        assert!(
            matches!(
                attempt,
                Err(Error {
                    kind: ErrorKind::Unsupported,
                    ..
                })
            ),
            "{reuse:?} must be refused, got {attempt:?}"
        );
        assert!(!l.alive());
    }
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
                let until = l.now() + Duration::from_secs(5);
                let mut wrote = false;
                while !wrote {
                    assert!(l.now() < until);
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
    let until = primary.now() + Duration::from_secs(5);
    while accepted < CONNECTIONS {
        assert!(primary.now() < until);
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

/// A shared release point for fixture jobs, so a held job parks instead of
/// polling: the tests below hold more threads than a machine has cores.
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            changed: Condvar::new(),
        })
    }
    fn wait(&self) {
        let mut open = self.open.lock().expect("gate");
        while !*open {
            open = self.changed.wait(open).expect("gate");
        }
    }
    fn release(&self) {
        *self.open.lock().expect("gate") = true;
        self.changed.notify_all();
    }
    fn released(&self) -> bool {
        *self.open.lock().expect("gate")
    }
}
#[track_caller]
fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready() {
        assert!(Instant::now() < deadline, "{what}");
        thread::yield_now();
    }
}
/// Collect exactly one completion for `op`, then prove nothing follows it.
#[track_caller]
fn settle<B: Backend>(l: &mut Driver<B>, op: OpId, out: &mut Completions) -> OpResult {
    let until = l.now() + Duration::from_secs(30);
    let mut result = None;
    while result.is_none() {
        assert!(l.now() < until, "the job never completed");
        l.turn(Timeout::Until(until), out).expect("turn");
        for c in out.drain() {
            assert_eq!(c.op, Some(op), "an unexpected completion arrived");
            assert!(c.terminal);
            assert!(result.replace(c.result).is_none(), "delivered twice");
        }
    }
    for _ in 0..5 {
        l.turn(Timeout::Now, out).expect("drain");
        assert!(out.is_empty(), "a second completion followed the first");
    }
    result.expect("settled")
}
/// Neither occupancy class can exhaust the other (issue #42).
///
/// Both halves turn on an **ordering** assertion rather than a deadline: the
/// starved class's job must complete while the saturating class is still held.
/// A slow machine cannot satisfy that by accident, and a pool with one class
/// cannot satisfy it at all — 32 connection-lifetime jobs on a four-thread pool
/// is precisely the deadlock this class exists to remove.
pub fn occupancy_classes_do_not_starve_each_other<B: Backend>() {
    let pool = Config::default().blocking_pool;
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    // A quiet baseline: another test's worker may still have been retiring.
    until("the shared pool is quiet", || {
        let s = pool_stats();
        s.busy == 0 && s.queued == 0 && s.reserved == 0
    });

    // Direction 1: a saturated long class refuses and delays no bounded work.
    let gate = Gate::new();
    let held = pool.threads * 8;
    let entered = Arc::new(AtomicUsize::new(0));
    let long: Vec<OpId> = (0..held)
        .map(|i| {
            let (gate, entered) = (gate.clone(), entered.clone());
            l.blocking_with(
                move |_| {
                    entered.fetch_add(1, Ordering::Release);
                    gate.wait();
                    Ok(Payload::U64(i as u64))
                },
                Occupancy::Long,
                Token(1000 + i as u64),
            )
            .expect("a long job is accepted past the bounded set's size")
        })
        .collect();
    // Every one is *running*: the class hands out threads rather than queueing,
    // so this cannot be reached unless `held` threads exist at once.
    until("every long job started", || {
        entered.load(Ordering::Acquire) == held
    });
    let stats = pool_stats();
    // Simultaneously busy is the load-bearing one: it can only be reached with
    // `held` distinct threads, so it is what says the class really grew.
    assert_eq!(stats.long_busy, held, "each holds a long worker of its own");
    assert!(
        stats.long_threads >= held,
        "at least that many long workers"
    );
    assert_eq!(stats.busy, 0, "no long job occupies a bounded worker");
    assert_eq!(stats.queued, 0, "no long job entered the bounded queue");
    assert_eq!(stats.reserved, 0, "no long job took a queue reservation");
    assert_eq!(stats.threads, pool.threads, "the bounded set is intact");

    let short = l
        .blocking(|| Ok(Payload::U64(7)), Token(1))
        .expect("bounded work is still accepted");
    assert!(
        matches!(
            settle(&mut l, short, &mut out),
            OpResult::Blocking(Payload::U64(7))
        ),
        "a bounded job ran while eight times the bounded set was held long"
    );
    assert!(
        !gate.released(),
        "it finished before anything released the long jobs"
    );

    gate.release();
    let mut settled = 0;
    let deadline = l.now() + Duration::from_secs(30);
    while settled < held {
        assert!(l.now() < deadline, "released long jobs never completed");
        l.turn(Timeout::Until(deadline), &mut out).expect("turn");
        for c in out.drain() {
            assert!(long.contains(&c.op.expect("job completion")));
            assert!(matches!(c.result, OpResult::Blocking(Payload::U64(_))));
            settled += 1;
        }
    }
    assert_eq!(
        settled, held,
        "every accepted long job settled exactly once"
    );

    // Direction 2: a saturated bounded class refuses and delays no long work.
    let gate = Gate::new();
    let running = Arc::new(AtomicUsize::new(0));
    let holders: Vec<OpId> = (0..pool.threads)
        .map(|_| {
            let (gate, running) = (gate.clone(), running.clone());
            l.blocking(
                move || {
                    running.fetch_add(1, Ordering::Release);
                    gate.wait();
                    Ok(Payload::U64(0))
                },
                Token(2),
            )
            .expect("bounded holder")
        })
        .collect();
    until("every bounded worker is occupied", || {
        running.load(Ordering::Acquire) == pool.threads
    });
    let mut queued = Vec::new();
    let refused = loop {
        match l.blocking(|| Ok(Payload::U64(0)), Token(3)) {
            Ok(op) => queued.push(op),
            Err(e) => break e,
        }
        assert!(queued.len() <= pool.queue_capacity, "queue bound enforced");
    };
    assert_eq!(refused.kind, ErrorKind::ResourceLimit);
    assert_eq!(queued.len(), pool.queue_capacity, "the queue is full");
    assert_eq!(pool_stats().queued, pool.queue_capacity);

    let long = l
        .blocking_with(|_| Ok(Payload::U64(9)), Occupancy::Long, Token(4))
        .expect("a long job is accepted while the bounded class refuses");
    assert!(matches!(
        settle(&mut l, long, &mut out),
        OpResult::Blocking(Payload::U64(9))
    ));
    assert!(
        l.blocking(|| Ok(Payload::U64(0)), Token(3)).is_err(),
        "the bounded class was still saturated when the long job finished"
    );
    assert!(!gate.released(), "nothing released the bounded holders");

    gate.release();
    let mut settled = 0;
    let expected = holders.len() + queued.len();
    let deadline = l.now() + Duration::from_secs(60);
    while settled < expected {
        assert!(l.now() < deadline, "released bounded jobs never completed");
        l.turn(Timeout::Until(deadline), &mut out).expect("turn");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::Blocking(Payload::U64(0))));
            settled += 1;
        }
    }
    assert_eq!(settled, expected, "every accepted bounded job settled once");
    assert!(!l.alive());
}
/// A long job delivers exactly one completion on every path that is not its own
/// return: cancellation, a panic, and the loop going away underneath it.
pub fn long_jobs_settle_once_on_cancel_panic_and_shutdown<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut out = Completions::default();

    // A running long job stops because it was asked to, not because it was
    // interrupted: nothing can take a thread back from a job that never looks.
    let started = Arc::new(AtomicBool::new(false));
    let observed = Arc::new(AtomicBool::new(false));
    let (running, stopped) = (started.clone(), observed.clone());
    let op = l
        .blocking_with(
            move |stop| {
                running.store(true, Ordering::Release);
                while !stop.requested() {
                    thread::park_timeout(Duration::from_millis(1));
                }
                stopped.store(true, Ordering::Release);
                Ok(Payload::U64(1))
            },
            Occupancy::Long,
            Token(1),
        )
        .expect("long job");
    until("the long job is running", || {
        started.load(Ordering::Acquire)
    });
    assert!(l.cancel(op));
    assert!(!l.cancel(op), "a second cancel finds it already cancelled");
    assert!(matches!(settle(&mut l, op, &mut out), OpResult::Cancelled));
    assert!(
        observed.load(Ordering::Acquire),
        "the job returned of its own accord after seeing the request"
    );

    // A panicking long job is reported to its host and leaves the class usable.
    // The hook is silenced so an expected panic does not read as a CI failure.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let op = l
        .blocking_with(
            |_| panic!("a long job may panic"),
            Occupancy::Long,
            Token(2),
        )
        .expect("long job");
    let result = settle(&mut l, op, &mut out);
    std::panic::set_hook(hook);
    assert!(
        matches!(
            result,
            OpResult::Err(Error {
                kind: ErrorKind::Other,
                ..
            })
        ),
        "unexpected {result:?}"
    );
    let op = l
        .blocking_with(|_| Ok(Payload::U64(3)), Occupancy::Long, Token(3))
        .expect("the long class still serves work");
    assert!(matches!(
        settle(&mut l, op, &mut out),
        OpResult::Blocking(Payload::U64(3))
    ));

    // A dropped loop asks its outstanding long jobs to stop. Without that, a
    // connection-lifetime job would hold its thread for the process's life and
    // wait forever for a host that no longer exists.
    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (running, exited) = (started.clone(), finished.clone());
    let mut doomed = Driver::<B>::new(Config::default()).expect("second loop");
    doomed
        .blocking_with(
            move |stop| {
                running.store(true, Ordering::Release);
                while !stop.requested() {
                    thread::park_timeout(Duration::from_millis(1));
                }
                exited.store(true, Ordering::Release);
                Ok(Payload::U64(4))
            },
            Occupancy::Long,
            Token(4),
        )
        .expect("long job");
    until("the long job is running", || {
        started.load(Ordering::Acquire)
    });
    assert!(doomed.alive(), "an accepted job keeps its loop alive");
    drop(doomed);
    until("shutdown released the long worker", || {
        finished.load(Ordering::Acquire)
    });
    until("the worker rejoined the idle set", || {
        pool_stats().long_busy == 0
    });
}

// ---------------------------------------------------------------------------
// DESIGN §5a.6: multi-threaded accept
// ---------------------------------------------------------------------------

/// Connections served by one multi-threaded-accept scenario.
const MT_CONNECTIONS: usize = 64;
/// Loops, each on its own thread, sharing the port.
const MT_LOOPS: usize = 4;
/// Handle ceiling for every loop that serves connections.
///
/// Far below `MT_CONNECTIONS`, and that is the point: each loop serves its
/// connections one at a time and returns the handle, so the workload fits
/// comfortably — but a handle leaked per connection, at accept, at `attach`, at
/// `detach` or at `close`, exhausts the ceiling long before the run ends and
/// turns an invisible orphan into a `ResourceLimit` failure.
const MT_CEILING: usize = 8;

fn mt_config() -> Config {
    Config {
        max_handles: MT_CEILING,
        max_operations: 64,
        ..Config::default()
    }
}

/// Read one connection's two-byte id, echo it, close the handle and drain the
/// close. Returns the id, so a caller can prove which connection this was.
fn serve_one<B: Backend>(l: &mut Driver<B>, h: Handle) -> u16 {
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(30);
    let mut got: Vec<u8> = Vec::new();
    let mut echoed = false;
    l.read(h, ReadBuf::Pooled, Token(1)).expect("read");
    while !echoed {
        assert!(l.now() < until, "connection stalled with {got:?}");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease: Some(b) } => {
                    assert_eq!(n, b.as_slice().len());
                    got.extend_from_slice(b.as_slice());
                    if got.len() < 2 {
                        l.read(h, ReadBuf::Pooled, Token(1)).expect("read more");
                    } else {
                        l.write(h, WriteBuf::Owned(got.clone()), Token(2))
                            .expect("echo");
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, got.len());
                    echoed = true;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    l.close(h, Token(3)).expect("close");
    let mut closed = false;
    while !closed {
        assert!(l.now() < until, "close never completed");
        l.turn(Timeout::Until(until), &mut out).expect("release");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::Closed), "{:?}", c.result);
            closed = true;
        }
    }
    assert_eq!(got.len(), 2, "one id per connection");
    u16::from_le_bytes([got[0], got[1]])
}

/// Connect, send a distinct id, and require that exact id back.
///
/// The echo is what makes "served exactly once" checkable from the outside: a
/// connection dropped at handoff never answers and this fails on the read.
fn mt_clients(addr: SocketAddr, count: usize) -> Vec<thread::JoinHandle<()>> {
    (0..count)
        .map(|i| {
            thread::spawn(move || {
                use std::io::{Read, Write};
                let mut s = std::net::TcpStream::connect(addr).expect("connect");
                s.set_read_timeout(Some(Duration::from_secs(30)))
                    .expect("timeout");
                s.write_all(&(i as u16).to_le_bytes()).expect("send id");
                let mut b = [0u8; 2];
                s.read_exact(&mut b).expect("echo");
                assert_eq!(u16::from_le_bytes(b), i as u16, "wrong connection answered");
            })
        })
        .collect()
}

/// Every id was served, once, by someone.
#[track_caller]
fn mt_verify(mut served: Vec<u16>, per_loop: &[usize], route: &str) {
    let total: usize = per_loop.iter().sum();
    assert_eq!(total, served.len(), "{route}: counts disagree with ids");
    served.sort_unstable();
    let expected: Vec<u16> = (0..MT_CONNECTIONS as u16).collect();
    assert_eq!(
        served, expected,
        "{route}: every connection exactly once, none lost or duplicated"
    );
    eprintln!("{route}: per-loop {per_loop:?}");
}

/// One accepting loop, `MT_LOOPS` sibling loops on their own threads, each
/// connection handed over with `detach`/`attach` and then driven to completion
/// on the loop that adopted it.
///
/// This is DESIGN §5a.6's "everywhere else" route, and on Windows it is the only
/// one: a socket joins exactly one completion port permanently, so a second loop
/// can never be given the same listener.
pub fn handoff_accept_exactly_once<B: Backend>() {
    let mut acceptor = Driver::<B>::new(mt_config()).expect("acceptor");
    let listener = acceptor
        .tcp_listen(localhost(), &ListenOpts::default())
        .expect("listen");
    let addr = acceptor.local_addr(listener).expect("addr");
    let mut senders = Vec::new();
    let mut workers = Vec::new();
    for _ in 0..MT_LOOPS {
        let (tx, rx) = std::sync::mpsc::channel::<B::Detached>();
        senders.push(tx);
        workers.push(thread::spawn(move || {
            let mut l = Driver::<B>::new(mt_config()).expect("worker loop");
            let mut served = Vec::new();
            // The acceptor drops its senders when the last connection is gone,
            // which is this worker's only stop signal.
            while let Ok(d) = rx.recv_timeout(Duration::from_secs(30)) {
                let h = l.attach(d, Token(0)).expect("attach");
                served.push(serve_one(&mut l, h));
            }
            assert!(!l.alive(), "worker loop still alive at shutdown");
            served
        }));
    }
    let clients = mt_clients(addr, MT_CONNECTIONS);
    // Single-shot accept, re-armed per connection. turnloop#77 is open: a
    // multishot accept can outrun the handle ceiling within one turn, and this
    // loop deliberately runs at a low ceiling, so depending on multishot here
    // would be testing that open issue rather than the handoff.
    acceptor.accept(listener, Token(0)).expect("accept");
    let mut handed = 0;
    let mut out = Completions::default();
    let until = acceptor.now() + Duration::from_secs(60);
    while handed < MT_CONNECTIONS {
        assert!(acceptor.now() < until, "only {handed} accepted");
        acceptor
            .turn(Timeout::Until(until), &mut out)
            .expect("turn");
        for c in out.drain() {
            let OpResult::Accepted { conn, .. } = c.result else {
                panic!("unexpected {:?}", c.result);
            };
            let d = acceptor.detach(conn).expect("detach accepted connection");
            senders[handed % MT_LOOPS].send(d).expect("worker alive");
            handed += 1;
            if handed < MT_CONNECTIONS {
                acceptor.accept(listener, Token(0)).expect("re-arm");
            }
        }
    }
    drop(senders);
    for c in clients {
        c.join().expect("client verified its own id came back");
    }
    let mut served = Vec::new();
    let mut per_loop = Vec::new();
    for w in workers {
        let ids = w.join().expect("worker");
        per_loop.push(ids.len());
        served.extend(ids);
    }
    mt_verify(served, &per_loop, "handoff");
    acceptor.close(listener, Token(9)).expect("close listener");
    acceptor.turn(Timeout::Now, &mut out).expect("release");
    assert!(matches!(out[0].result, OpResult::Closed));
    assert!(!acceptor.alive(), "acceptor still alive at shutdown");
}

/// `MT_LOOPS` loops on `MT_LOOPS` threads, each with its own listener on one
/// shared port, with the kernel choosing which loop accepts each connection.
///
/// This is DESIGN §5a.6's kernel-balanced route. It runs only where
/// [`ReusePort::Distribute`] can be honoured; elsewhere the listener is refused
/// and [`reuse_port_distribute`] is the test that proves it.
///
/// The share each loop receives is deliberately **not** asserted. Distribution
/// is by 4-tuple hash, so shares are uneven by nature and asserting evenness
/// would be asserting something the kernel never promised. What is asserted is
/// the invariant that matters: every connection served exactly once.
pub fn kernel_accept_exactly_once<B: Backend>() {
    if !DISTRIBUTES {
        return;
    }
    let opts = ListenOpts {
        reuse_port: ReusePort::Distribute,
        ..ListenOpts::default()
    };
    let done = Arc::new(AtomicUsize::new(0));
    let (addr_tx, addr_rx) = std::sync::mpsc::channel::<SocketAddr>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let spawn = |bind: Option<SocketAddr>,
                 addr_tx: std::sync::mpsc::Sender<SocketAddr>,
                 ready_tx: std::sync::mpsc::Sender<()>,
                 done: Arc<AtomicUsize>| {
        thread::spawn(move || {
            let mut l = Driver::<B>::new(mt_config()).expect("worker loop");
            let listener = l
                .tcp_listen(bind.unwrap_or_else(localhost), &opts)
                .expect("shared-port listen");
            if bind.is_none() {
                addr_tx
                    .send(l.local_addr(listener).expect("addr"))
                    .expect("publish addr");
            }
            drop(addr_tx);
            ready_tx.send(()).expect("announce bound");
            drop(ready_tx);
            let mut served = Vec::new();
            let mut out = Completions::default();
            let until = l.now() + Duration::from_secs(60);
            // Stop only once every connection has been served by someone: this
            // loop cannot know its own share in advance, because the kernel
            // decides it.
            while done.load(Ordering::Acquire) < MT_CONNECTIONS {
                // Single-shot, for the turnloop#77 reason in the handoff route.
                let op = l.accept(listener, Token(0)).expect("accept");
                let mut conn = None;
                while conn.is_none() {
                    assert!(l.now() < until, "accept stalled");
                    if done.load(Ordering::Acquire) >= MT_CONNECTIONS {
                        break;
                    }
                    l.turn(Timeout::After(Duration::from_millis(2)), &mut out)
                        .expect("turn");
                    for c in out.drain() {
                        match c.result {
                            OpResult::Accepted { conn: h, .. } => conn = Some(h),
                            OpResult::Cancelled => {}
                            other => panic!("unexpected {other:?}"),
                        }
                    }
                }
                let Some(h) = conn else {
                    l.cancel(op);
                    break;
                };
                served.push(serve_one(&mut l, h));
                done.fetch_add(1, Ordering::AcqRel);
            }
            // Drain whatever the final cancelled accept left behind.
            let mut out = Completions::default();
            l.close(listener, Token(9)).expect("close listener");
            let until = l.now() + Duration::from_secs(10);
            while l.alive() {
                assert!(l.now() < until, "listener never released");
                l.turn(Timeout::Until(until), &mut out).expect("release");
                for c in out.drain() {
                    assert!(
                        matches!(c.result, OpResult::Closed | OpResult::Cancelled),
                        "{:?}",
                        c.result
                    );
                }
            }
            assert!(!l.alive(), "worker loop still alive at shutdown");
            served
        })
    };
    let mut workers = vec![spawn(
        None,
        addr_tx.clone(),
        ready_tx.clone(),
        Arc::clone(&done),
    )];
    let addr = addr_rx.recv().expect("first listener published its port");
    for _ in 1..MT_LOOPS {
        workers.push(spawn(
            Some(addr),
            addr_tx.clone(),
            ready_tx.clone(),
            Arc::clone(&done),
        ));
    }
    drop((addr_tx, ready_tx));
    // No client may connect before every listener holds the port, or the early
    // connections could only ever reach the loops that had bound.
    for _ in 0..MT_LOOPS {
        ready_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("all listeners bound");
    }
    let clients = mt_clients(addr, MT_CONNECTIONS);
    for c in clients {
        c.join().expect("client verified its own id came back");
    }
    let mut served = Vec::new();
    let mut per_loop = Vec::new();
    for w in workers {
        let ids = w.join().expect("worker");
        per_loop.push(ids.len());
        served.extend(ids);
    }
    mt_verify(served, &per_loop, "reuse-port");
}
