#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[path = "../../../crates/turnloop-io/tests/support/count.rs"]
mod count;
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
use turnloop_postgres::{
    Event, Outcome,
    asynchronous::{Client, ConnectOptions, Pool},
};
const AUTH: &[u8] = b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I";
const RESULT: &[u8] = b"D\0\0\0\x0c\0\x01\0\0\0\x0242C\0\0\0\x0dSELECT 1\0Z\0\0\0\x05I";
async fn startup_packet<S: Stream>(s: &mut S) {
    let mut length = [0; 4];
    exact(s, &mut length).await;
    let n = u32::from_be_bytes(length) as usize;
    assert!((8..1024).contains(&n));
    let mut body = [0; 1024];
    exact(s, &mut body[..n - 4]).await;
    assert_eq!(&body[..4], &[0, 3, 0, 0]);
}
async fn startup<S: Stream>(s: &mut S) {
    startup_packet(s).await;
    write_all(s, AUTH).await.expect("auth");
}
async fn query<S: Stream>(s: &mut S) -> Vec<u8> {
    let mut header = [0; 5];
    exact(s, &mut header).await;
    assert_eq!(header[0], b'Q');
    let n = u32::from_be_bytes(header[1..].try_into().expect("length")) as usize;
    let mut b = vec![0; n - 4];
    exact(s, &mut b).await;
    b
}
#[test]
fn simple_query_timeout_and_pool_reuse() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let server_h = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            startup(&mut s).await;
            for _ in 0..2 {
                assert_eq!(query(&mut s).await, b"SELECT 42\0");
                write_all(&mut s, RESULT).await.expect("result");
            }
            assert_eq!(query(&mut s).await, b"SELECT stall\0");
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("cancel EOF"), 0);
            let mut next = listener.accept().await.expect("replacement");
            startup(&mut next).await;
            assert_eq!(query(&mut next).await, b"SELECT 42\0");
            write_all(&mut next, RESULT).await.expect("result");
            let mut b = [0];
            assert_eq!(read(&mut next, &mut b).await.expect("end EOF"), 0);
            drop(server_h);
            4
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(5);
            let options = ConnectOptions {
                address,
                protocol: Default::default(),
                tls: None,
                channel_binding: None,
            };
            let pool = Pool::new(
                &h,
                options,
                turnloop_postgres::pool::Config {
                    max: 1,
                    max_idle: 1,
                    ..Default::default()
                },
                Duration::from_secs(2),
            )
            .expect("pool");
            let mut rows = 0;
            for _ in 0..2 {
                let mut c = pool.acquire(at).await.expect("checkout");
                assert_eq!(
                    c.query("SELECT 42", at, |e| {
                        if let Event::Row { mut row, .. } = e {
                            assert_eq!(
                                row.next().expect("row").expect("value"),
                                Some(b"42".as_slice())
                            );
                            rows += 1;
                        }
                        Ok(())
                    })
                    .await
                    .expect("query"),
                    Outcome::Success
                );
            }
            let mut c = pool.acquire(at).await.expect("checkout");
            assert_eq!(
                c.query("SELECT stall", h.now() + Duration::from_millis(2), |_| Ok(
                    ()
                ))
                .await
                .expect_err("timeout")
                .kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(!c.is_reusable());
            drop(c);
            let mut c = pool.acquire(at).await.expect("replacement");
            assert_eq!(
                c.query("SELECT 42", at, |_| Ok(())).await.expect("query"),
                Outcome::Success
            );
            drop(c);
            pool.end().await.expect("end");
            assert_eq!(rows, 2);
            rows
        })
        .expect("spawn");
    assert_eq!(drive(&mut ex, &mut client), 2);
    assert_eq!(drive(&mut ex, &mut server), 4);
}
#[test]
fn copy_stream_drop_closes_session() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            startup(&mut s).await;
            assert_eq!(query(&mut s).await, b"COPY t FROM STDIN\0");
            write_all(&mut s, b"G\0\0\0\x07\0\0\0")
                .await
                .expect("copy ready");
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(3);
            let mut c = Client::connect(
                &h,
                &ConnectOptions {
                    address,
                    protocol: Default::default(),
                    tls: None,
                    channel_binding: None,
                },
                at,
            )
            .await
            .expect("connect");
            drop(c.copy_in("COPY t FROM STDIN", at).await.expect("COPY"));
            assert!(!c.is_reusable());
        })
        .expect("spawn");
    drive(&mut ex, &mut client);
    drive(&mut ex, &mut server);
}

#[test]
fn warmed_async_pool_queries_allocate_zero_and_idle_waits() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            startup(&mut s).await;
            for _ in 0..1001 {
                assert_eq!(query(&mut s).await, b"SELECT 42\0");
                write_all(&mut s, RESULT).await.expect("reply");
            }
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
            1001
        })
        .expect("server");
    let idle = std::rc::Rc::new(std::cell::Cell::new(false));
    let client_idle = idle.clone();
    let mut client = ex
        .spawn_local(async move {
            count::prove_counter().await;
            let at = h.now() + Duration::from_secs(30);
            let pool = Pool::new(
                &h,
                ConnectOptions {
                    address,
                    protocol: Default::default(),
                    tls: None,
                    channel_binding: None,
                },
                turnloop_postgres::pool::Config {
                    max: 1,
                    max_idle: 1,
                    idle_timeout: Some(Duration::from_secs(60)),
                    ..Default::default()
                },
                Duration::from_secs(5),
            )
            .expect("pool");
            let held = pool.acquire(at).await.expect("hold only slot");
            let blocked = pool.acquire(h.now() + Duration::from_millis(10)).await;
            assert!(
                matches!(blocked, Err(e) if e.kind() == std::io::ErrorKind::TimedOut),
                "queued acquire deadline must run"
            );
            drop(held);
            for i in 0..1001 {
                let (rows, n) = count::measure(async {
                    let mut c = pool.acquire(at).await.expect("acquire");
                    let mut rows = 0;
                    assert_eq!(
                        c.query("SELECT 42", at, |e| {
                            if let Event::Row { mut row, .. } = e {
                                assert_eq!(
                                    row.next().expect("row").expect("value"),
                                    Some(b"42".as_slice())
                                );
                                rows += 1;
                            }
                            Ok(())
                        })
                        .await
                        .expect("query"),
                        Outcome::Success
                    );
                    rows
                })
                .await;
                assert_eq!(rows, 1);
                if i > 0 {
                    assert_eq!(n, 0, "query + checkout/release must allocate zero");
                }
            }
            client_idle.set(true);
            h.sleep(Duration::from_millis(60))
                .await
                .expect("idle timer");
            pool.end().await.expect("end");
        })
        .expect("client");
    let end = ex.handle().now() + Duration::from_secs(30);
    while !idle.get() {
        assert!(ex.handle().now() < end);
        ex.turn(Timeout::Until(end)).expect("turn");
    }
    // Drain finite completions; then the pooled idle socket has no readiness
    // subscription, and a turn must actually wait for the timer.
    let mut waited = false;
    for _ in 0..8 {
        let before = ex.handle().now();
        let info = ex.turn(Timeout::Until(end)).expect("idle turn");
        if ex.handle().now().duration_since(before) >= Duration::from_millis(10) {
            assert_eq!(info.os_waits, 1);
            waited = true;
            break;
        }
    }
    assert!(waited, "idle pooled connection spun instead of parking");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 1001);
}

#[path = "support/scram.rs"]
mod scram;

async fn sasl_message<S: Stream>(s: &mut S) -> Vec<u8> {
    let mut header = [0; 5];
    exact(s, &mut header).await;
    assert_eq!(header[0], b'p');
    let len = u32::from_be_bytes(header[1..].try_into().expect("length")) as usize;
    assert!((4..4096).contains(&len));
    let mut body = vec![0; len - 4];
    exact(s, &mut body).await;
    body
}
async fn sasl_auth<S: Stream>(s: &mut S, code: u32, body: &[u8]) {
    let packet = [
        b"R".as_slice(),
        &((body.len() + 8) as u32).to_be_bytes(),
        &code.to_be_bytes(),
        body,
    ]
    .concat();
    write_all(s, &packet).await.expect("SCRAM challenge");
}

#[derive(Clone, Copy, Debug)]
enum BindingCase {
    Derived,
    Override,
    Unsupported,
    RequiredUnsupported,
    Required,
}

struct DropObserved<S> {
    stream: S,
    drops: std::rc::Rc<std::cell::Cell<usize>>,
}
impl<S> Drop for DropObserved<S> {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}
impl<S: Stream> AsyncRead for DropObserved<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, bytes)
    }
}
impl<S: Stream> AsyncWrite for DropObserved<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_close(cx)
    }
}

async fn scram_server<S: Stream>(s: &mut S, binding: Option<&[u8]>) {
    use base64::Engine;
    startup_packet(s).await;
    sasl_auth(s, 10, b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0").await;
    let initial = sasl_message(s).await;
    let end = initial.iter().position(|b| *b == 0).expect("mechanism NUL");
    assert_eq!(
        &initial[..end],
        if binding.is_some() {
            b"SCRAM-SHA-256-PLUS".as_slice()
        } else {
            b"SCRAM-SHA-256"
        }
    );
    let len =
        u32::from_be_bytes(initial[end + 1..end + 5].try_into().expect("SASL length")) as usize;
    let first = std::str::from_utf8(&initial[end + 5..]).expect("client first");
    assert_eq!(len, first.len());
    let gs2 = if binding.is_some() {
        "p=tls-server-end-point,,"
    } else {
        "n,,"
    };
    let bare = first.strip_prefix(gs2).expect("exact GS2 flag");
    let nonce = bare.strip_prefix("n=,r=").expect("client nonce");
    assert!(!nonce.is_empty());
    let server_first = format!("r={nonce}server,s=c2FsdA==,i=4096");
    sasl_auth(s, 11, server_first.as_bytes()).await;
    let final_bytes = sasl_message(s).await;
    let final_message = std::str::from_utf8(&final_bytes).expect("client final");
    let (without_proof, proof) = final_message.split_once(",p=").expect("proof");
    let (cbind, final_nonce) = without_proof.split_once(",r=").expect("cbind and nonce");
    assert_eq!(final_nonce, format!("{nonce}server"));
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(cbind.strip_prefix("c=").expect("cbind"))
        .expect("base64");
    let mut expected = gs2.as_bytes().to_vec();
    expected.extend_from_slice(binding.unwrap_or_default());
    assert_eq!(
        decoded, expected,
        "cbind must contain exact certificate digest/override"
    );
    let auth = format!("{bare},{server_first},{without_proof}");
    let salted = scram::hex_salted_password();
    let client_key = scram::hmac(&salted, b"Client Key");
    let signature = scram::hmac(&scram::sha256(&client_key), auth.as_bytes());
    let expected_proof: Vec<_> = client_key
        .iter()
        .zip(signature)
        .map(|(k, s)| k ^ s)
        .collect();
    assert_eq!(
        proof,
        scram::base64(&expected_proof),
        "authenticated client proof"
    );
    let server_signature = scram::hmac(&scram::hmac(&salted, b"Server Key"), auth.as_bytes());
    sasl_auth(
        s,
        12,
        format!("v={}", scram::base64(&server_signature)).as_bytes(),
    )
    .await;
    write_all(s, AUTH).await.expect("authenticated ready");
    assert_eq!(query(s).await, b"SELECT 42\0");
    write_all(s, RESULT).await.expect("result");
}

#[test]
fn tls_scram_uses_verified_leaf_override_or_unsupported_and_enforces_required() {
    use turnloop_tls::{
        ClientConfig, ClientOptions, ServerConfig, TlsStream, asynchronous::ClientTls,
        rustls::pki_types::PrivatePkcs8KeyDer,
    };
    const NOW: u64 = 1_789_344_000;
    let mut ran = 0;
    for case in [
        BindingCase::Derived,
        BindingCase::Override,
        BindingCase::Unsupported,
        BindingCase::RequiredUnsupported,
        BindingCase::Required,
    ] {
        let unsupported = matches!(
            case,
            BindingCase::Unsupported | BindingCase::RequiredUnsupported
        );
        let key = rcgen::KeyPair::generate_for(if unsupported {
            &rcgen::PKCS_ED25519
        } else {
            &rcgen::PKCS_ECDSA_P256_SHA256
        })
        .expect("key");
        let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
            .expect("params")
            .self_signed(&key)
            .expect("certificate");
        let der = cert.der().clone();
        let expected_digest = scram::sha256(der.as_ref());
        let binding = match case {
            BindingCase::Override => Some(vec![0xa5; 48]),
            BindingCase::Unsupported | BindingCase::RequiredUnsupported => None,
            _ => Some(expected_digest.to_vec()),
        };
        let server_config = ServerConfig::new(
            vec![der.clone()],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            vec![],
            NOW,
        )
        .expect("server config");
        let client_tls = ClientTls {
            config: ClientConfig::new(
                ClientOptions {
                    ca: Some(vec![der.clone()]),
                    ..Default::default()
                },
                NOW,
            )
            .expect("client config"),
            server_name: "localhost".try_into().expect("name"),
            unix_seconds: NOW,
        };
        let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
        let h = ex.handle();
        let server_h = h.clone();
        let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
        let address = listener.local_addr().expect("address");
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let client_drops = drops.clone();
        let mut server = ex.spawn_local(async move {
            let at = server_h.now() + Duration::from_secs(10);
            let mut socket = listener.accept().await.expect("accept");
            let mut request = [0; 8];
            exact(&mut socket, &mut request).await;
            assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
            write_all(&mut socket, b"S").await.expect("SSL accepted");
            let mut tls = TlsStream::accept(socket, &server_config, &server_h, at, NOW).await.expect("verified TLS");
            assert!(tls.peer_certificates().is_none(), "no client certificate was requested");
            if matches!(case, BindingCase::RequiredUnsupported) {
                let result = read(&mut tls, &mut [0; 1]).await;
                // The rejected client's drop can surface as EOF/reset or WASI
                // last-operation-failed (which has no portable OS error kind).
                // Require zero startup bytes here; below, independently require
                // the exact binding error and one actual owned-stream drop.
                assert!(matches!(result, Ok(0) | Err(_)), "startup data after required binding failure: {result:?}");
            } else {
                scram_server(&mut tls, binding.as_deref()).await;
                // Flush the result, then keep the connection alive until client drop.
                let result = read(&mut tls, &mut [0; 1]).await;
                assert!(matches!(result, Ok(0)) || matches!(result, Err(ref e) if matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset)), "client closes: {result:?}");
            }
            1
        }).expect("server");
        let mut client = ex
            .spawn_local(async move {
                count::prove_counter().await;
                let (digests, allocations) = count::measure(async {
                    for _ in 0..1000 {
                        let digest = turnloop_tls::tls_server_end_point(der.as_ref());
                        if unsupported {
                            assert!(digest.is_none());
                        } else {
                            assert_eq!(digest.expect("binding").as_ref(), expected_digest);
                        }
                    }
                    1000
                })
                .await;
                assert_eq!(digests, 1000);
                assert_eq!(
                    allocations, 0,
                    "certificate binding allocates no scratch storage"
                );
                let at = h.now() + Duration::from_secs(10);
                let options = ConnectOptions {
                    address,
                    protocol: turnloop_postgres::Config {
                        password: b"secret".to_vec(),
                        ssl: turnloop_postgres::SslMode::Require,
                        channel_binding_required: matches!(
                            case,
                            BindingCase::Required | BindingCase::RequiredUnsupported
                        ),
                        ..Default::default()
                    },
                    tls: Some(client_tls),
                    channel_binding: matches!(case, BindingCase::Override).then(|| vec![0xa5; 48]),
                };
                if matches!(case, BindingCase::RequiredUnsupported) {
                    let stream = h
                        .connect(address, Default::default())
                        .await
                        .expect("connect");
                    let result = Client::from_stream(
                        &h,
                        DropObserved {
                            stream,
                            drops: client_drops.clone(),
                        },
                        options.protocol,
                        options.tls.as_ref(),
                        None,
                        at,
                    )
                    .await;
                    let error = result.err().expect("required binding must fail");
                    assert_eq!(
                        client_drops.get(),
                        1,
                        "failed connection must drop its stream once"
                    );
                    assert!(
                        error.to_string().contains(
                            "channel binding required but certificate binding is unavailable"
                        ),
                        "{error}"
                    );
                } else {
                    let mut c = Client::connect(&h, &options, at)
                        .await
                        .expect("TLS SCRAM login");
                    let mut rows = 0;
                    assert_eq!(
                        c.query("SELECT 42", at, |event| {
                            if let Event::Row { mut row, .. } = event {
                                assert_eq!(
                                    row.next().expect("row").expect("value"),
                                    Some(b"42".as_slice())
                                );
                                rows += 1;
                            }
                            Ok(())
                        })
                        .await
                        .expect("query"),
                        Outcome::Success
                    );
                    assert_eq!(rows, 1);
                }
            })
            .expect("client");
        drive(&mut ex, &mut client);
        ran += drive(&mut ex, &mut server);
        assert_eq!(
            drops.get(),
            usize::from(matches!(case, BindingCase::RequiredUnsupported))
        );
    }
    assert_eq!(ran, 5, "all TLS SCRAM cases must run");
}
