//! Multi-threaded accept scaling harness (DESIGN §5a.6).
//!
//! The question this exists to answer is whether **one** turnloop server can use
//! more than one core, and by which of the two routes §5a.6 offers:
//!
//! * `--route reuse-port`: N loops on N threads, each with its own listener on
//!   one port, and the kernel deciding which loop accepts each connection. Only
//!   where [`ReusePort::Distribute`] can be honoured — Linux, Android, FreeBSD.
//! * `--route handoff`: one accepting loop and N sibling loops on N threads,
//!   each connection moved with `detach`/`attach`. Available everywhere native,
//!   and on Windows it is the only route, because a socket joins exactly one
//!   completion port permanently.
//!
//! The workload is connection-oriented on purpose — accept, one request, one
//! response, close — because that is the shape where the accept path is the
//! thing under test rather than a rounding error.
//!
//! **This harness reports, it does not judge.** It prints one JSON line per run
//! plus the per-loop service counts, and it refuses to print anything at all
//! unless its own subject demonstrably ran: every loop must have bound a
//! listener (reuse-port) or adopted connections (handoff), and every loop must
//! have served at least one connection. A scaling number from a run where three
//! of four loops sat idle is worse than no number.
use crate::counter::Counter;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use turnloop::*;

/// Which of DESIGN §5a.6's two routes to exercise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Route {
    /// Every loop binds the port; the kernel picks the accepting loop.
    ReusePort,
    /// One loop accepts and hands each connection to a sibling loop.
    Handoff,
}

/// One scaling run's configuration.
pub struct Args {
    /// Which route to exercise.
    pub route: Route,
    /// Serving loops, each on its own thread.
    pub loops: usize,
    /// Connections to drive through the server in total.
    pub connections: usize,
    /// Client threads generating the load.
    pub clients: usize,
    /// Request and response size in bytes.
    pub payload: usize,
    /// Handle ceiling per serving loop.
    ///
    /// Generous by default: turnloop#77 is open, so a multishot accept can
    /// outrun a tight ceiling within one turn, and a harness that hit that
    /// would be measuring the open issue.
    pub max_handles: usize,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            route: Route::Handoff,
            loops: 4,
            connections: 20_000,
            clients: 8,
            payload: 64,
            max_handles: 4096,
        }
    }
}

impl Args {
    /// Parse the harness flags out of a command line.
    ///
    /// Unrecognised arguments are ignored: this shares a binary with the timer
    /// and instruction harnesses.
    pub fn parse<I: Iterator<Item = String>>(args: I) -> Self {
        let argv: Vec<String> = args.collect();
        let mut out = Self::default();
        let value = |name: &str| -> Option<String> {
            let i = argv.iter().position(|a| a == name)?;
            argv.get(i + 1).cloned()
        };
        let number = |name: &str, current: usize| -> usize {
            value(name).map_or(current, |v| {
                v.parse()
                    .unwrap_or_else(|_| panic!("{name} takes a number"))
            })
        };
        if let Some(r) = value("--route") {
            out.route = match r.as_str() {
                "reuse-port" => Route::ReusePort,
                "handoff" => Route::Handoff,
                other => panic!("--route is reuse-port or handoff, not {other}"),
            };
        }
        out.loops = number("--loops", out.loops).max(1);
        out.connections = number("--connections", out.connections);
        out.clients = number("--clients", out.clients).max(1);
        out.payload = number("--payload", out.payload).max(1);
        out.max_handles = number("--max-handles", out.max_handles);
        out
    }

    fn config(&self) -> Config {
        Config {
            max_handles: self.max_handles,
            max_operations: self.max_handles * 4,
            ..Config::default()
        }
    }
}

/// A serving loop: accept or adopt connections, echo one payload each, close.
struct Server {
    slots: Vec<Option<Handle>>,
    payload: usize,
    served: usize,
}

impl Server {
    fn new(capacity: usize, payload: usize) -> Self {
        Self {
            slots: vec![None; capacity],
            payload,
            served: 0,
        }
    }
    /// Begin serving a connection this loop now owns.
    fn adopt(&mut self, l: &mut Loop, h: Handle) {
        let i = h.index();
        assert!(self.slots[i].is_none(), "slot {i} reused while live");
        self.slots[i] = Some(h);
        l.read(h, ReadBuf::Pooled, Token(i as u64)).expect("read");
    }
    /// Advance one completion. Returns true when a connection finished.
    fn step(&mut self, l: &mut Loop, c: Completion) -> bool {
        let i = c.token.0 as usize;
        match c.result {
            OpResult::Read { n, lease: Some(b) } => {
                let h = self.slots[i].expect("live slot");
                assert!(n > 0);
                // Echo exactly what arrived; a short read re-arms rather than
                // answering a partial request.
                if n < self.payload {
                    l.read(h, ReadBuf::Pooled, Token(i as u64)).expect("more");
                } else {
                    l.write(h, WriteBuf::Owned(b.as_slice().to_vec()), Token(i as u64))
                        .expect("echo");
                }
                false
            }
            OpResult::Wrote(_) => {
                let h = self.slots[i].expect("live slot");
                l.close(h, Token(i as u64)).expect("close");
                false
            }
            OpResult::Closed => {
                self.slots[i] = None;
                self.served += 1;
                true
            }
            OpResult::Eof => {
                let h = self.slots[i].expect("live slot");
                l.close(h, Token(i as u64)).expect("close");
                false
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Token reserved for the listener's own multishot accept.
const ACCEPT: Token = Token(u64::MAX);

/// Connect, send one payload, read it back, close. Repeated by each client
/// thread until the run's connection budget is spent.
///
/// `issued` is a ticket counter rather than a countdown: every thread takes the
/// next number and stops once the numbers run past the budget, so the work is
/// claimed exactly once with a single `fetch_add` and no compare-exchange loop.
fn client(addr: SocketAddr, payload: usize, issued: &AtomicUsize, budget: usize) -> usize {
    let request = vec![0x5a_u8; payload];
    let mut response = vec![0_u8; payload];
    let mut done = 0;
    while issued.fetch_add(1, Ordering::AcqRel) < budget {
        let mut s = TcpStream::connect(addr).expect("connect");
        s.set_nodelay(true).expect("nodelay");
        s.set_read_timeout(Some(Duration::from_secs(30)))
            .expect("timeout");
        s.write_all(&request).expect("request");
        s.read_exact(&mut response).expect("response");
        assert_eq!(response, request, "wrong bytes came back");
        done += 1;
    }
    done
}

/// Run one scaling configuration and print its result.
pub fn run(counter: &Counter, args: &Args) {
    assert!(
        args.connections >= 100 * args.loops,
        "--connections must be at least 100 per loop, or a loop can be given \
         nothing by chance and the run proves nothing"
    );
    // Refuse a route this platform cannot take, before any thread is started,
    // so the operator gets one line and a status rather than a panic from
    // inside a worker and a confusing rendezvous failure behind it.
    if args.route == Route::ReusePort {
        let mut probe = Loop::new(Config::default()).expect("probe loop");
        if let Err(e) = probe.tcp_listen(
            ([127, 0, 0, 1], 0).into(),
            &ListenOpts {
                reuse_port: ReusePort::Distribute,
                ..ListenOpts::default()
            },
        ) {
            eprintln!(
                "--route reuse-port is unavailable here: ReusePort::Distribute is {:?}. Only Linux, Android and FreeBSD distribute accepts in the kernel; on this platform use --route handoff, which is the supported multi-core route (DESIGN 5a.6).",
                e.kind
            );
            std::process::exit(2);
        }
    }
    let served = Arc::new(AtomicUsize::new(0));
    let issued = Arc::new(AtomicUsize::new(0));
    let (addr, threads) = match args.route {
        Route::ReusePort => reuse_port_servers(args, &served),
        Route::Handoff => handoff_servers(args, &served),
    };
    let before = counter.read().expect("counter");
    let start = Instant::now();
    let (payload, budget) = (args.payload, args.connections);
    let clients: Vec<_> = (0..args.clients)
        .map(|_| {
            let issued = Arc::clone(&issued);
            thread::spawn(move || client(addr, payload, &issued, budget))
        })
        .collect();
    let driven: usize = clients.into_iter().map(|c| c.join().expect("client")).sum();
    let elapsed = start.elapsed();
    let total = counter
        .read()
        .expect("counter")
        .checked_sub(before)
        .expect("monotonic counter");
    let per_loop: Vec<usize> = threads
        .into_iter()
        .map(|t| t.join().expect("loop"))
        .collect();

    // Liveness before reporting. A number from a run whose subject never
    // executed is the failure mode this project has paid for repeatedly.
    assert_eq!(driven, args.connections, "client budget not spent");
    assert_eq!(
        per_loop.iter().sum::<usize>(),
        args.connections,
        "connections served {per_loop:?} does not match the budget"
    );
    assert_eq!(per_loop.len(), args.loops, "not every loop reported");
    assert!(
        per_loop.iter().all(|n| *n > 0),
        "a loop served nothing, so this configuration did not run on {} cores: {per_loop:?}",
        args.loops
    );
    let route = match args.route {
        Route::ReusePort => "reuse_port",
        Route::Handoff => "handoff",
    };
    println!(
        "{{\"name\":\"accept_scaling_{route}_{}\",\"operations\":{},\"total\":{total},\
         \"per_operation\":{:.2},\"unit\":\"{}\",\"loops\":{},\"elapsed_ns\":{},\
         \"connections_per_second\":{:.0},\"per_loop\":{per_loop:?}}}",
        args.loops,
        args.connections,
        total as f64 / args.connections as f64,
        counter.unit(),
        args.loops,
        elapsed.as_nanos(),
        args.connections as f64 / elapsed.as_secs_f64(),
    );
}

/// N loops, N threads, one listener each on a shared port.
fn reuse_port_servers(
    args: &Args,
    served: &Arc<AtomicUsize>,
) -> (SocketAddr, Vec<thread::JoinHandle<usize>>) {
    let opts = ListenOpts {
        reuse_port: ReusePort::Distribute,
        accept_defaults: AcceptDefaults {
            nodelay: true,
            keep_alive: None,
        },
        ..ListenOpts::default()
    };
    let config = args.config();
    let (payload, budget, loops) = (args.payload, args.connections, args.loops);
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let spawn = |bind: Option<SocketAddr>,
                 addr_tx: mpsc::Sender<SocketAddr>,
                 ready_tx: mpsc::Sender<()>,
                 served: Arc<AtomicUsize>| {
        thread::spawn(move || {
            let mut l = Loop::new(config).expect("loop");
            let listener = l
                .tcp_listen(
                    bind.unwrap_or_else(|| ([127, 0, 0, 1], 0).into()),
                    &opts,
                )
                .unwrap_or_else(|e| {
                    panic!("this platform cannot honour ReusePort::Distribute ({e:?}); use --route handoff")
                });
            if bind.is_none() {
                addr_tx
                    .send(l.local_addr(listener).expect("addr"))
                    .expect("addr");
            }
            drop((addr_tx, ready_tx));
            l.accept_start(listener, ACCEPT).expect("accept_start");
            let mut server = Server::new(config.max_handles, payload);
            let mut out = Completions::default();
            while served.load(Ordering::Acquire) < budget {
                l.turn(Timeout::After(Duration::from_millis(1)), &mut out)
                    .expect("turn");
                for c in out.drain() {
                    if c.token == ACCEPT {
                        let OpResult::Accepted { conn, .. } = c.result else {
                            panic!("unexpected {:?}", c.result);
                        };
                        server.adopt(&mut l, conn);
                    } else if server.step(&mut l, c) {
                        served.fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
            server.served
        })
    };
    let mut threads = vec![spawn(
        None,
        addr_tx.clone(),
        ready_tx.clone(),
        Arc::clone(served),
    )];
    let addr = addr_rx.recv().expect("first listener published its port");
    for _ in 1..loops {
        threads.push(spawn(
            Some(addr),
            addr_tx.clone(),
            ready_tx.clone(),
            Arc::clone(served),
        ));
    }
    drop((addr_tx, ready_tx));
    // Every listener must hold the port before the first client connects, or
    // the early connections could only ever reach the loops that had bound.
    while ready_rx.recv().is_ok() {}
    (addr, threads)
}

/// One accepting loop plus N sibling loops adopting detached connections.
fn handoff_servers(
    args: &Args,
    served: &Arc<AtomicUsize>,
) -> (SocketAddr, Vec<thread::JoinHandle<usize>>) {
    let config = args.config();
    let (payload, budget, loops) = (args.payload, args.connections, args.loops);
    let mut senders = Vec::new();
    let mut threads = Vec::new();
    let mut notifiers = Vec::new();
    let (ready_tx, ready_rx) = mpsc::channel::<Notifier>();
    for _ in 0..loops {
        let (tx, rx) = mpsc::channel::<Detached>();
        senders.push(tx);
        let served = Arc::clone(served);
        let ready_tx = ready_tx.clone();
        threads.push(thread::spawn(move || {
            let mut l = Loop::new(config).expect("worker loop");
            ready_tx.send(l.notifier()).expect("publish notifier");
            drop(ready_tx);
            let mut server = Server::new(config.max_handles, payload);
            let mut out = Completions::default();
            while served.load(Ordering::Acquire) < budget {
                // The acceptor notifies after sending, so a parked worker wakes
                // on a handoff rather than polling for one.
                while let Ok(d) = rx.try_recv() {
                    let h = l.attach(d, Token(0)).expect("attach");
                    server.adopt(&mut l, h);
                }
                l.turn(Timeout::After(Duration::from_millis(1)), &mut out)
                    .expect("turn");
                for c in out.drain() {
                    if server.step(&mut l, c) {
                        served.fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
            server.served
        }));
    }
    drop(ready_tx);
    for _ in 0..loops {
        notifiers.push(ready_rx.recv().expect("worker notifier"));
    }
    let (addr_tx, addr_rx) = mpsc::channel::<SocketAddr>();
    let acceptor_served = Arc::clone(served);
    thread::spawn(move || {
        let mut l = Loop::new(config).expect("acceptor");
        let listener = l
            .tcp_listen(
                ([127, 0, 0, 1], 0).into(),
                &ListenOpts {
                    accept_defaults: AcceptDefaults {
                        nodelay: true,
                        keep_alive: None,
                    },
                    ..ListenOpts::default()
                },
            )
            .expect("listen");
        addr_tx
            .send(l.local_addr(listener).expect("addr"))
            .expect("addr");
        drop(addr_tx);
        l.accept_start(listener, ACCEPT).expect("accept_start");
        let mut out = Completions::default();
        let mut next = 0;
        while acceptor_served.load(Ordering::Acquire) < budget {
            l.turn(Timeout::After(Duration::from_millis(1)), &mut out)
                .expect("turn");
            for c in out.drain() {
                let OpResult::Accepted { conn, .. } = c.result else {
                    panic!("unexpected {:?}", c.result);
                };
                // Round-robin. DESIGN §5a.6 puts the policy in the host, and
                // this is the simplest one a host could write; least-loaded
                // would be the other obvious choice and would need the workers
                // to publish their depth.
                let d = l.detach(conn).expect("detach");
                if senders[next % loops].send(d).is_ok() {
                    let _ = notifiers[next % loops].notify();
                }
                next += 1;
            }
        }
    });
    let addr = addr_rx.recv().expect("acceptor published its port");
    (addr, threads)
}
