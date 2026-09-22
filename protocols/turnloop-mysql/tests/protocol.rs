use mysql_common::{
    constants::CapabilityFlags as Caps, packets::Column as UpstreamColumn, proto::MySerialize,
};
use turnloop_mysql::*;
fn frame(seq: u8, body: &[u8]) -> Vec<u8> {
    let mut b = (body.len() as u32).to_le_bytes()[..3].to_vec();
    b.push(seq);
    b.extend_from_slice(body);
    b
}
fn flush(c: &mut Connection) -> Vec<u8> {
    let b = c.output().to_vec();
    c.consume_output(b.len())
        .expect("fixture operation must succeed");
    b
}
fn handshake(plugin: &str, extra: Caps) -> Vec<u8> {
    let caps = Caps::CLIENT_PROTOCOL_41
        | Caps::CLIENT_SECURE_CONNECTION
        | Caps::CLIENT_PLUGIN_AUTH
        | Caps::CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA
        | Caps::CLIENT_LONG_PASSWORD
        | Caps::CLIENT_MULTI_RESULTS
        | Caps::CLIENT_PS_MULTI_RESULTS
        | Caps::CLIENT_TRANSACTIONS
        | extra;
    let mut b = vec![10];
    b.extend_from_slice(b"9.6.0\0");
    b.extend_from_slice(&7u32.to_le_bytes());
    b.extend_from_slice(b"12345678\0");
    b.extend_from_slice(&(caps.bits() as u16).to_le_bytes());
    b.push(45);
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&((caps.bits() >> 16) as u16).to_le_bytes());
    b.push(21);
    b.extend_from_slice(&[0; 10]);
    b.extend_from_slice(b"901234567890\0");
    b.extend_from_slice(plugin.as_bytes());
    b.push(0);
    frame(0, &b)
}
fn ok(seq: u8, affected: u8, status: u16) -> Vec<u8> {
    let mut b = vec![0, affected, 0];
    b.extend_from_slice(&status.to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    frame(seq, &b)
}
fn eof(seq: u8, status: u16) -> Vec<u8> {
    let mut b = vec![0xfe, 0, 0];
    b.extend_from_slice(&status.to_le_bytes());
    frame(seq, &b)
}
fn column(seq: u8, t: ColumnType) -> Vec<u8> {
    let col = UpstreamColumn::new(t)
        .with_name(b"answer")
        .with_character_set(45);
    let mut b = Vec::new();
    col.serialize(&mut b);
    frame(seq, &b)
}
fn ready() -> Connection {
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    c.receive(&handshake("mysql_native_password", Caps::empty()))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Progress)
    ));
    let b = flush(&mut c);
    assert_eq!(b[3], 1);
    c.receive(&ok(2, 0, 2))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Connected { connection_id: 7 })
    ));
    c
}
#[test]
fn native_and_caching_fast_full_tls_and_auth_switch() {
    let mut c = Connection::new(Config {
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.receive(&handshake("mysql_native_password", Caps::empty()))
        .expect("fixture operation must succeed");
    c.next_event().expect("fixture operation must succeed");
    let bytes = flush(&mut c);
    assert!(bytes.windows(20).any(|v| v
        == [
            0x0f, 0x8b, 0x90, 0x33, 0xe0, 0x89, 0x7c, 0x0a, 0x83, 0x38, 0xeb, 0xe3, 0xde, 0xa9,
            0x01, 0x0d, 0xda, 0x47, 0xab, 0x56
        ]));
    let mut switch = vec![0xfe];
    switch.extend_from_slice(b"caching_sha2_password\0abcdefghijklmnopqrst\0");
    c.receive(&frame(2, &switch))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Progress)
    ));
    let b = flush(&mut c);
    assert_eq!(b[3], 3);
    assert_eq!(b.len(), 36);
    c.receive(&frame(4, &[1, 3]))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::AuthFastSuccess)
    ));
    c.receive(&ok(5, 0, 2))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Connected { .. })
    ));
    let mut c = Connection::new(Config {
        password: b"secret".to_vec(),
        tls: true,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.receive(&handshake("caching_sha2_password", Caps::CLIENT_SSL))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::UpgradeTls)
    ));
    let ssl = flush(&mut c);
    assert_eq!(ssl.len(), 36);
    assert_eq!(ssl[3], 1);
    assert!(c.query(1, "SELECT 1", None).is_err());
    c.tls_established().expect("fixture operation must succeed");
    assert_eq!(flush(&mut c)[3], 2);
    c.receive(&frame(3, &[1, 4]))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::AuthFull)
    ));
    assert_eq!(flush(&mut c), frame(4, b"secret\0"));
    c.receive(&ok(5, 0, 2))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Connected { .. })
    ));
    let mut c = Connection::new(Config {
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.receive(&handshake("caching_sha2_password", Caps::empty()))
        .expect("fixture operation must succeed");
    c.next_event().expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(2, &[1, 4]))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::AuthFull)
    ));
    assert_eq!(flush(&mut c), frame(3, &[2]));
}
#[test]
fn fragmented_multiset_rows_error_and_prepare_binary() {
    let mut c = ready();
    c.query(1, "SELECT 42; SELECT 99", None)
        .expect("fixture operation must succeed");
    assert_eq!(&flush(&mut c)[4..], b"\x03SELECT 42; SELECT 99");
    let bytes = [
        frame(1, &[1]),
        column(2, ColumnType::MYSQL_TYPE_LONG),
        eof(3, 2),
        frame(4, b"\x0242"),
        eof(5, 10),
        frame(6, &[1]),
        column(7, ColumnType::MYSQL_TYPE_LONG),
        eof(8, 2),
        frame(9, b"\x0299"),
        eof(10, 2),
    ]
    .concat();
    let mut values = Vec::new();
    let mut completed = 0;
    let mut columns = 0;
    for b in bytes {
        c.receive(&[b]).expect("fixture operation must succeed");
        while let Some(e) = c.next_event().expect("fixture operation must succeed") {
            match e {
                Event::Row { row, .. } => {
                    for v in row {
                        let RawValue::Bytes(b) = v.expect("fixture operation must succeed") else {
                            panic!()
                        };
                        values.push(b.to_vec());
                    }
                }
                Event::Column { column, .. } => {
                    assert_eq!(column.name, b"answer");
                    columns += 1;
                }
                Event::Completed { token, outcome } => {
                    assert_eq!(token, 1);
                    assert_eq!(outcome, Outcome::Success);
                    completed += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(values, [b"42", b"99"]);
    assert_eq!(columns, 2);
    assert_eq!(completed, 1);
    c.prepare(2, "SELECT ?", None)
        .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c)[4], 0x16);
    let mut prepare = vec![0];
    prepare.extend_from_slice(&17u32.to_le_bytes());
    prepare.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0]);
    c.receive(
        &[
            frame(1, &prepare),
            column(2, ColumnType::MYSQL_TYPE_LONG),
            eof(3, 2),
            column(4, ColumnType::MYSQL_TYPE_LONG),
            eof(5, 2),
        ]
        .concat(),
    )
    .expect("fixture operation must succeed");
    let mut stmt = None;
    while let Some(e) = c.next_event().expect("fixture operation must succeed") {
        if let Event::Prepared { statement, .. } = e {
            stmt = Some(statement);
        }
    }
    let stmt = stmt.expect("fixture operation must succeed");
    assert_eq!(stmt.parameters, 1);
    c.execute(3, stmt.id, &[Value::Int(42)], None)
        .expect("fixture operation must succeed");
    let execute = flush(&mut c);
    assert_eq!(
        &execute[4..],
        &[
            0x17, 17, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 8, 0, 42, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    c.receive(
        &[
            frame(1, &[1]),
            column(2, ColumnType::MYSQL_TYPE_LONG),
            eof(3, 2),
            frame(4, &[0, 0, 42, 0, 0, 0]),
            eof(5, 2),
        ]
        .concat(),
    )
    .expect("fixture operation must succeed");
    let mut rows = 0;
    while let Some(e) = c.next_event().expect("fixture operation must succeed") {
        if let Event::Row { mut row, .. } = e {
            assert_eq!(
                row.next()
                    .expect("fixture operation must succeed")
                    .expect("fixture operation must succeed"),
                RawValue::Scalar(Value::Int(42))
            );
            rows += 1;
        }
    }
    assert_eq!(rows, 1);
    c.reset_statement(4, stmt.id)
        .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c), frame(0, &[0x1a, 17, 0, 0, 0]));
    c.receive(&ok(1, 0, 2))
        .expect("fixture operation must succeed");
    while c
        .next_event()
        .expect("fixture operation must succeed")
        .is_some()
    {}
    c.close_statement(5, stmt.id)
        .expect("fixture operation must succeed");
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
    flush(&mut c);
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 5,
            outcome: Outcome::Success
        })
    ));
    assert!(c.execute(6, stmt.id, &[], None).is_err());
    c.query(7, "bad", None)
        .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(1, b"\xff\x28\x04#42000syntax error"))
        .expect("fixture operation must succeed");
    let Some(Event::Error { error, .. }) = c.next_event().expect("fixture operation must succeed")
    else {
        panic!()
    };
    assert_eq!(error.errno, 1064);
    assert_eq!(error.code(), Some("ER_PARSE_ERROR"));
    assert_eq!(error.sql_state, "42000");
    assert_eq!(error.sql_message, "syntax error");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 7,
            outcome: Outcome::ServerError
        })
    ));
}
#[test]
fn infile_disabled_timeout_and_bad_sequence() {
    let mut c = ready();
    c.query(1, "LOAD DATA", None)
        .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(1, b"\xfb/etc/passwd"))
        .expect("fixture operation must succeed");
    assert_eq!(c.next_event().unwrap_err(), Error::LocalInfileDisabled);
    assert!(c.output().is_empty());
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Aborted(Error::LocalInfileDisabled)
        })
    ));
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Closed { .. })
    ));
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
    let mut c = ready();
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let now = Instant::now();
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    let now = Instant::from_duration(std::time::Duration::ZERO);
    c.query(2, "slow", Some(now))
        .expect("fixture operation must succeed");
    c.handle_timeout(now);
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 2,
            outcome: Outcome::Aborted(Error::Timeout)
        })
    ));
    let mut c = ready();
    c.query(3, "SELECT 1", None)
        .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&ok(99, 0, 2))
        .expect("fixture operation must succeed");
    assert!(c.next_event().is_err());
    c.abort(Error::Transport);
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed { token: 3, .. })
    ));
}

#[test]
fn rsa_full_auth_uses_host_seed_and_matches_independent_oaep_vector() {
    let mut c = Connection::new(Config {
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.receive(&handshake("caching_sha2_password", Caps::empty()))
        .expect("fixture operation must succeed");
    c.next_event().expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(2, &[1, 4]))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::AuthFull)
    ));
    assert_eq!(flush(&mut c), frame(3, &[2]));
    let mut key = vec![1];
    key.extend_from_slice(include_bytes!("fixtures/rsa-public.pem"));
    c.receive(&frame(4, &key))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::RsaSeedNeeded)
    ));
    assert!(c.output().is_empty());
    let mut seed = [0; 20];
    for (i, b) in seed.iter_mut().enumerate() {
        *b = i as u8;
    }
    c.rsa_seed(seed).expect("fixture operation must succeed");
    assert_eq!(
        flush(&mut c),
        frame(5, include_bytes!("fixtures/rsa-oaep-expected.bin"))
    );
    c.receive(&ok(6, 0, 2))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Connected { .. })
    ));
    assert!(c.rsa_seed(seed).is_err());
}
#[test]
fn local_infile_enabled_is_an_explicit_borrowed_request() {
    let mut c = Connection::new(Config {
        local_infile: true,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.receive(&handshake(
        "mysql_native_password",
        Caps::CLIENT_LOCAL_FILES,
    ))
    .expect("fixture operation must succeed");
    c.next_event().expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&ok(2, 0, 2))
        .expect("fixture operation must succeed");
    c.next_event().expect("fixture operation must succeed");
    c.query(1, "LOAD DATA LOCAL INFILE 'fixture.tsv'", None)
        .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(1, b"\xfbfixture.tsv"))
        .expect("fixture operation must succeed");
    let Some(Event::LocalInfile { token, file_name }) =
        c.next_event().expect("fixture operation must succeed")
    else {
        panic!()
    };
    assert_eq!(token, 1);
    assert_eq!(file_name, b"fixture.tsv");
    assert!(c.output().is_empty());
    c.local_infile_data(b"42\n")
        .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c), frame(2, b"42\n"));
    c.local_infile_finish()
        .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c), frame(3, b""));
    c.receive(&ok(4, 1, 2))
        .expect("fixture operation must succeed");
    let Some(Event::Ok { packet, .. }) = c.next_event().expect("fixture operation must succeed")
    else {
        panic!()
    };
    assert_eq!(packet.affected_rows(), 1);
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Success
        })
    ));
}
/// `can_accept` is the admission rule itself: whenever it is false a command is
/// rejected without side effects, and whenever it is true the next one is taken.
#[test]
fn can_accept_reports_the_exact_admission_decision() {
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    assert!(!c.can_accept());
    assert_eq!(
        c.ping(1),
        Err(Error::State("connection busy or closed")),
        "handshake in progress"
    );
    let mut c = ready();
    assert!(c.can_accept());
    c.query(1, "SELECT 1", None)
        .expect("admitted when can_accept is true");
    let request = c.output().len();
    assert!(!c.can_accept());
    assert!(c.ping(2).is_err());
    assert_eq!(
        c.output().len(),
        request,
        "a rejected command adds no bytes"
    );
    c.consume_output(request - 1)
        .expect("fixture operation must succeed");
    assert!(!c.can_accept(), "unacknowledged output");
    assert!(c.ping(2).is_err());
    c.consume_output(1).expect("fixture operation must succeed");
    assert!(!c.can_accept(), "command still pending");
    assert!(c.ping(2).is_err());
    c.receive(&ok(1, 0, 2))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Ok { token: 1, .. })
    ));
    assert!(!c.can_accept(), "Completed not yet delivered");
    assert!(c.ping(2).is_err());
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Success
        })
    ));
    assert!(c.can_accept());
    c.ping(2).expect("admitted when can_accept is true");
    assert_eq!(flush(&mut c), frame(0, &[0x0e]));
    c.abort(Error::Transport);
    assert!(!c.can_accept());
    assert!(c.quit().is_err());
}
