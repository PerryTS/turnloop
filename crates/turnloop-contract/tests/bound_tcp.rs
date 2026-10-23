//! A host may bind/configure TCP before choosing its peer. Adoption must keep
//! that exact socket and local port through the handshake and cancellation.
#![cfg(all(not(loom), any(unix, windows), not(target_arch = "wasm32")))]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};
use turnloop::{
    backend::{Backend, Platform},
    *,
};
type Transport = <Platform as Backend>::Detached;
#[path = "bound_tcp/native.rs"]
mod native;

fn take(l: &mut Loop, token: Token) -> Completion {
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    loop {
        assert!(l.now() < until, "completion {token:?} timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        if !out.is_empty() {
            assert_eq!(out.len(), 1, "duplicate or unexpected completion");
            let completion = out.drain().next().unwrap();
            assert_eq!(completion.token, token, "unexpected completion");
            assert!(completion.terminal);
            return completion;
        }
    }
}
fn close(l: &mut Loop, handle: Handle) {
    l.close(handle, Token(99)).expect("close");
    assert!(matches!(take(l, Token(99)).result, OpResult::Closed));
    assert!(!l.alive(), "native ownership must end after close");
}
fn exchange(v6: bool) {
    let listener = TcpListener::bind(if v6 { "[::1]:0" } else { "127.0.0.1:0" }).expect("listener");
    let remote = listener.local_addr().expect("remote endpoint");
    let (transport, local) = native::socket(v6, true);
    let identity = native::identity(&transport);
    // A live bound socket prevents a second exclusive bind. Never close/rebind
    // as an implementation of adoption: both peers below must see this port.
    assert!(TcpListener::bind(local).is_err());
    let mut l = Loop::new(Config::default()).expect("loop");
    let handle = l
        .tcp_connect_bound(transport, remote, &TcpOpts::default(), Token(1))
        .expect("adopt/connect");
    assert_eq!(
        l.raw_transport(handle).unwrap(),
        identity,
        "adoption replaced the native socket"
    );
    let recv_buffer = l
        .get_option(handle, SocketOptionKind::RecvBufferSize)
        .unwrap();
    assert!(matches!(recv_buffer,SocketOption::RecvBufferSize(n) if n>=65536));
    assert_eq!(
        l.local_addr(handle).expect("local before completion"),
        local
    );
    assert_eq!(
        l.get_option(handle, SocketOptionKind::NoDelay)
            .expect("nodelay"),
        SocketOption::NoDelay(true)
    );
    assert_eq!(
        l.get_option(handle, SocketOptionKind::Ttl)
            .expect("hop limit"),
        SocketOption::Ttl(47)
    );
    assert!(matches!(take(&mut l, Token(1)).result, OpResult::Connected));
    assert_eq!(l.raw_transport(handle).unwrap(), identity);
    // Kernel autotuning may grow the receive buffer during the handshake.
    assert!(
        matches!(l.get_option(handle, SocketOptionKind::RecvBufferSize).unwrap(), SocketOption::RecvBufferSize(n) if n>=65536)
    );
    let (mut peer, observed) = listener.accept().expect("accept");
    assert_eq!(observed, local);
    peer.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read deadline");
    peer.set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write deadline");
    l.write(handle, WriteBuf::Owned(b"bound-port".to_vec()), Token(2))
        .expect("write");
    assert!(matches!(take(&mut l, Token(2)).result, OpResult::Wrote(10)));
    let mut bytes = [0; 10];
    peer.read_exact(&mut bytes).expect("peer read");
    assert_eq!(&bytes, b"bound-port");
    peer.write_all(b"reply").expect("peer write");
    l.read(handle, ReadBuf::Pooled, Token(3)).expect("read");
    let completion = take(&mut l, Token(3));
    match completion.result {
        OpResult::Read {
            n,
            lease: Some(lease),
        } => {
            assert_eq!(n, 5);
            assert_eq!(lease.as_slice(), b"reply");
        }
        other => panic!("unexpected read: {other:?}"),
    }
    close(&mut l, handle);
}
#[test]
fn adopted_ipv4_preserves_bind_options_and_bytes() {
    exchange(false);
}
#[test]
fn adopted_ipv6_preserves_bind_options_and_bytes() {
    exchange(true);
}

#[test]
fn invalid_transports_and_setup_failures_release_their_ownership() {
    let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
    for mode in 0..4 {
        let (transport, local) = native::socket(false, mode != 0);
        let mut l = Loop::new(Config::default()).unwrap();
        let opts = TcpOpts {
            connect_timeout: if mode == 2 {
                Some(Duration::ZERO)
            } else {
                None
            },
            ..TcpOpts::default()
        };
        let address = if mode == 1 {
            "[::1]:9".parse().unwrap()
        } else {
            peer
        };
        if mode == 3 {
            // Spend the only operation slot, then reject connect after backend
            // adoption. Its registration and handle must roll back as well.
            drop(l);
            l = Loop::new(Config {
                max_operations: 1,
                ..Config::default()
            })
            .unwrap();
            let timer = l
                .timer(l.now() + Duration::from_secs(60), None, Token(8))
                .unwrap();
            assert_eq!(
                l.tcp_connect_bound(transport, address, &opts, Token(1))
                    .unwrap_err()
                    .kind,
                ErrorKind::ResourceLimit
            );
            assert!(l.cancel(l.timer_op(timer).unwrap()));
            assert!(matches!(take(&mut l, Token(8)).result, OpResult::Cancelled));
            close(&mut l, timer);
        } else {
            assert!(
                l.tcp_connect_bound(transport, address, &opts, Token(1))
                    .is_err()
            );
            assert!(!l.alive());
        }
        if mode != 0 {
            drop(TcpListener::bind(local).expect("failed setup leaked bound socket"));
        }
    }
    let mut l = Loop::new(Config::default()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    assert!(
        l.tcp_connect_bound(
            native::listener(listener),
            peer,
            &TcpOpts::default(),
            Token(1)
        )
        .is_err()
    );
    assert!(!l.alive());
    drop(TcpListener::bind(address).expect("rejected listener released"));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut peer_stream, _) = listener.accept().unwrap();
    peer_stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert!(
        l.tcp_connect_bound(native::stream(stream), peer, &TcpOpts::default(), Token(1))
            .is_err()
    );
    assert!(!l.alive());
    assert_eq!(
        peer_stream.read(&mut [0]).unwrap(),
        0,
        "rejected connected socket leaked"
    );
}

#[test]
fn closing_an_adopted_attempt_delivers_one_terminal_result_before_close() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (transport, _) = native::socket(false, true);
    let mut l = Loop::new(Config::default()).unwrap();
    let h = l
        .tcp_connect_bound(
            transport,
            listener.local_addr().unwrap(),
            &TcpOpts::default(),
            Token(1),
        )
        .unwrap();
    l.close(h, Token(2)).unwrap();
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let (mut terminal, mut closed) = (0, 0);
    while closed == 0 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).unwrap();
        for c in out.drain() {
            assert!(c.terminal);
            if c.token == Token(1) {
                assert_eq!(closed, 0);
                terminal += 1;
                assert!(matches!(
                    c.result,
                    OpResult::Connected | OpResult::Cancelled | OpResult::Err(_)
                ));
            } else {
                assert_eq!(c.token, Token(2));
                assert!(matches!(c.result, OpResult::Closed));
                assert_eq!(terminal, 1);
                closed += 1;
            }
        }
    }
    assert_eq!((terminal, closed), (1, 1));
    assert!(!l.alive());
    l.turn(Timeout::Now, &mut out).unwrap();
    assert!(out.is_empty());
}

#[cfg(feature = "executor")]
#[test]
fn executor_connects_the_bound_transport_and_flushes_real_bytes() {
    use futures_io::AsyncWrite;
    use std::{cell::Cell, future::poll_fn, pin::Pin, rc::Rc};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = listener.local_addr().unwrap();
    let (transport, local) = native::socket(false, true);
    let mut ex = LocalExecutor::<Platform>::new(Config::default()).unwrap();
    let io = ex.handle();
    let completed = Rc::new(Cell::new(false));
    let flag = completed.clone();
    let task = ex
        .spawn_local(async move {
            let mut stream = io
                .connect_bound(transport, remote, TcpOpts::default())
                .await
                .expect("bound connect future");
            assert_eq!(io.driver().local_addr(stream.handle()).unwrap(), local);
            let mut offset = 0;
            while offset < 8 {
                let n = poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, &b"executor"[offset..]))
                    .await
                    .unwrap();
                assert!(n > 0);
                offset += n;
            }
            poll_fn(|cx| Pin::new(&mut stream).poll_close(cx))
                .await
                .unwrap();
            flag.set(true);
        })
        .unwrap();
    let until = ex.driver().now() + Duration::from_secs(5);
    while !task.is_finished() || ex.alive() {
        assert!(
            ex.driver().now() < until,
            "executor did not release its socket"
        );
        ex.turn(Timeout::Until(until)).unwrap();
    }
    assert!(completed.get());
    let (mut peer, observed) = listener.accept().unwrap();
    assert_eq!(observed, local);
    peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut bytes = Vec::new();
    peer.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"executor");
}

#[cfg(feature = "executor")]
#[test]
fn dropping_unpolled_and_pending_bound_connect_futures_releases_ownership() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Waker},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = listener.local_addr().unwrap();
    let mut ex = LocalExecutor::<Platform>::new(Config::default()).unwrap();
    let io = ex.handle();
    let (transport, local) = native::socket(false, true);
    let future = io.connect_bound(transport, remote, TcpOpts::default());
    assert!(TcpListener::bind(local).is_err());
    drop(future);
    drop(TcpListener::bind(local).expect("unpolled future leaked bound socket"));
    let (transport, _) = native::socket(false, true);
    let mut future = io.connect_bound(transport, remote, TcpOpts::default());
    let mut cx = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut future).poll(&mut cx).is_pending());
    drop(future);
    let until = ex.driver().now() + Duration::from_secs(5);
    while ex.alive() {
        assert!(
            ex.driver().now() < until,
            "abandoned native connect did not quiesce"
        );
        ex.turn(Timeout::Until(until)).unwrap();
    }
    // The same executor still admits a new connection after cancellation.
    let (transport, _) = native::socket(false, true);
    let mut future = io.connect_bound(transport, remote, TcpOpts::default());
    let stream = loop {
        if let std::task::Poll::Ready(result) = Pin::new(&mut future).poll(&mut cx) {
            break result.unwrap();
        }
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).unwrap();
    };
    drop(stream);
    while ex.alive() {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).unwrap();
    }
}
