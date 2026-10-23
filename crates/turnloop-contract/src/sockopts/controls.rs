use super::*;

fn settings<B: Backend>(l: &Driver<B>, h: Handle) -> KeepAlive {
    match read_option(l, h, SocketOptionKind::KeepAliveSettings) {
        SocketOption::KeepAliveSettings(value) => value,
        other => panic!("unexpected schedule {other:?}"),
    }
}
fn enabled<B: Backend>(l: &Driver<B>, h: Handle) -> bool {
    match read_option(l, h, SocketOptionKind::KeepAliveEnabled) {
        SocketOption::KeepAliveEnabled(value) => value,
        other => panic!("unexpected switch {other:?}"),
    }
}

/// WASI configures each timing independently, including while probing is off.
pub fn keep_alive_controls_are_independent<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = plain_pair(&mut l, &ListenOpts::default());
    for h in [server, client, conn] {
        assert!(!enabled(&l, h));
        let original = settings(&l, h);
        assert!(original.idle.is_some() && original.interval.is_some() && original.count.is_some());
        let schedule = KeepAlive {
            idle: Some(Duration::from_secs(7)),
            interval: Some(Duration::from_secs(3)),
            count: Some(4),
        };
        l.set_option(h, SocketOption::KeepAliveSettings(schedule))
            .expect("schedule");
        assert_eq!(settings(&l, h), schedule);
        assert!(!enabled(&l, h), "changing timings enabled probing");
        l.set_option(h, SocketOption::KeepAliveEnabled(true))
            .expect("enable");
        assert_eq!(keep_alive_of(&l, h), Some(schedule));
        let updated = KeepAlive {
            idle: Some(Duration::from_secs(9)),
            ..schedule
        };
        l.set_option(
            h,
            SocketOption::KeepAliveSettings(KeepAlive {
                idle: updated.idle,
                ..KeepAlive::default()
            }),
        )
        .expect("partial update");
        assert_eq!(keep_alive_of(&l, h), Some(updated));
        l.set_option(h, SocketOption::KeepAliveEnabled(false))
            .expect("disable");
        assert_eq!(keep_alive_of(&l, h), None);
        assert_eq!(settings(&l, h), updated);
        // Validation must happen before writing the first valid field.
        for invalid in [
            KeepAlive {
                idle: Some(Duration::from_secs(11)),
                interval: Some(Duration::ZERO),
                count: None,
            },
            KeepAlive {
                idle: Some(Duration::from_secs(11)),
                interval: None,
                count: Some(0),
            },
            KeepAlive {
                idle: Some(Duration::MAX),
                ..KeepAlive::default()
            },
        ] {
            for option in [
                SocketOption::KeepAliveSettings(invalid),
                SocketOption::KeepAlive(Some(invalid)),
            ] {
                assert_eq!(
                    l.set_option(h, option).expect_err("invalid schedule").kind,
                    ErrorKind::InvalidInput
                );
                assert!(!enabled(&l, h), "invalid schedule changed switch");
                assert_eq!(
                    settings(&l, h),
                    updated,
                    "invalid schedule partially applied"
                );
            }
        }
        l.set_option(h, SocketOption::KeepAliveSettings(KeepAlive::default()))
            .expect("empty update");
        assert_eq!(settings(&l, h), updated);
        assert!(!enabled(&l, h));
    }
    close_all(&mut l, &[server, client, conn]);
}

/// Updating a backlog preserves the listener and an already pending accept.
pub fn listener_backlog_preserves_pending_accept<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let server = l
        .tcp_listen(localhost(), &ListenOpts::default())
        .expect("listen");
    let address = l.local_addr(server).expect("address");
    l.accept(server, Token(1)).expect("pending accept");
    for size in [0, u32::MAX] {
        assert_eq!(
            l.set_option(server, SocketOption::ListenBacklog(size))
                .expect_err("invalid backlog")
                .kind,
            ErrorKind::InvalidInput
        );
    }
    for size in [9, 17] {
        l.set_option(server, SocketOption::ListenBacklog(size))
            .expect("update backlog");
        assert_eq!(l.local_addr(server).expect("same address"), address);
    }
    let client = l
        .tcp_connect(address, &TcpOpts::default(), Token(2))
        .expect("connect");
    let mut conn = None;
    let mut connected = false;
    let mut out = Completions::default();
    let deadline = l.now() + Duration::from_secs(5);
    while conn.is_none() || !connected {
        assert!(l.now() < deadline);
        l.turn(Timeout::Until(deadline), &mut out).expect("turn");
        for completion in out.drain() {
            match completion.result {
                OpResult::Accepted { conn: h, .. } => {
                    assert!(conn.replace(h).is_none());
                }
                OpResult::Connected => {
                    assert!(!connected);
                    connected = true;
                }
                other => panic!("unexpected completion {other:?}"),
            }
        }
    }
    let conn = conn.expect("accepted");
    for h in [client, conn] {
        assert_eq!(
            l.set_option(h, SocketOption::ListenBacklog(9))
                .expect_err("connected is not listener")
                .kind,
            ErrorKind::InvalidInput
        );
    }
    l.read(conn, ReadBuf::Pooled, Token(3)).expect("read");
    l.write(client, WriteBuf::Owned(b"backlog".to_vec()), Token(4))
        .expect("write");
    let mut received = Vec::new();
    let mut written = 0;
    while received.len() < 7 || written < 7 {
        assert!(l.now() < deadline);
        l.turn(Timeout::Until(deadline), &mut out).expect("turn");
        for completion in out.drain() {
            match completion.result {
                OpResult::Read {
                    n,
                    lease: Some(lease),
                } => {
                    received.extend_from_slice(&lease.as_slice()[..n]);
                    if received.len() < 7 {
                        l.read(conn, ReadBuf::Pooled, Token(3))
                            .expect("remaining read");
                    }
                }
                OpResult::Wrote(n) => written += n,
                other => panic!("unexpected I/O {other:?}"),
            }
        }
    }
    assert_eq!(received, b"backlog");
    assert_eq!(written, 7);
    close_all(&mut l, &[server, client, conn]);
}
