#![cfg(not(target_arch = "wasm32"))]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime},
};
use turnloop_smtp::{
    Auth, Capabilities, Config, Connection, Envelope, Event, State, Tls, encode_data,
    message::{self, FileAttachment, Mail},
};
fn discard(c: &mut Connection) {
    c.consume_output(c.output().len());
}
fn plain_ready(pipeline: bool) -> Connection {
    let mut c = Connection::new(Config {
        tls: Tls::None,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.connected(Instant::now())
        .expect("fixture operation must succeed");
    c.receive(b"220 test\r\n", Instant::now())
        .expect("fixture operation must succeed");
    discard(&mut c);
    c.receive(
        if pipeline {
            b"250-test\r\n250-PIPELINING\r\n250-SIZE 10000\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n"
        } else {
            b"250 test\r\n"
        },
        Instant::now(),
    )
    .expect("fixture operation must succeed");
    assert_eq!(c.poll_event(), Some(Event::Ready));
    c
}
fn envelope() -> Envelope {
    Envelope {
        from: "a@example.test".into(),
        to: vec!["ok@example.test".into(), "bad@example.test".into()],
    }
}
#[test]
fn normalization_and_dot_stuffing() {
    let mut out = Vec::new();
    let size = encode_data(b".first\nsecond\rthird\r\n..last", &mut out);
    assert_eq!(out, b"..first\r\nsecond\r\nthird\r\n...last\r\n.\r\n");
    assert_eq!(size, b".first\r\nsecond\r\nthird\r\n..last\r\n".len());
    out.clear();
    assert_eq!(encode_data(b"", &mut out), 0);
    assert_eq!(out, b".\r\n");
    out.clear();
    encode_data(b"x\r\n", &mut out);
    assert_eq!(out, b"x\r\n.\r\n");
}
#[test]
fn ehlo_fallback_required_tls_and_deadline() {
    let now = Instant::now();
    let mut c = Connection::new(Config {
        tls: Tls::Required,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.connected(now).expect("fixture operation must succeed");
    let deadline = c.next_timeout().expect("fixture operation must succeed");
    assert_eq!(deadline, now + Duration::from_secs(30));
    for b in b"220 hi\r\n" {
        c.receive(&[*b], now)
            .expect("fixture operation must succeed");
    }
    assert!(c.output().starts_with(b"EHLO"));
    discard(&mut c);
    c.receive(b"502 unsupported\r\n", now)
        .expect("fixture operation must succeed");
    assert!(c.output().starts_with(b"HELO"));
    discard(&mut c);
    c.receive(b"250 hi\r\n", now)
        .expect("fixture operation must succeed");
    assert!(matches!(c.poll_event(), Some(Event::Failed { error, .. }) if error.code == "ETLS"));
    assert_eq!(c.state(), State::Closed);
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    c.connected(now).expect("fixture operation must succeed");
    c.handle_timeout(now + Duration::from_secs(30));
    assert!(
        matches!(c.poll_event(), Some(Event::Failed { error, .. }) if error.code == "ETIMEDOUT" && error.message == "Greeting never received")
    );
    c.handle_timeout(now + Duration::from_secs(31));
    assert_eq!(c.poll_event(), Some(Event::CloseTransport));
    assert_eq!(c.poll_event(), Some(Event::Closed));
    assert_eq!(c.poll_event(), None);
}
#[test]
fn malformed_multiline_and_injection_are_rejected() {
    let now = Instant::now();
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    c.connected(now).expect("fixture operation must succeed");
    assert!(c.receive(b"220-a\r\n221 b\r\n", now).is_err());
    assert_eq!(c.state(), State::Closed);
    assert!(
        Connection::new(Config {
            name: "x\r\nMAIL FROM:<attacker>".into(),
            ..Config::default()
        })
        .is_err()
    );
    let mut c = plain_ready(true);
    let mut e = envelope();
    e.from = "a\r\nDATA".into();
    assert!(c.send(1, e, "id".into(), b"body", now).is_err());
    assert!(c.output().is_empty());
}
#[test]
fn capability_enforcement_all_rejected_and_mail_failure_drain() {
    let now = Instant::now();
    let mut c = plain_ready(false);
    assert_eq!(c.capabilities(), &Capabilities::default());
    assert_eq!(
        c.send(1, envelope(), "id".into(), "ü".as_bytes(), now)
            .unwrap_err()
            .code,
        "EMESSAGE"
    );
    let mut c = plain_ready(true);
    c.send(2, envelope(), "id".into(), b"body", now)
        .expect("fixture operation must succeed");
    discard(&mut c);
    c.receive(
        b"550 bad sender\r\n250 would accept\r\n550 bad recipient\r\n",
        now,
    )
    .expect("fixture operation must succeed");
    assert!(
        matches!(c.poll_event(), Some(Event::Failed { token: Some(2), error, .. }) if error.command == "MAIL FROM" && error.response_code == Some(550))
    );
    assert_eq!(c.output(), b"RSET\r\n");
    discard(&mut c);
    c.receive(b"250 reset\r\n", now)
        .expect("fixture operation must succeed");
    assert_eq!(c.state(), State::Ready);
    assert_eq!(c.poll_event(), None);
    c.send(3, envelope(), "id".into(), b"body", now)
        .expect("fixture operation must succeed");
    discard(&mut c);
    c.receive(b"250 mail\r\n550 no\r\n551 no\r\n", now)
        .expect("fixture operation must succeed");
    assert!(
        matches!(c.poll_event(), Some(Event::Failed { token: Some(3), error, .. }) if error.message.contains("all recipients"))
    );
    assert_eq!(c.output(), b"RSET\r\n");
}
#[test]
fn mime_builder_supplies_date_id_alternatives_attachments_and_headers() {
    let built = message::build(Mail {
        from: "Sender <sender@example.test>"
            .parse()
            .expect("fixture operation must succeed"),
        to: vec![
            "to@example.test"
                .parse()
                .expect("fixture operation must succeed"),
        ],
        cc: vec![],
        bcc: vec![
            "blind@example.test"
                .parse()
                .expect("fixture operation must succeed"),
        ],
        subject: "Unicode ✓".into(),
        text: Some("plain text".into()),
        html: Some("<b>HTML</b>".into()),
        attachments: vec![FileAttachment {
            filename: "binary.bin".into(),
            content_type: "application/octet-stream".into(),
            content: vec![0, 1, 2, 255],
        }],
        headers: vec![("X-Lane".into(), "smtp".into())],
        date: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        message_id: "<fixed@example.test>".into(),
        boundary_seed: "123abc".into(),
    })
    .expect("fixture operation must succeed");
    let bytes = built.formatted();
    let text = String::from_utf8(bytes).expect("fixture operation must succeed");
    assert!(text.contains("Date: Tue, 14 Nov 2023 22:13:20 +0000\r\n"));
    assert!(text.contains("Message-ID: <fixed@example.test>"));
    assert!(text.contains("multipart/mixed"));
    assert!(text.contains("multipart/alternative"));
    assert!(text.contains("AAEC/w=="));
    assert!(text.contains("X-Lane: smtp"));
    assert!(text.contains("plain text"));
    assert!(text.contains("<b>HTML</b>"));
    assert!(!text.contains("Bcc:"));
    assert_eq!(built.envelope().to().len(), 2);
}
trait Socket: Read + Write {}
impl<T: Read + Write> Socket for T {}
fn line(socket: &mut dyn Socket) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = [0];
        socket
            .read_exact(&mut b)
            .expect("fixture operation must succeed");
        out.push(b[0]);
        if out.ends_with(b"\r\n") {
            return out;
        }
        assert!(out.len() < 65536);
    }
}
fn roots() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            include_bytes!("fixtures/ca.der").to_vec(),
        ))
        .expect("fixture operation must succeed");
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}
fn server_tls(stream: TcpStream) -> rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    let cert =
        rustls::pki_types::CertificateDer::from(include_bytes!("fixtures/server.der").to_vec());
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("fixtures/server-key.der").to_vec(),
    ));
    let cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("fixture operation must succeed");
    rustls::StreamOwned::new(
        rustls::ServerConnection::new(Arc::new(cfg)).expect("fixture operation must succeed"),
        stream,
    )
}
#[derive(Clone, Copy, Debug)]
enum Mode {
    StartTls,
    Implicit,
    Login,
    OAuth,
    AuthFail,
}
fn test_socket(mode: Mode) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("fixture operation must succeed");
    let addr = listener
        .local_addr()
        .expect("fixture operation must succeed");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("fixture operation must succeed");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fixture operation must succeed");
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("fixture operation must succeed");
        let mut transcript = Vec::new();
        let mut socket: Box<dyn Socket> = if matches!(mode, Mode::Implicit) {
            Box::new(server_tls(stream))
        } else if matches!(mode, Mode::StartTls) {
            let mut stream = stream;
            stream
                .write_all(b"220 test ESMTP\r\n")
                .expect("fixture operation must succeed");
            let ehlo = line(&mut stream);
            assert!(ehlo.starts_with(b"EHLO "));
            transcript.extend(ehlo);
            stream
                .write_all(b"250-test\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n")
                .expect("fixture operation must succeed");
            let start = line(&mut stream);
            assert_eq!(start, b"STARTTLS\r\n");
            transcript.extend(start);
            stream
                .write_all(b"220 ready for TLS\r\n")
                .expect("fixture operation must succeed");
            Box::new(server_tls(stream))
        } else {
            Box::new(stream)
        };
        if !matches!(mode, Mode::StartTls) {
            socket
                .write_all(b"220 test ESMTP\r\n")
                .expect("fixture operation must succeed");
            socket.flush().expect("fixture operation must succeed");
        }
        let ehlo = line(&mut *socket);
        assert!(ehlo.starts_with(b"EHLO "));
        transcript.extend(ehlo);
        socket.write_all(b"250-test\r\n250-PIPELINING\r\n250-SIZE 10000\r\n250-8BITMIME\r\n250-SMTPUTF8\r\n250 AUTH PLAIN LOGIN XOAUTH2\r\n").expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        let auth = line(&mut *socket);
        transcript.extend(&auth);
        match mode {
            Mode::Login => {
                assert_eq!(auth, b"AUTH LOGIN\r\n");
                socket
                    .write_all(b"334 VXNlcm5hbWU6\r\n")
                    .expect("fixture operation must succeed");
                socket.flush().expect("fixture operation must succeed");
                assert_eq!(line(&mut *socket), b"dXNlcg==\r\n");
                socket
                    .write_all(b"334 UGFzc3dvcmQ6\r\n")
                    .expect("fixture operation must succeed");
                socket.flush().expect("fixture operation must succeed");
                assert_eq!(line(&mut *socket), b"cGFzcw==\r\n");
            }
            Mode::OAuth => {
                assert_eq!(
                    auth,
                    b"AUTH XOAUTH2 dXNlcj11c2VyAWF1dGg9QmVhcmVyIHRva2VuAQE=\r\n"
                );
            }
            _ => {
                assert_eq!(auth, b"AUTH PLAIN AHVzZXIAcGFzcw==\r\n");
            }
        }
        if matches!(mode, Mode::AuthFail) {
            socket
                .write_all(b"535 5.7.8 Invalid credentials\r\n")
                .expect("fixture operation must succeed");
            socket.flush().expect("fixture operation must succeed");
            return transcript;
        }
        socket
            .write_all(b"235 2.7.0 authenticated\r\n")
            .expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        let mail = line(&mut *socket);
        assert!(mail.starts_with(b"MAIL FROM:<a@example.test> SIZE="));
        transcript.extend(mail);
        // Read all envelope commands before responding: enforces actual PIPELINING.
        let rcpt1 = line(&mut *socket);
        assert_eq!(rcpt1, b"RCPT TO:<ok@example.test>\r\n");
        transcript.extend(rcpt1);
        let rcpt2 = line(&mut *socket);
        assert_eq!(rcpt2, b"RCPT TO:<bad@example.test>\r\n");
        transcript.extend(rcpt2);
        socket
            .write_all(b"250 mail\r\n250 accepted\r\n550 5.1.1 rejected\r\n")
            .expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        assert_eq!(line(&mut *socket), b"DATA\r\n");
        socket
            .write_all(b"354 go ahead\r\n")
            .expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        let mut body = Vec::new();
        loop {
            let l = line(&mut *socket);
            if l == b".\r\n" {
                break;
            }
            body.extend(l);
        }
        assert_eq!(body, b"Subject: test\r\n\r\n..hello\r\nlast\r\n");
        transcript.extend(body);
        socket
            .write_all(b"250 2.0.0 queued as TEST123\r\n")
            .expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        assert_eq!(line(&mut *socket), b"RSET\r\n");
        socket
            .write_all(b"250 reset\r\n")
            .expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        assert_eq!(line(&mut *socket), b"QUIT\r\n");
        socket
            .write_all(b"221 bye\r\n")
            .expect("fixture operation must succeed");
        socket.flush().expect("fixture operation must succeed");
        transcript
    });
    let auth = match mode {
        Mode::Login => Auth::Login {
            user: "user".into(),
            password: "pass".into(),
        },
        Mode::OAuth => Auth::Xoauth2 {
            user: "user".into(),
            access_token: "token".into(),
        },
        _ => Auth::Plain {
            user: "user".into(),
            password: "pass".into(),
        },
    };
    let tls = match mode {
        Mode::StartTls => Tls::Required,
        Mode::Implicit => Tls::Implicit,
        _ => Tls::None,
    };
    let mut c = Connection::new(Config {
        auth: Some(auth),
        tls,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    let stream = TcpStream::connect(addr).expect("fixture operation must succeed");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fixture operation must succeed");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("fixture operation must succeed");
    // Preserve the TcpStream until an upgrade event, then move it into rustls.
    enum Wire {
        Plain(TcpStream),
        Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    }
    impl Read for Wire {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            match self {
                Self::Plain(s) => s.read(b),
                Self::Tls(s) => s.read(b),
            }
        }
    }
    impl Write for Wire {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            match self {
                Self::Plain(s) => s.write(b),
                Self::Tls(s) => s.write(b),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            match self {
                Self::Plain(s) => s.flush(),
                Self::Tls(s) => s.flush(),
            }
        }
    }
    let mut wire = Some(Wire::Plain(stream));
    c.connected(Instant::now())
        .expect("fixture operation must succeed");
    let mut sent = false;
    let mut failed = false;
    loop {
        if let Some(e) = c.poll_event() {
            match e {
                Event::UpgradeTls => {
                    let Some(Wire::Plain(stream)) = wire.take() else {
                        panic!("second TLS upgrade")
                    };
                    let mut tls = rustls::StreamOwned::new(
                        rustls::ClientConnection::new(
                            roots(),
                            "localhost"
                                .try_into()
                                .expect("fixture operation must succeed"),
                        )
                        .expect("fixture operation must succeed"),
                        stream,
                    );
                    while tls.conn.is_handshaking() {
                        tls.conn
                            .complete_io(&mut tls.sock)
                            .expect("fixture operation must succeed");
                    }
                    wire = Some(Wire::Tls(Box::new(tls)));
                    c.tls_established(Instant::now())
                        .expect("fixture operation must succeed");
                }
                Event::Ready => {
                    assert!(c.capabilities().pipelining);
                    c.send(
                        7,
                        envelope(),
                        "<id@example.test>".into(),
                        b"Subject: test\n\n.hello\rlast",
                        Instant::now(),
                    )
                    .expect("fixture operation must succeed");
                }
                Event::Sent { token, info } => {
                    assert_eq!(token, 7);
                    assert_eq!(info.accepted, ["ok@example.test"]);
                    assert_eq!(info.rejected.len(), 1);
                    assert_eq!(info.rejected[0].recipient, "bad@example.test");
                    assert_eq!(info.rejected[0].error.response_code, Some(550));
                    assert_eq!(info.response, "250 2.0.0 queued as TEST123");
                    assert_eq!(info.envelope, envelope());
                    assert_eq!(info.message_id, "<id@example.test>");
                    sent = true;
                    c.reset(Instant::now())
                        .expect("fixture operation must succeed");
                }
                Event::Reset => {
                    c.quit(Instant::now())
                        .expect("fixture operation must succeed");
                }
                Event::Failed { token, error, .. } => {
                    assert!(matches!(mode, Mode::AuthFail));
                    assert_eq!(token, None);
                    assert_eq!(error.code, "EAUTH");
                    assert_eq!(error.response_code, Some(535));
                    assert_eq!(error.command, "AUTH PLAIN");
                    assert_eq!(
                        error.message,
                        "Invalid login: 535 5.7.8 Invalid credentials"
                    );
                    failed = true;
                }
                Event::CloseTransport => {}
                Event::Closed => break,
            }
            continue;
        }
        let w = wire.as_mut().expect("fixture operation must succeed");
        w.write_all(c.output())
            .expect("fixture operation must succeed");
        w.flush().expect("fixture operation must succeed");
        discard(&mut c);
        let mut b = [0; 4096];
        let n = w.read(&mut b).expect("fixture operation must succeed");
        assert!(n > 0);
        c.receive(&b[..n], Instant::now())
            .expect("fixture operation must succeed");
    }
    assert_eq!(sent, !matches!(mode, Mode::AuthFail));
    assert_eq!(failed, matches!(mode, Mode::AuthFail));
    let transcript = server.join().expect("fixture operation must succeed");
    assert!(transcript.windows(4).any(|w| w == b"AUTH"));
}
#[test]
fn socket_starttls_plain_auth_and_partial_reject() {
    test_socket(Mode::StartTls);
}
#[test]
fn socket_implicit_tls() {
    test_socket(Mode::Implicit);
}
#[test]
fn socket_login() {
    test_socket(Mode::Login);
}
#[test]
fn socket_xoauth2() {
    test_socket(Mode::OAuth);
}
#[test]
fn socket_auth_failure() {
    test_socket(Mode::AuthFail);
}

#[test]
#[ignore = "private SMTP sink: scripts/test-servers.py run cargo test --workspace -- --include-ignored"]
fn installed_postfix_smtp_sink_records_message() {
    use std::{fs, path::PathBuf};
    let port: u16 = std::env::var("TURNLOOP_TEST_SMTP_PORT")
        .expect("run scripts/test-servers.py run")
        .parse()
        .expect("SMTP port");
    let root =
        PathBuf::from(std::env::var_os("TURNLOOP_TEST_SMTP_TOOLS").expect("SMTP dump directory"));
    let mut socket = TcpStream::connect(("127.0.0.1", port)).expect("connect private SMTP sink");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fixture operation must succeed");
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("fixture operation must succeed");
    let mut c = Connection::new(Config {
        tls: Tls::None,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.connected(Instant::now())
        .expect("fixture operation must succeed");
    let mut sent = false;
    loop {
        if let Some(event) = c.poll_event() {
            match event {
                Event::Ready => c
                    .send(
                        88,
                        Envelope {
                            from: "sender@example.test".into(),
                            to: vec!["recipient@example.test".into()],
                        },
                        "<sink@example.test>".into(),
                        b"Subject: real Postfix\r\n\r\n.turnloop sink payload\r\n",
                        Instant::now(),
                    )
                    .expect("fixture operation must succeed"),
                Event::Sent { token, info } => {
                    assert_eq!(token, 88);
                    assert_eq!(info.accepted, ["recipient@example.test"]);
                    sent = true;
                    c.quit(Instant::now())
                        .expect("fixture operation must succeed");
                }
                Event::CloseTransport => {}
                Event::Closed => break,
                event => panic!("Unexpected {event:?}"),
            }
            continue;
        }
        socket
            .write_all(c.output())
            .expect("fixture operation must succeed");
        discard(&mut c);
        let mut bytes = [0; 4096];
        let n = socket
            .read(&mut bytes)
            .expect("fixture operation must succeed");
        assert!(n > 0);
        c.receive(&bytes[..n], Instant::now())
            .expect("fixture operation must succeed");
    }
    assert!(sent);
    let dumps: Vec<_> = fs::read_dir(&root)
        .expect("fixture operation must succeed")
        .map(|e| e.expect("fixture operation must succeed").path())
        .filter(|p| {
            p.file_name()
                .expect("fixture operation must succeed")
                .to_string_lossy()
                .starts_with("message-")
        })
        .collect();
    assert_eq!(dumps.len(), 1);
    let message = fs::read(&dumps[0]).expect("fixture operation must succeed");
    assert!(
        message
            .windows(b".turnloop sink payload".len())
            .any(|w| w == b".turnloop sink payload")
    );
    assert!(
        message
            .windows(b"Subject: real Postfix".len())
            .any(|w| w == b"Subject: real Postfix")
    );
}

#[test]
fn partial_greeting_keeps_deadline_and_data_failure_has_node_error_shape() {
    let now = Instant::now();
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    c.connected(now).expect("fixture operation must succeed");
    c.receive(b"220 partial", now + Duration::from_secs(29))
        .expect("fixture operation must succeed");
    assert_eq!(c.next_timeout(), Some(now + Duration::from_secs(30)));
    c.handle_timeout(now + Duration::from_secs(30));
    assert_eq!(c.state(), State::Closed);
    let mut c = plain_ready(true);
    c.send(9, envelope(), "id".into(), b"body", now)
        .expect("fixture operation must succeed");
    discard(&mut c);
    c.receive(b"250 mail\r\n250 accepted\r\n550 denied\r\n", now)
        .expect("fixture operation must succeed");
    discard(&mut c);
    c.receive(b"354 send\r\n", now)
        .expect("fixture operation must succeed");
    discard(&mut c);
    c.receive(b"554 content rejected\r\n", now)
        .expect("fixture operation must succeed");
    assert!(
        matches!(c.poll_event(), Some(Event::Failed { token: Some(9), error, envelope: Some(e), accepted, rejected }) if error.code == "EMESSAGE" && error.command == "DATA" && error.message == "Message failed: 554 content rejected" && e == envelope() && accepted == ["ok@example.test"] && rejected.len() == 1)
    );
    c.close();
    c.close();
    let events: Vec<_> = std::iter::from_fn(|| c.poll_event()).collect();
    assert_eq!(events, [Event::CloseTransport, Event::Closed]);
}
