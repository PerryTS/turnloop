//! Native TCP adoption contract, exercised by both Unix and Windows CI.
use crate::*;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    time::Duration,
};

fn bound(address: SocketAddr) -> (Detached, SocketAddr) {
    #[cfg(any(turnloop_backend = "epoll", turnloop_backend = "kqueue"))]
    {
        backend::unix::bound_tcp_for_test(address)
    }
    #[cfg(turnloop_backend = "iocp")]
    {
        backend::iocp::bound_tcp_for_test(address)
    }
}

fn connected(stream: TcpStream) -> Detached {
    #[cfg(any(turnloop_backend = "epoll", turnloop_backend = "kqueue"))]
    {
        Detached::from_fd(stream.into()).unwrap()
    }
    #[cfg(turnloop_backend = "iocp")]
    {
        Detached::from_socket(stream.into()).unwrap()
    }
}

fn take(loop_: &mut Loop, op: OpId) -> OpResult {
    let until = loop_.now() + Duration::from_secs(5);
    let mut events = Completions::default();
    loop {
        assert!(loop_.now() < until, "operation did not complete");
        loop_.turn(Timeout::Until(until), &mut events).unwrap();
        for event in events.drain() {
            if event.op == Some(op) {
                assert!(event.terminal);
                return event.result;
            }
        }
    }
}

fn drain_close(loop_: &mut Loop, handle: Handle) {
    loop_.close(handle, Token(99)).unwrap();
    let until = loop_.now() + Duration::from_secs(5);
    let mut events = Completions::default();
    while loop_.alive() {
        assert!(loop_.now() < until, "close did not complete");
        loop_.turn(Timeout::Until(until), &mut events).unwrap();
    }
}

#[test]
fn bound_connect_preserves_endpoint_and_transfers_bytes() {
    for address in ["127.0.0.1:0", "[::1]:0"] {
        let listener = TcpListener::bind(address).unwrap();
        let remote = listener.local_addr().unwrap();
        let (transport, local) = bound(address.parse().unwrap());
        let mut driver = Loop::new(Config::default()).unwrap();
        let (handle, op) = driver
            .tcp_connect_bound(transport, remote, Token(41))
            .unwrap();
        assert_eq!(driver.local_addr(handle).unwrap(), local);
        assert!(matches!(take(&mut driver, op), OpResult::Connected));
        listener.set_nonblocking(true).unwrap();
        let (mut peer, observed) = listener.accept().unwrap();
        assert_eq!(observed, local);
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let op = driver
            .write(handle, WriteBuf::Owned(b"bound".to_vec()), Token(42))
            .unwrap();
        assert!(matches!(take(&mut driver, op), OpResult::Wrote(5)));
        let mut bytes = [0; 5];
        peer.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"bound");
        peer.write_all(b"reply").unwrap();
        let op = driver.read(handle, ReadBuf::Pooled, Token(43)).unwrap();
        match take(&mut driver, op) {
            OpResult::Read {
                n,
                lease: Some(lease),
            } => {
                assert_eq!(n, 5);
                assert_eq!(&lease.as_slice()[..n], b"reply");
            }
            result => panic!("unexpected receive: {result:?}"),
        }
        drain_close(&mut driver, handle);
    }
}

#[test]
fn bound_connect_rejects_connected_and_mismatched_sockets_without_leaking_handle() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = listener.local_addr().unwrap();
    let established = TcpStream::connect(remote).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    let mut driver = Loop::new(Config::default()).unwrap();
    let error = driver
        .tcp_connect_bound(connected(established), remote, Token(41))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(!driver.alive());
    let (transport, _) = bound("127.0.0.1:0".parse().unwrap());
    let error = driver
        .tcp_connect_bound(transport, "[::1]:80".parse().unwrap(), Token(42))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidInput);
    assert!(!driver.alive());
}

#[test]
fn bound_connect_cancel_has_one_terminal_acknowledgement() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (transport, _) = bound("127.0.0.1:0".parse().unwrap());
    let mut driver = Loop::new(Config::default()).unwrap();
    let (handle, op) = driver
        .tcp_connect_bound(transport, listener.local_addr().unwrap(), Token(41))
        .unwrap();
    assert!(driver.cancel(op));
    assert!(matches!(take(&mut driver, op), OpResult::Cancelled));
    let mut events = Completions::default();
    driver.turn(Timeout::Now, &mut events).unwrap();
    assert!(!events.iter().any(|event| event.op == Some(op)));
    drain_close(&mut driver, handle);
}

#[test]
fn bound_connect_submission_failure_releases_adopted_socket() {
    let config = Config {
        max_operations: 1,
        ..Config::default()
    };
    let mut driver = Loop::new(config).unwrap();
    let timer = driver
        .timer(driver.now() + Duration::from_secs(60), None, Token(1))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (transport, local) = bound("127.0.0.1:0".parse().unwrap());
    let error = driver
        .tcp_connect_bound(transport, listener.local_addr().unwrap(), Token(41))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::ResourceLimit);
    // Rebinding proves the rejected owned descriptor was closed.
    let rebound = TcpListener::bind(local).unwrap();
    assert_eq!(rebound.local_addr().unwrap(), local);
    drain_close(&mut driver, timer);
}
