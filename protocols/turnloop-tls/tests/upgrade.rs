//! The mid-stream TLS upgrade, end to end (issue #35).
//!
//! This is Perry's `node:net` `socket.upgradeToTLS` shape, and PostgreSQL's
//! `SSLRequest` exactly: a plaintext connection is established and used, the
//! server agrees to upgrade, and only then does TLS start — on the *same*
//! connection. The socket starts on a turnloop `Loop`, exchanges plaintext
//! through it, is handed to this test as an owned descriptor, and finishes a
//! real rustls handshake there with the loop already dropped. Nothing in the
//! handshake goes through turnloop, so a loop that kept any claim on the
//! descriptor would show up as a failed or hung handshake.
#![cfg(all(feature = "turnloop", not(target_arch = "wasm32")))]
mod support;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use support::*;
use turnloop_io::turnloop::{
    self, Completions, Config, Loop, OpResult, ReadBuf, TcpOpts, Timeout, Token, WriteBuf,
};
use turnloop_tls::{ClientConfig, ClientOptions, rustls::pki_types::ServerName};

/// PostgreSQL's `SSLRequest`: length 8, request code 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

#[cfg(unix)]
fn into_stream(transport: turnloop::Detached) -> TcpStream {
    TcpStream::from(transport.into_fd())
}
#[cfg(windows)]
fn into_stream(transport: turnloop::Detached) -> TcpStream {
    TcpStream::from(transport.into_socket().expect("socket transport"))
}

#[test]
fn a_plaintext_socket_is_handed_off_mid_stream_for_a_real_tls_handshake() {
    let cert = certificate();
    let server = server_config(&cert);
    let client = ClientConfig::new(
        ClientOptions {
            extra_ca_pem: cert.cert.pem().into_bytes(),
            alpn: vec![b"h2".to_vec()],
            ..Default::default()
        },
        NOW,
    )
    .expect("client config");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let peer = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut request = [0; SSL_REQUEST.len()];
        socket.read_exact(&mut request).expect("SSLRequest");
        assert_eq!(request, SSL_REQUEST);
        // 'S' is PostgreSQL's "yes, start TLS now, on this connection".
        socket.write_all(b"S").expect("agreement");
        let mut stream = Stream::new(server.accept().expect("server"), socket);
        let mut ping = [0; 4];
        stream.read_exact(&mut ping).expect("encrypted read");
        assert_eq!(&ping, b"ping");
        assert_eq!(stream.engine.alpn_protocol(), Some(b"h2".as_slice()));
        stream.write_all(b"pong").expect("encrypted write");
    });

    // Phase 1: plaintext, entirely on the loop.
    let mut l = Loop::new(Config::default()).expect("loop");
    let h = l
        .tcp_connect(address, &TcpOpts::default(), Token(1))
        .expect("connect");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut connected = false;
    while !connected {
        assert!(l.now() < until, "connect timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::Connected), "{c:?}");
            connected = true;
        }
    }
    l.write(h, WriteBuf::Owned(SSL_REQUEST.to_vec()), Token(2))
        .expect("write SSLRequest");
    l.read(h, ReadBuf::Pooled, Token(3)).expect("read");
    let until = l.now() + Duration::from_secs(5);
    let (mut wrote, mut agreed) = (false, false);
    while !wrote || !agreed {
        assert!(l.now() < until, "SSLRequest exchange timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Wrote(n) => {
                    assert_eq!(n, SSL_REQUEST.len());
                    wrote = true;
                }
                OpResult::Read {
                    n,
                    lease: Some(data),
                } => {
                    assert_eq!(&data.as_slice()[..n], b"S", "the server refused TLS");
                    agreed = true;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    // Phase 2: the upgrade. The loop hands over the connected socket, keeps
    // nothing, and is dropped before a single TLS byte is written.
    let reported = l.raw_transport(h).expect("live identity");
    let transport = l.detach(h).expect("quiescent handoff");
    assert!(!l.alive(), "the loop still counts the upgraded socket");
    let stream = into_stream(transport);
    #[cfg(unix)]
    assert_eq!(
        turnloop::RawTransport::Fd(std::os::fd::AsRawFd::as_raw_fd(&stream)),
        reported
    );
    #[cfg(windows)]
    assert_eq!(
        turnloop::RawTransport::Socket(
            std::os::windows::io::AsRawSocket::as_raw_socket(&stream) as usize
        ),
        reported
    );
    // Loop-created sockets are handed over non-blocking; a synchronous
    // handshake wants blocking I/O, which is one call away.
    stream.set_nonblocking(false).expect("blocking");
    drop(l);

    // Phase 3: a real rustls handshake and application data on that descriptor.
    let mut tls = Stream::new(
        client
            .connect(ServerName::try_from("localhost").expect("name"))
            .expect("client connection"),
        stream,
    );
    tls.write_all(b"ping").expect("encrypted write");
    let mut pong = [0; 4];
    tls.read_exact(&mut pong).expect("encrypted read");
    assert_eq!(&pong, b"pong");
    assert_eq!(tls.engine.alpn_protocol(), Some(b"h2".as_slice()));
    assert_eq!(
        tls.engine.handshake_kind(),
        Some(turnloop_tls::rustls::HandshakeKind::Full),
        "the upgrade must be a real handshake, not a resumption"
    );
    peer.join().expect("peer");
}
