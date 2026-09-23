//! Adopting a listening socket the host created, on every Unix backend.
//!
//! `Detached::from_fd` recognises a listener only where the kernel answers
//! `SO_ACCEPTCONN`; macOS and the BSDs do not, so there an adopted listener used
//! to become a stream that could never accept. `Detached::from_listener_fd` says
//! what the descriptor is. These tests bind outside turnloop, adopt, and prove the
//! loop accepts a real connection and reads its bytes.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    )
))]
use std::{
    io::Write,
    net::{Ipv4Addr, TcpListener, TcpStream},
    os::{
        fd::OwnedFd,
        unix::net::{UnixListener, UnixStream},
    },
    time::Duration,
};
use turnloop::*;

/// Accept one connection on an adopted listener, then read the byte the peer
/// wrote before the accept was even submitted.
fn accept_and_read(listener: OwnedFd, connect: impl FnOnce() -> Box<dyn Write>) {
    let mut l = Loop::new(Config::default()).expect("loop");
    let server = l
        .attach(
            Detached::from_listener_fd(listener).expect("adopt listener"),
            Token(1),
        )
        .expect("attach");
    let mut client = connect();
    client.write_all(b"x").expect("client write");
    l.accept(server, Token(2)).expect("accept");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut conn = None;
    let mut read = false;
    while !read {
        assert!(l.now() < until, "adopted listener never accepted and read");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Accepted { conn: h, .. } | OpResult::PipeAccepted { conn: h } => {
                    conn = Some(h);
                    l.read(h, ReadBuf::Pooled, Token(3)).expect("read");
                }
                OpResult::Read { n, lease: Some(b) } => {
                    assert_eq!((n, b.as_slice()), (1, &b"x"[..]));
                    read = true;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    let conn = conn.expect("accepted");
    for (i, h) in [conn, server].into_iter().enumerate() {
        l.close(h, Token(10 + i as u64)).expect("close");
    }
    while l.alive() {
        assert!(l.now() < until, "close never completed");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        out.drain();
    }
}

#[test]
fn adopted_tcp_listener_accepts() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let address = listener.local_addr().expect("address");
    accept_and_read(listener.into(), move || {
        Box::new(TcpStream::connect(address).expect("connect"))
    });
}

#[test]
fn adopted_unix_listener_accepts() {
    let path = std::env::temp_dir().join(format!("turnloop-adopt-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    let peer = path.clone();
    accept_and_read(listener.into(), move || {
        Box::new(UnixStream::connect(&peer).expect("connect"))
    });
    let _ = std::fs::remove_file(&path);
}

/// What is provably not a listener is refused rather than adopted as one: a
/// connected socket (it has a peer, which a listener never has) and a file.
#[test]
fn from_listener_fd_refuses_what_is_not_a_listener() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let connected = TcpStream::connect(listener.local_addr().expect("address")).expect("connect");
    let error = Detached::from_listener_fd(connected.into()).expect_err("connected socket");
    assert_eq!(error.kind, ErrorKind::InvalidInput);

    let file = std::fs::File::open(std::env::current_exe().expect("exe")).expect("open");
    let error = Detached::from_listener_fd(file.into()).expect_err("file");
    assert_eq!(error.kind, ErrorKind::InvalidInput);
}
