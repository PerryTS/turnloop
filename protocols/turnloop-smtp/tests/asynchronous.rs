#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use turnloop_io::{
    turnloop::{Config as LoopConfig, LocalExecutor, Timeout, backend::Platform},
    *,
};
fn finish<T>(task: &mut turnloop::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task must finish"),
    }
}
fn drive<T>(executor: &mut LocalExecutor<Platform>, task: &mut turnloop::JoinHandle<T>) -> T {
    let at = executor.handle().now() + Duration::from_secs(90);
    while !task.is_finished() {
        assert!(executor.handle().now() < at, "test deadline");
        executor.turn(Timeout::Until(at)).expect("turn");
    }
    finish(task)
}
async fn exact<S: Stream>(s: &mut S, mut bytes: &mut [u8]) {
    while !bytes.is_empty() {
        let n = read(s, bytes).await.expect("read");
        assert!(n > 0, "unexpected EOF");
        bytes = &mut bytes[n..];
    }
}
use turnloop_smtp::{
    Auth, Envelope, Tls,
    asynchronous::{ConnectOptions, Transport},
};
use turnloop_tls::asynchronous::ClientTls;
use turnloop_tls::{
    ClientConfig, ClientOptions, ServerConfig, TlsStream,
    rustls::pki_types::{CertificateDer, PrivateKeyDer},
};
async fn line<S: Stream>(s: &mut S) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut b = [0];
        exact(s, &mut b).await;
        bytes.push(b[0]);
        assert!(bytes.len() < 8192);
        if bytes.ends_with(b"\r\n") {
            return bytes;
        }
    }
}
#[test]
fn starttls_auth_pipeline_and_recipient_results() {
    for implicit in [false, true] {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("wall time")
            .as_secs();
        let server = ServerConfig::new(
            vec![CertificateDer::from(
                include_bytes!("fixtures/server.der").to_vec(),
            )],
            PrivateKeyDer::try_from(include_bytes!("fixtures/server-key.der").to_vec())
                .expect("key"),
            vec![],
            now,
        )
        .expect("TLS config");
        let tls = ClientTls {
            config: ClientConfig::new(
                ClientOptions {
                    ca: Some(vec![CertificateDer::from(
                        include_bytes!("fixtures/ca.der").to_vec(),
                    )]),
                    alpn: vec![],
                    ..Default::default()
                },
                now,
            )
            .expect("client TLS"),
            server_name: "localhost".try_into().expect("name"),
            unix_seconds: now,
        };
        let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
        let h = ex.handle();
        let sh = h.clone();
        let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
        let address = listener.local_addr().expect("address");
        let mut server = ex
            .spawn_local(async move {
                let mut s = listener.accept().await.expect("accept");
                let at = sh.now() + Duration::from_secs(5);
                if !implicit {
                    write_all(&mut s, b"220 test\r\n").await.expect("greeting");
                    assert!(line(&mut s).await.starts_with(b"EHLO"));
                    write_all(&mut s, b"250-test\r\n250 STARTTLS\r\n")
                        .await
                        .expect("EHLO");
                    assert_eq!(line(&mut s).await, b"STARTTLS\r\n");
                    write_all(&mut s, b"220 ready\r\n").await.expect("STARTTLS");
                }
                let mut s = TlsStream::accept(s, &server, &sh, at, now)
                    .await
                    .expect("TLS handshake");
                if implicit {
                    write_all(&mut s, b"220 test\r\n").await.expect("greeting");
                }
                assert!(line(&mut s).await.starts_with(b"EHLO"));
                write_all(&mut s, b"250-test\r\n250-PIPELINING\r\n250 AUTH PLAIN\r\n")
                    .await
                    .expect("EHLO");
                assert!(line(&mut s).await.starts_with(b"AUTH PLAIN "));
                write_all(&mut s, b"235 authenticated\r\n")
                    .await
                    .expect("auth");
                assert!(line(&mut s).await.starts_with(b"MAIL FROM:"));
                assert_eq!(line(&mut s).await, b"RCPT TO:<ok@example.test>\r\n");
                assert_eq!(line(&mut s).await, b"RCPT TO:<bad@example.test>\r\n");
                write_all(&mut s, b"250 sender\r\n250 recipient\r\n550 rejected\r\n")
                    .await
                    .expect("envelope");
                assert_eq!(line(&mut s).await, b"DATA\r\n");
                write_all(&mut s, b"354 go\r\n").await.expect("DATA");
                assert_eq!(line(&mut s).await, b"..payload\r\n");
                assert_eq!(line(&mut s).await, b".\r\n");
                write_all(&mut s, b"250 queued\r\n").await.expect("sent");
            })
            .expect("spawn");
        let mut client = ex
            .spawn_local(async move {
                let at = h.now() + Duration::from_secs(5);
                let mut t = Transport::connect(
                    &h,
                    &ConnectOptions {
                        address,
                        protocol: turnloop_smtp::Config {
                            tls: if implicit {
                                Tls::Implicit
                            } else {
                                Tls::Required
                            },
                            auth: Some(Auth::Plain {
                                user: "lane".into(),
                                password: "test".into(),
                            }),
                            ..Default::default()
                        },
                        tls: Some(tls),
                    },
                    at,
                )
                .await
                .expect("connect");
                assert!(t.capabilities().pipelining);
                let info = t
                    .send(
                        Envelope {
                            from: "a@example.test".into(),
                            to: vec!["ok@example.test".into(), "bad@example.test".into()],
                        },
                        "message-id".into(),
                        b".payload\n",
                        at,
                    )
                    .await
                    .expect("send");
                assert_eq!(info.accepted, ["ok@example.test"]);
                assert_eq!(info.rejected.len(), 1);
                assert_eq!(info.rejected[0].error.response_code, Some(550));
                assert_eq!(info.response_code, 250);
            })
            .expect("spawn");
        drive(&mut ex, &mut client);
        drive(&mut ex, &mut server);
    }
}
