#![cfg(all(
    feature = "turnloop",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use turnloop_io::{
    turnloop::{Config, LocalExecutor, Timeout, backend::Platform},
    *,
};
use turnloop_tls::{
    ClientConfig, ClientOptions, ServerConfig, TlsStream,
    rustls::pki_types::{PrivatePkcs8KeyDer, ServerName},
};
const NOW: u64 = 1_789_344_000;
fn finish<T>(task: &mut turnloop::executor::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task must complete successfully"),
    }
}
#[test]
fn async_tls_alpn_fragmented_plaintext_and_close_notify() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("certificate");
    let server = ServerConfig::new(
        vec![cert.cert.der().clone()],
        PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
        vec![b"h2".to_vec()],
        NOW,
    )
    .expect("server config");
    let client = ClientConfig::new(
        ClientOptions {
            ca: Some(vec![cert.cert.der().clone()]),
            alpn: vec![b"h2".to_vec()],
            ..Default::default()
        },
        NOW,
    )
    .expect("client config");
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let h2 = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
    let address = listener.local_addr().expect("address");
    let end = h.now() + Duration::from_secs(5);
    let mut s = executor
        .spawn_local(async move {
            let stream = listener.accept().await.expect("accept");
            let mut tls = TlsStream::accept(stream, &server, &h, end, NOW)
                .await
                .expect("server handshake");
            assert_eq!(tls.alpn_protocol(), Some(b"h2".as_slice()));
            let mut data = [0; 1];
            let mut count = 0;
            while count < 32768 {
                let n = read(&mut tls, &mut data).await.expect("TLS read");
                assert_eq!((n, data[0]), (1, b'x'), "early EOF or corrupt plaintext");
                count += n;
            }
            write_all(&mut tls, b"k").await.expect("acknowledge");
            flush(&mut tls).await.expect("acknowledge flush");
            assert_eq!(read(&mut tls, &mut data).await.expect("close notify"), 0);
            count
        })
        .expect("spawn");
    let mut c = executor
        .spawn_local(async move {
            let stream = h2
                .connect(address, Default::default())
                .await
                .expect("connect");
            let mut tls = TlsStream::connect(
                stream,
                &client,
                ServerName::try_from("localhost").expect("name"),
                &h2,
                end,
                NOW,
            )
            .await
            .expect("client handshake");
            assert_eq!(tls.alpn_protocol(), Some(b"h2".as_slice()));
            write_all(&mut tls, &[b'x'; 32768])
                .await
                .expect("TLS write");
            flush(&mut tls).await.expect("TLS flush");
            // Reading the acknowledgement also consumes the server's session tickets.
            // Closing a socket with unread bytes sends RST, and Windows then discards
            // data the server has not read yet (Unix delivers it before the reset).
            let mut ack = [0; 1];
            assert_eq!(read(&mut tls, &mut ack).await.expect("acknowledgement"), 1);
            assert_eq!(ack[0], b'k');
            close(&mut tls).await.expect("close notify");
            32768
        })
        .expect("spawn");
    while !s.is_finished() || !c.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut s), finish(&mut c));
}
#[test]
fn silent_peer_handshake_deadline() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
    let address = listener.local_addr().expect("address");
    let server = executor
        .spawn_local(async move {
            let _s = listener.accept().await.expect("accept");
            std::future::pending::<()>().await
        })
        .expect("spawn");
    let mut c = executor
        .spawn_local(async move {
            let stream = h
                .connect(address, Default::default())
                .await
                .expect("connect");
            let config = ClientConfig::new(ClientOptions::default(), NOW).expect("config");
            let end = h.now() + Duration::from_millis(5);
            let result = TlsStream::connect(
                stream,
                &config,
                ServerName::try_from("localhost").expect("name"),
                &h,
                end,
                NOW,
            )
            .await;
            assert_eq!(
                result.err().expect("timeout").kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(h.now() >= end);
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !c.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    finish(&mut c);
    drop(server);
    executor.turn(Timeout::Now).expect("cancel turn");
}
