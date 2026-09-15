//! Socket options on live handles (DESIGN §7.7; issue #34).
//!
//! Every assertion here reads the value back **from the operating system**:
//! `get_option` is a `getsockopt`/`wasi:sockets` call on the live socket, never a
//! cache of what `set_option` was given, so a backend that accepted an option and
//! ignored it fails these tests. Two of them go further and prove behaviour the
//! host can observe: `linger_zero_resets_the_connection` makes a peer's read fail
//! with `ConnectionReset` where a graceful close would have produced `Eof`, and
//! `multicast_membership_is_tracked` shows the kernel refusing to leave a group it
//! was never asked to join.
use super::*;
use turnloop::{KeepAlive, MulticastGroup, SocketOption, SocketOptionKind};

/// A connected triple whose client keeps the platform's own Nagle setting, so a
/// later `NoDelay(true)` is an observable transition rather than a no-op.
pub fn plain_pair<B: Backend>(l: &mut Driver<B>, listen: &ListenOpts) -> (Handle, Handle, Handle) {
    let server = l.tcp_listen(localhost(), listen).expect("listen");
    l.accept(server, Token(1)).expect("accept");
    let client = l
        .tcp_connect(
            l.local_addr(server).expect("addr"),
            &TcpOpts::default(),
            Token(2),
        )
        .expect("connect");
    let mut conn = None;
    let mut connected = false;
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
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
/// Close every handle and run the loop until nothing is left, so a fixture cannot
/// leak a descriptor into the next one.
pub fn close_all<B: Backend>(l: &mut Driver<B>, handles: &[Handle]) {
    let mut out = Completions::default();
    for (i, h) in handles.iter().enumerate() {
        l.close(*h, Token(900 + i as u64)).expect("close");
    }
    let until = l.now() + Duration::from_secs(5);
    while l.alive() {
        assert!(l.now() < until, "close never completed");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        out.drain();
    }
}
fn read_option<B: Backend>(l: &Driver<B>, h: Handle, kind: SocketOptionKind) -> SocketOption {
    l.get_option(h, kind)
        .unwrap_or_else(|e| panic!("get_option({kind:?}) must reach the OS: {e:?}"))
}
fn keep_alive_of<B: Backend>(l: &Driver<B>, h: Handle) -> Option<KeepAlive> {
    match read_option(l, h, SocketOptionKind::KeepAlive) {
        SocketOption::KeepAlive(value) => value,
        other => panic!("KeepAlive kind answered with {other:?}"),
    }
}
fn buffer_of<B: Backend>(l: &Driver<B>, h: Handle, send: bool) -> u32 {
    let kind = if send {
        SocketOptionKind::SendBufferSize
    } else {
        SocketOptionKind::RecvBufferSize
    };
    match read_option(l, h, kind) {
        SocketOption::SendBufferSize(n) | SocketOption::RecvBufferSize(n) => n,
        other => panic!("buffer kind answered with {other:?}"),
    }
}
/// A kernel may keep the request exactly (macOS, Windows, WASI hosts on those) or
/// double it (Linux). Anything outside that band means the request was not applied.
fn honoured(actual: u32, requested: u32) -> bool {
    actual >= requested && actual <= requested.saturating_mul(2)
}

/// Keep-alive is settable and readable on a connected client **and on an accepted
/// connection**, which is what `socket.setKeepAlive()` needs and what turnloop
/// could not do at all before issue #34.
pub fn keep_alive_round_trip<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &ListenOpts::default());
    let schedule = KeepAlive {
        idle: Some(Duration::from_secs(7)),
        interval: Some(Duration::from_secs(3)),
        count: Some(4),
    };
    for (name, h) in [("client", client), ("accepted", conn)] {
        assert_eq!(keep_alive_of(&l, h), None, "{name} starts without probing");
        l.set_option(h, SocketOption::KeepAlive(Some(schedule)))
            .unwrap_or_else(|e| panic!("{name} keep-alive: {e:?}"));
        let read = keep_alive_of(&l, h).unwrap_or_else(|| panic!("{name} probing must be on"));
        assert_eq!(read.idle, Some(Duration::from_secs(7)), "{name} idle");
        assert_eq!(
            read.interval,
            Some(Duration::from_secs(3)),
            "{name} interval"
        );
        assert_eq!(read.count, Some(4), "{name} count");
        l.set_option(h, SocketOption::KeepAlive(None))
            .unwrap_or_else(|e| panic!("{name} disable: {e:?}"));
        assert_eq!(
            keep_alive_of(&l, h),
            None,
            "{name} probing must be off again"
        );
    }
    // A zero idle time has no meaning to any OS and is rejected, not rounded to
    // "immediately" or silently dropped.
    assert_eq!(
        l.set_option(
            client,
            SocketOption::KeepAlive(Some(KeepAlive {
                idle: Some(Duration::ZERO),
                ..KeepAlive::default()
            })),
        )
        .expect_err("zero idle")
        .kind,
        ErrorKind::InvalidInput
    );
    close_all(&mut l, &[client, conn, server]);
}
/// Buffer sizes reach the kernel on a client, an accepted connection and a UDP
/// socket, and the value read back is the kernel's, not the request.
pub fn buffer_sizes_round_trip<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &ListenOpts::default());
    let udp = l.udp_bind(localhost(), &UdpOpts::default()).expect("bind");
    for (name, h) in [("client", client), ("accepted", conn), ("udp", udp)] {
        for send in [false, true] {
            let mut previous = 0;
            for requested in [32u32 * 1024, 64 * 1024] {
                let option = if send {
                    SocketOption::SendBufferSize(requested)
                } else {
                    SocketOption::RecvBufferSize(requested)
                };
                l.set_option(h, option)
                    .unwrap_or_else(|e| panic!("{name} send={send} {requested}: {e:?}"));
                let actual = buffer_of(&l, h, send);
                assert!(
                    honoured(actual, requested),
                    "{name} send={send}: asked {requested}, OS reports {actual}"
                );
                assert!(
                    actual > previous,
                    "{name} send={send}: {actual} did not grow past {previous}"
                );
                previous = actual;
            }
        }
    }
    close_all(&mut l, &[client, conn, server, udp]);
}
/// The unicast hop limit is settable and readable on UDP (`dgram.setTTL`).
pub fn ttl_round_trip<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let udp = l.udp_bind(localhost(), &UdpOpts::default()).expect("bind");
    let before = match read_option(&l, udp, SocketOptionKind::Ttl) {
        SocketOption::Ttl(hops) => hops,
        other => panic!("Ttl kind answered with {other:?}"),
    };
    assert!(before > 0, "a bound socket always has a hop limit");
    l.set_option(udp, SocketOption::Ttl(7)).expect("set TTL");
    assert!(
        matches!(
            read_option(&l, udp, SocketOptionKind::Ttl),
            SocketOption::Ttl(7)
        ),
        "the OS must report the hop limit we set"
    );
    close_all(&mut l, &[udp]);
}
/// A listener's `accept_defaults` reach every accepted connection before the host
/// sees it, so a server never has to configure each connection by hand.
pub fn accept_defaults_keep_alive<B: Backend>() {
    let schedule = KeepAlive {
        idle: Some(Duration::from_secs(11)),
        ..KeepAlive::default()
    };
    let listen = ListenOpts {
        accept_defaults: AcceptDefaults {
            keep_alive: Some(schedule),
            ..AcceptDefaults::EMPTY
        },
        ..ListenOpts::default()
    };
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &listen);
    let read = keep_alive_of(&l, conn).expect("the accepted socket must have probing on");
    assert_eq!(read.idle, Some(Duration::from_secs(11)));
    assert_eq!(
        keep_alive_of(&l, client),
        None,
        "the default belongs to accepted connections, not to every socket"
    );
    close_all(&mut l, &[client, conn, server]);
}
/// Handles that are not sockets, are closing, or never existed are rejected
/// rather than reaching a backend table with someone else's index.
pub fn option_handle_validation<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let timer = l
        .timer(l.now() + Duration::from_secs(60), None, Token(1))
        .expect("timer");
    assert_eq!(
        l.set_option(timer, SocketOption::RecvBufferSize(4096))
            .expect_err("a timer is not a socket")
            .kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        l.get_option(timer, SocketOptionKind::RecvBufferSize)
            .expect_err("a timer is not a socket")
            .kind,
        ErrorKind::InvalidInput
    );
    let udp = l.udp_bind(localhost(), &UdpOpts::default()).expect("bind");
    l.close(udp, Token(2)).expect("close");
    assert_eq!(
        l.set_option(udp, SocketOption::RecvBufferSize(4096))
            .expect_err("a closing socket takes no options")
            .kind,
        ErrorKind::InvalidInput
    );
    close_all(&mut l, &[timer]);
    assert_eq!(
        l.set_option(udp, SocketOption::RecvBufferSize(4096))
            .expect_err("a released handle is gone")
            .kind,
        ErrorKind::NotFound
    );
}
/// `SO_LINGER` with a zero timeout is the one option whose effect a host can see
/// directly: the peer's pending read fails with `ConnectionReset` where the same
/// close without it delivers `Eof`. Both arms run, so neither verdict is vacuous.
pub fn linger_zero_resets_the_connection<B: Backend>() {
    for reset in [false, true] {
        let mut l = Driver::<B>::new(Config::default()).expect("loop");
        let (server, client, conn) = plain_pair(&mut l, &ListenOpts::default());
        if reset {
            l.set_option(client, SocketOption::Linger(Some(Duration::ZERO)))
                .expect("linger 0");
            assert!(
                matches!(
                    read_option(&l, client, SocketOptionKind::Linger),
                    SocketOption::Linger(Some(Duration::ZERO))
                ),
                "the OS must report the linger we set"
            );
        } else {
            assert!(
                matches!(
                    read_option(&l, client, SocketOptionKind::Linger),
                    SocketOption::Linger(None)
                ),
                "lingering is off by default"
            );
        }
        l.read(conn, ReadBuf::Pooled, Token(10)).expect("read");
        l.close(client, Token(11)).expect("close client");
        let until = l.now() + Duration::from_secs(5);
        let mut out = Completions::default();
        let mut observed = None;
        while observed.is_none() {
            assert!(l.now() < until, "the peer never noticed the close");
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                if c.token == Token(10) {
                    observed = Some(c.result);
                }
            }
        }
        match (reset, observed.expect("read result")) {
            (true, OpResult::Err(e)) => assert_eq!(
                e.kind,
                ErrorKind::ConnectionReset,
                "linger 0 must reset, not close gracefully"
            ),
            (false, OpResult::Eof) => {}
            (reset, other) => panic!("linger zero = {reset}: unexpected {other:?}"),
        }
        close_all(&mut l, &[conn, server]);
    }
}
/// Broadcast and the multicast send options are settable on a UDP socket, and the
/// kernel reports each one back.
pub fn udp_broadcast_and_multicast_options<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let udp = l.udp_bind(localhost(), &UdpOpts::default()).expect("bind");
    assert!(
        matches!(
            read_option(&l, udp, SocketOptionKind::Broadcast),
            SocketOption::Broadcast(false)
        ),
        "broadcast is off by default"
    );
    l.set_option(udp, SocketOption::Broadcast(true))
        .expect("broadcast on");
    assert!(matches!(
        read_option(&l, udp, SocketOptionKind::Broadcast),
        SocketOption::Broadcast(true)
    ));
    l.set_option(udp, SocketOption::Broadcast(false))
        .expect("broadcast off");
    assert!(matches!(
        read_option(&l, udp, SocketOptionKind::Broadcast),
        SocketOption::Broadcast(false)
    ));
    l.set_option(udp, SocketOption::MulticastTtl(4))
        .expect("multicast ttl");
    assert!(
        matches!(
            read_option(&l, udp, SocketOptionKind::MulticastTtl),
            SocketOption::MulticastTtl(4)
        ),
        "the OS must report the multicast hop limit we set"
    );
    l.set_option(udp, SocketOption::MulticastLoop(false))
        .expect("multicast loop");
    assert!(matches!(
        read_option(&l, udp, SocketOptionKind::MulticastLoop),
        SocketOption::MulticastLoop(false)
    ));
    l.set_option(udp, SocketOption::MulticastLoop(true))
        .expect("multicast loop");
    assert!(matches!(
        read_option(&l, udp, SocketOptionKind::MulticastLoop),
        SocketOption::MulticastLoop(true)
    ));
    close_all(&mut l, &[udp]);
}
/// Group membership has no getter, so the proof is the kernel's own bookkeeping:
/// leaving a group it never joined fails, leaving one it joined succeeds, and
/// leaving that one twice fails again. A backend that dropped the join on the
/// floor cannot produce that sequence.
pub fn multicast_membership_is_tracked<B: Backend>(group: std::net::IpAddr, bind: SocketAddr) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let udp = l.udp_bind(bind, &UdpOpts::default()).expect("bind");
    let membership = MulticastGroup {
        group,
        interface: 0,
    };
    assert!(
        l.set_option(udp, SocketOption::MulticastLeave(membership))
            .is_err(),
        "leaving a group that was never joined must fail"
    );
    l.set_option(udp, SocketOption::MulticastJoin(membership))
        .expect("join");
    l.set_option(udp, SocketOption::MulticastLeave(membership))
        .expect("leave the group we joined");
    assert!(
        l.set_option(udp, SocketOption::MulticastLeave(membership))
            .is_err(),
        "the kernel forgot the leave"
    );
    // A group of the other family is a caller error, not a kernel error.
    let mismatched = MulticastGroup {
        group: if group.is_ipv4() {
            std::net::Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1).into()
        } else {
            std::net::Ipv4Addr::new(224, 0, 0, 251).into()
        },
        interface: 0,
    };
    assert_eq!(
        l.set_option(udp, SocketOption::MulticastJoin(mismatched))
            .expect_err("family mismatch")
            .kind,
        ErrorKind::InvalidInput
    );
    close_all(&mut l, &[udp]);
}
/// `IPV6_V6ONLY` is readable on a live socket and, as documented, refused after
/// bind on every platform. The test exists so the documentation cannot drift away
/// from the behaviour.
pub fn ipv6_only_is_readable_and_bind_time<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let Ok(udp) = l.udp_bind(
        (std::net::Ipv6Addr::LOCALHOST, 0).into(),
        &UdpOpts::default(),
    ) else {
        // A host without IPv6 cannot answer this question either way.
        return;
    };
    assert!(
        matches!(
            read_option(&l, udp, SocketOptionKind::Ipv6Only),
            SocketOption::Ipv6Only(_)
        ),
        "a live IPv6 socket must answer the query"
    );
    assert_eq!(
        l.set_option(udp, SocketOption::Ipv6Only(true))
            .expect_err("bind-time only")
            .kind,
        ErrorKind::InvalidInput,
        "setting IPV6_V6ONLY after bind must be reported, not quietly accepted"
    );
    close_all(&mut l, &[udp]);
}
/// Nagle control is settable and readable on a connected client and on an
/// accepted connection, and a listener can apply it to every connection it
/// accepts. This is `socket.setNoDelay()` and the per-connection server default.
pub fn nodelay_round_trip_and_accept_default<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    // The creation-time hint still works, and is visible through the OS.
    let eager = l
        .tcp_connect(
            (std::net::Ipv4Addr::LOCALHOST, 9).into(),
            &TcpOpts { nodelay: true },
            Token(30),
        )
        .expect("connecting socket");
    assert!(
        matches!(
            read_option(&l, eager, SocketOptionKind::NoDelay),
            SocketOption::NoDelay(true)
        ),
        "TcpOpts::nodelay must reach the socket it created"
    );
    // An IP-level option on a socket that has neither connected nor bound: the
    // backend still has to learn its address family to pick IP_TTL over
    // IPV6_UNICAST_HOPS.
    assert!(
        matches!(read_option(&l, eager, SocketOptionKind::Ttl), SocketOption::Ttl(hops) if hops > 0),
        "an unconnected socket still knows its address family"
    );
    close_all(&mut l, &[eager]);

    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &ListenOpts::default());
    for (name, h) in [("client", client), ("accepted", conn)] {
        assert!(
            matches!(
                read_option(&l, h, SocketOptionKind::NoDelay),
                SocketOption::NoDelay(false)
            ),
            "{name}: Nagle is on until a host turns it off"
        );
        l.set_option(h, SocketOption::NoDelay(true))
            .unwrap_or_else(|e| panic!("{name} nodelay: {e:?}"));
        assert!(
            matches!(
                read_option(&l, h, SocketOptionKind::NoDelay),
                SocketOption::NoDelay(true)
            ),
            "{name}: the OS must report TCP_NODELAY"
        );
        l.set_option(h, SocketOption::NoDelay(false))
            .expect("restore");
        assert!(matches!(
            read_option(&l, h, SocketOptionKind::NoDelay),
            SocketOption::NoDelay(false)
        ));
    }
    close_all(&mut l, &[client, conn, server]);

    let listen = ListenOpts {
        accept_defaults: AcceptDefaults {
            nodelay: true,
            ..AcceptDefaults::EMPTY
        },
        ..ListenOpts::default()
    };
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &listen);
    assert!(
        matches!(
            read_option(&l, conn, SocketOptionKind::NoDelay),
            SocketOption::NoDelay(true)
        ),
        "the accepted connection must arrive with the listener's default applied"
    );
    assert!(
        matches!(
            read_option(&l, client, SocketOptionKind::NoDelay),
            SocketOption::NoDelay(false)
        ),
        "the default belongs to accepted connections only"
    );
    close_all(&mut l, &[client, conn, server]);
}
/// A small write still makes a full round trip with Nagle disabled on both
/// endpoints, including on the accepted side, which is the shape a request/
/// response server uses. (Loopback acknowledges instantly, so the *latency*
/// difference Nagle causes is not reproducible here; the behaviour asserted is
/// that the configured connection still carries bytes in both directions.)
pub fn nodelay_small_write_round_trip<B: Backend>() {
    let listen = ListenOpts {
        accept_defaults: AcceptDefaults {
            nodelay: true,
            ..AcceptDefaults::EMPTY
        },
        ..ListenOpts::default()
    };
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &listen);
    l.set_option(client, SocketOption::NoDelay(true))
        .expect("client nodelay");
    let mut echoed = 0;
    let mut received = 0;
    for round in 0..8u64 {
        l.read(conn, ReadBuf::Pooled, Token(20))
            .expect("server read");
        l.write(client, WriteBuf::Owned(vec![round as u8]), Token(21))
            .expect("client write");
        let until = l.now() + Duration::from_secs(5);
        let mut out = Completions::default();
        let mut echo = None;
        while echo.is_none() {
            assert!(l.now() < until, "round {round} stalled");
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                if let OpResult::Read { n, lease: Some(b) } = c.result {
                    assert_eq!(n, 1);
                    assert_eq!(b.as_slice(), &[round as u8]);
                    if c.token == Token(20) {
                        received += 1;
                        l.read(client, ReadBuf::Pooled, Token(22))
                            .expect("read back");
                        l.write(conn, WriteBuf::Owned(vec![round as u8]), Token(23))
                            .expect("echo");
                    } else {
                        echo = Some(());
                        echoed += 1;
                    }
                }
            }
        }
    }
    assert_eq!((received, echoed), (8, 8), "every round must complete");
    close_all(&mut l, &[client, conn, server]);
}
/// A listener's accept defaults belong to the listener, not to the loop that
/// created it: they travel with the transport through `detach`/`attach`, so a
/// listener handed to another agent keeps configuring its connections there.
pub fn accept_defaults_survive_transfer<B: Backend>() {
    let listen = ListenOpts {
        accept_defaults: AcceptDefaults {
            nodelay: true,
            ..AcceptDefaults::EMPTY
        },
        ..ListenOpts::default()
    };
    let mut source = Driver::<B>::new(Config::default()).expect("source loop");
    let server = source.tcp_listen(localhost(), &listen).expect("listen");
    let address = source.local_addr(server).expect("address");
    let transport = source.detach(server).expect("quiescent detach");
    assert!(!source.alive(), "the source loop keeps nothing");

    let mut l = Driver::<B>::new(Config::default()).expect("destination loop");
    let server = l.attach(transport, Token(1)).expect("attach");
    l.accept(server, Token(2)).expect("accept");
    let client = l
        .tcp_connect(address, &TcpOpts::default(), Token(3))
        .expect("connect");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let (mut conn, mut connected) = (None, false);
    while conn.is_none() || !connected {
        assert!(l.now() < until, "transferred listener never accepted");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Accepted { conn: h, .. } => conn = Some(h),
                OpResult::Connected => connected = true,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    let conn = conn.expect("accepted");
    assert!(
        matches!(
            read_option(&l, conn, SocketOptionKind::NoDelay),
            SocketOption::NoDelay(true)
        ),
        "the default did not travel with the listener"
    );
    close_all(&mut l, &[client, conn, server]);
}
/// Options the platform cannot express are reported, never accepted and ignored.
/// `expected` names what this backend genuinely lacks.
pub fn unsupported_options_are_reported<B: Backend>(
    expected: &[(SocketOption, Option<SocketOptionKind>)],
) {
    assert!(!expected.is_empty(), "an empty matrix proves nothing");
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let udp = l.udp_bind(localhost(), &UdpOpts::default()).expect("bind");
    for (option, kind) in expected {
        let refused = l
            .set_option(udp, *option)
            .err()
            .unwrap_or_else(|| panic!("{option:?} was accepted by a backend that cannot apply it"));
        assert_eq!(
            refused.kind,
            ErrorKind::Unsupported,
            "set {option:?} must report Unsupported"
        );
        if let Some(kind) = kind {
            assert_eq!(
                l.get_option(udp, *kind)
                    .expect_err("unsupported getter")
                    .kind,
                ErrorKind::Unsupported,
                "get {kind:?} must report Unsupported"
            );
        }
    }
    close_all(&mut l, &[udp]);
}
/// A backend without Nagle control refuses the creation-time hint as well, so a
/// host cannot tell itself the connection is configured when it is not.
pub fn unsupported_connect_nodelay_is_rejected<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    assert_eq!(
        l.tcp_connect(
            (std::net::Ipv4Addr::LOCALHOST, 9).into(),
            &TcpOpts { nodelay: true },
            Token(1),
        )
        .expect_err("no Nagle control here")
        .kind,
        ErrorKind::Unsupported
    );
    assert!(!l.alive(), "a rejected socket retains nothing");
}
/// A listener default this backend cannot apply is refused when the listener is
/// created, not ignored once per accepted connection.
pub fn unsupported_accept_default_rejects_the_listener<B: Backend>(defaults: AcceptDefaults) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let listen = ListenOpts {
        accept_defaults: defaults,
        ..ListenOpts::default()
    };
    assert_eq!(
        l.tcp_listen(localhost(), &listen)
            .expect_err("unsupported accept default")
            .kind,
        ErrorKind::Unsupported
    );
    assert!(!l.alive(), "a rejected listener retains nothing");
}
