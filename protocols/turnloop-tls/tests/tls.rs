#![cfg(not(target_arch = "wasm32"))]
mod support;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use support::*;
use turnloop_tls::{
    ClientConfig, ClientOptions,
    rustls::{self, pki_types::ServerName},
};

#[test]
fn sockets_alpn_sni_payload_and_resumption() {
    let cert = certificate();
    let server = server_config(&cert);
    let client = ClientConfig::new(
        ClientOptions {
            extra_ca_pem: cert.cert.pem().into_bytes(),
            alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        for _ in 0..2 {
            let mut stream = Stream::new(server.accept().unwrap(), listener.accept().unwrap().0);
            let mut request = [0; 4];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            assert_eq!(stream.engine.alpn_protocol(), Some(b"h2".as_slice()));
            stream.write_all(b"pong").unwrap();
        }
    });
    for index in 0..2 {
        let mut stream = Stream::new(
            client
                .connect(ServerName::try_from("localhost").unwrap())
                .unwrap(),
            TcpStream::connect(address).unwrap(),
        );
        stream.write_all(b"ping").unwrap();
        let mut response = [0; 4];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        assert_eq!(stream.engine.alpn_protocol(), Some(b"h2".as_slice()));
        assert_eq!(
            stream.engine.handshake_kind(),
            Some(if index == 0 {
                rustls::HandshakeKind::Full
            } else {
                rustls::HandshakeKind::Resumed
            })
        );
    }
    task.join().unwrap();
}

fn attempt(options: ClientOptions, name: &'static str) -> Result<(), String> {
    let cert = certificate();
    let config = server_config(&cert);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        let mut stream = Stream::new(config.accept().unwrap(), listener.accept().unwrap().0);
        let mut data = [0];
        let _ = stream.read_exact(&mut data);
    });
    let mut stream = Stream::new(
        ClientConfig::new(options, NOW)
            .unwrap()
            .connect(ServerName::try_from(name).unwrap())
            .unwrap(),
        TcpStream::connect(address).unwrap(),
    );
    let result = stream.write_all(b"x").map_err(|e| {
        let tls = e
            .get_ref()
            .and_then(|e| e.downcast_ref::<rustls::Error>())
            .unwrap();
        turnloop_tls::node_error_code(tls).to_string()
    });
    drop(stream);
    task.join().unwrap();
    result
}
#[test]
fn untrusted_chain_and_explicit_insecure_option() {
    assert_eq!(
        attempt(ClientOptions::default(), "localhost").unwrap_err(),
        "UNABLE_TO_VERIFY_LEAF_SIGNATURE"
    );
    attempt(
        ClientOptions {
            reject_unauthorized: false,
            ..Default::default()
        },
        "wrong.invalid",
    )
    .unwrap();
}
#[test]
fn injected_deadline_is_terminal_once() {
    let mut client = ClientConfig::new(ClientOptions::default(), NOW)
        .unwrap()
        .connect(ServerName::try_from("localhost").unwrap())
        .unwrap();
    let now = Instant::now();
    client.set_handshake_deadline(Some(now + Duration::from_secs(1)));
    assert_eq!(client.handle_timeout(now), None);
    assert_eq!(
        client.handle_timeout(now + Duration::from_secs(1)),
        Some("ERR_TLS_HANDSHAKE_TIMEOUT")
    );
    assert_eq!(client.handle_timeout(now + Duration::from_secs(2)), None);
    assert!(client.process(&mut [], NOW).state.is_err());
}

fn verify_generated(cert: rcgen::CertifiedKey, name: &'static str) -> Result<(), String> {
    let options = ClientOptions {
        ca: Some(vec![cert.cert.der().clone()]),
        ..Default::default()
    };
    let server = server_config(&cert);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        let mut stream = Stream::new(server.accept().unwrap(), listener.accept().unwrap().0);
        let mut byte = [0];
        if stream.read_exact(&mut byte).is_ok() {
            assert_eq!(byte, [42]);
        }
    });
    let mut stream = Stream::new(
        ClientConfig::new(options, NOW)
            .unwrap()
            .connect(ServerName::try_from(name).unwrap())
            .unwrap(),
        TcpStream::connect(address).unwrap(),
    );
    let result = stream.write_all(&[42]).map_err(|e| {
        turnloop_tls::node_error_code(
            e.get_ref()
                .unwrap()
                .downcast_ref::<rustls::Error>()
                .unwrap(),
        )
        .to_owned()
    });
    drop(stream);
    task.join().unwrap();
    result
}
#[test]
fn certificate_name_expiry_and_not_yet_valid_codes() {
    assert_eq!(
        verify_generated(certificate(), "wrong.invalid").unwrap_err(),
        "ERR_TLS_CERT_ALTNAME_INVALID"
    );
    for (start, end, code) in [
        (2010, 2020, "CERT_HAS_EXPIRED"),
        (2030, 2040, "CERT_NOT_YET_VALID"),
    ] {
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        params.not_before = rcgen::date_time_ymd(start, 1, 1);
        params.not_after = rcgen::date_time_ymd(end, 1, 1);
        let key_pair = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        assert_eq!(
            verify_generated(rcgen::CertifiedKey { cert, key_pair }, "localhost").unwrap_err(),
            code
        );
    }
}
#[test]
fn ca_chain_and_sni_enabled_or_disabled() {
    let mut root_params = rcgen::CertificateParams::new(vec!["Test Root".into()]).unwrap();
    root_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let root_key = rcgen::KeyPair::generate().unwrap();
    let root = root_params.self_signed(&root_key).unwrap();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&leaf_key, &root, &root_key)
        .unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into(),
    )
    .unwrap();
    let server = Arc::new(server);
    for enabled in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = server.clone();
        let task = thread::spawn(move || {
            let socket = listener.accept().unwrap().0;
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut stream =
                rustls::StreamOwned::new(rustls::ServerConnection::new(server).unwrap(), socket);
            let mut payload = [0; 4];
            stream.read_exact(&mut payload).unwrap();
            assert_eq!(&payload, b"sni?");
            assert_eq!(
                stream.conn.server_name(),
                if enabled { Some("localhost") } else { None }
            );
            stream.write_all(b"okay").unwrap();
        });
        let client = ClientConfig::new(
            ClientOptions {
                extra_ca_pem: root.pem().into_bytes(),
                enable_sni: enabled,
                ..Default::default()
            },
            NOW,
        )
        .unwrap();
        let mut stream = Stream::new(
            client
                .connect(ServerName::try_from("localhost").unwrap())
                .unwrap(),
            TcpStream::connect(address).unwrap(),
        );
        stream.write_all(b"sni?").unwrap();
        let mut reply = [0; 4];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"okay");
        task.join().unwrap();
    }
}

fn provider_with(suite: rustls::SupportedCipherSuite) -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::CryptoProvider {
        cipher_suites: vec![suite],
        ..rustls::crypto::ring::default_provider()
    })
}

/// Handshake over loopback and exchange one byte each way. The server's `Ok`
/// carries the client chain it saw; each `Err` is that side's error text.
type Outcome = (Result<Vec<Vec<u8>>, String>, Result<(), String>);
fn handshake(client: ClientConfig, server: turnloop_tls::ServerConfig) -> Outcome {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        let mut stream = Stream::new(server.accept().unwrap(), listener.accept().unwrap().0);
        let mut byte = [0];
        stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
        assert_eq!(byte, [42]);
        let peer = stream
            .engine
            .peer_certificates()
            .unwrap_or_default()
            .iter()
            .map(|c| c.to_vec())
            .collect();
        stream.write_all(&[43]).map_err(|e| e.to_string())?;
        Ok(peer)
    });
    let mut stream = Stream::new(
        client
            .connect(ServerName::try_from("localhost").unwrap())
            .unwrap(),
        TcpStream::connect(address).unwrap(),
    );
    let mut reply = [0];
    let client = stream
        .write_all(&[42])
        .and_then(|()| stream.read_exact(&mut reply))
        .map(|()| assert_eq!(reply, [43]))
        .map_err(|e| e.to_string());
    drop(stream);
    (task.join().unwrap(), client)
}

#[test]
fn host_selected_crypto_provider_is_the_one_negotiating() {
    use rustls::crypto::ring::cipher_suite::{
        TLS13_AES_128_GCM_SHA256, TLS13_CHACHA20_POLY1305_SHA256,
    };
    let cert = certificate();
    let server = |suite| {
        turnloop_tls::ServerConfig::with_provider(
            provider_with(suite),
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            vec![b"http/1.1".to_vec()],
            NOW,
        )
        .unwrap()
    };
    let client = |suite| {
        ClientConfig::new(
            ClientOptions {
                ca: Some(vec![cert.cert.der().clone()]),
                provider: Some(provider_with(suite)),
                ..Default::default()
            },
            NOW,
        )
        .unwrap()
    };
    // Each side offers only its provider's one suite. Had either side kept the
    // full ring provider, the two would have found a common suite.
    let (server_result, client_result) = handshake(
        client(TLS13_AES_128_GCM_SHA256),
        server(TLS13_CHACHA20_POLY1305_SHA256),
    );
    let server_error = server_result.unwrap_err();
    assert!(
        server_error.contains("NoCipherSuitesInCommon"),
        "{server_error}"
    );
    // The test adapter drops the failed server connection without an alert.
    assert!(client_result.is_err());
    let (server_result, client_result) = handshake(
        client(TLS13_CHACHA20_POLY1305_SHA256),
        server(TLS13_CHACHA20_POLY1305_SHA256),
    );
    assert!(server_result.unwrap().is_empty());
    client_result.unwrap();
}

#[test]
fn from_rustls_carries_client_certificate_authentication() {
    use turnloop_tls::HostTime;
    let server_cert = certificate();
    let client_cert = rcgen::generate_simple_self_signed(vec!["client.test".into()]).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = || {
        let time = HostTime::new(NOW);
        let mut roots = rustls::RootCertStore::empty();
        roots.add(client_cert.cert.der().clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            roots.into(),
            provider.clone(),
        )
        .build()
        .unwrap();
        let config =
            rustls::ServerConfig::builder_with_details(provider.clone(), time.time_provider())
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_client_cert_verifier(verifier)
                .with_single_cert(
                    vec![server_cert.cert.der().clone()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        server_cert.key_pair.serialize_der(),
                    )
                    .into(),
                )
                .unwrap();
        turnloop_tls::ServerConfig::from_rustls(Arc::new(config), time)
    };
    // Deliberately wrong until `process` supplies the host's time.
    let time = HostTime::new(0);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(server_cert.cert.der().clone()).unwrap();
    let config = Arc::new(
        rustls::ClientConfig::builder_with_details(provider.clone(), time.time_provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_client_auth_cert(
                vec![client_cert.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(client_cert.key_pair.serialize_der())
                    .into(),
            )
            .unwrap(),
    );
    let client = ClientConfig::from_rustls(config.clone(), time.clone());
    assert!(Arc::ptr_eq(client.rustls_config(), &config));
    let (server_result, client_result) = handshake(client, server());
    client_result.unwrap();
    assert_eq!(
        server_result.unwrap(),
        vec![client_cert.cert.der().to_vec()]
    );
    // The wrapped config validated the server certificate against the clock
    // that `process` fed, not the 1970 value it started with.
    assert_eq!(time.unix_seconds(), NOW);

    // The same server refuses a client that has no certificate to present.
    let plain = ClientConfig::new(
        ClientOptions {
            ca: Some(vec![server_cert.cert.der().clone()]),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    let (server_result, _) = handshake(plain, server());
    let server_error = server_result.unwrap_err();
    assert!(
        server_error.contains("peer sent no certificates"),
        "{server_error}"
    );
}
