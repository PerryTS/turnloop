use std::time::Duration;
use turnloop_postgres::*;
fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    b.extend_from_slice(body);
    b
}
fn flush(c: &mut Connection) -> Vec<u8> {
    let b = c.output().to_vec();
    c.consume_output(b.len())
        .expect("fixture operation must succeed");
    b
}
fn ready() -> Connection {
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(b'R', &0u32.to_be_bytes()))
        .expect("fixture operation must succeed");
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
    c.receive(&frame(b'Z', b"I"))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Connected)
    ));
    c
}
#[test]
fn cleartext_md5_and_tls_transitions() {
    let mut c = Connection::new(Config {
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    assert!(flush(&mut c).windows(9).any(|v| v == b"postgres\0"));
    c.receive(&frame(b'R', &3u32.to_be_bytes()))
        .expect("fixture operation must succeed");
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
    assert_eq!(flush(&mut c), frame(b'p', b"secret\0"));
    let mut c = Connection::new(Config {
        user: "user".into(),
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(b'R', &[0, 0, 0, 5, 1, 2, 3, 4]))
        .expect("fixture operation must succeed");
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
    assert_eq!(
        flush(&mut c),
        frame(b'p', b"md5fccef98e4f1cf6cbe96b743fad4e8bd0\0")
    );
    let mut c = Connection::new(Config {
        ssl: SslMode::Require,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c), [0, 0, 0, 8, 4, 210, 22, 47]);
    c.receive(b"S").expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::UpgradeTls)
    ));
    assert!(c.query(1, "SELECT 1", None).is_err());
    c.tls_established().expect("fixture operation must succeed");
    assert!(!c.output().is_empty());
    let mut c = Connection::new(Config {
        ssl: SslMode::Require,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(b"N").expect("fixture operation must succeed");
    assert!(c.next_event().is_err());
}
#[test]
fn fragmented_pipeline_errors_metadata_and_exact_completion() {
    let mut c = ready();
    c.query(10, "SELECT 42; SELECT 43", None)
        .expect("fixture operation must succeed");
    c.query(11, "bad", None)
        .expect("fixture operation must succeed");
    c.query(12, "SELECT 99", None)
        .expect("fixture operation must succeed");
    flush(&mut c);
    let mut fields = vec![0, 1];
    fields.extend_from_slice(b"answer\0");
    fields.extend_from_slice(&[
        0, 0, 0, 0, 0, 0, 0, 0, 0, 23, 0, 4, 255, 255, 255, 255, 0, 0,
    ]);
    let bytes=[frame(b'T',&fields),frame(b'D',&[0,1,0,0,0,2,b'4',b'2']),frame(b'C',b"SELECT 1\0"),frame(b'C',b"SELECT 0\0"),frame(b'Z',b"I"),frame(b'E',b"SERROR\0C42601\0Mbad syntax\0Ddetail\0Hhint\0P4\0smy_schema\0tmy_table\0cmy_column\0dmy_type\0nconstraint\0Ffile.c\0L12\0Rroutine\0\0"),frame(b'Z',b"I"),frame(b'D',&[0,1,0,0,0,2,b'9',b'9']),frame(b'C',b"SELECT 1\0"),frame(b'Z',b"I")].concat();
    let mut rows = 0;
    let mut complete = Vec::new();
    let mut errors = 0;
    let mut descriptions = 0;
    for b in bytes {
        c.receive(&[b]).expect("fixture operation must succeed");
        while let Some(e) = c.next_event().expect("fixture operation must succeed") {
            match e {
                Event::Fields { fields, .. } => {
                    let f = fields
                        .into_iter()
                        .next()
                        .expect("fixture operation must succeed")
                        .expect("fixture operation must succeed");
                    assert_eq!(f.name, "answer");
                    assert_eq!(f.data_type_size, 4);
                    descriptions += 1;
                }
                Event::Row { token, mut row } => {
                    assert_eq!(
                        row.next()
                            .expect("fixture operation must succeed")
                            .expect("fixture operation must succeed")
                            .expect("fixture operation must succeed"),
                        if token == 10 { b"42" } else { b"99" }
                    );
                    rows += 1;
                }
                Event::Error { token, error } => {
                    assert_eq!(token, Some(11));
                    assert_eq!(error.code(), "42601");
                    assert_eq!(error.detail(), Some("detail"));
                    assert_eq!(error.get(b'n'), Some("constraint"));
                    assert_eq!(error.position(), Some("4"));
                    errors += 1;
                }
                Event::Completed { token, outcome, .. } => complete.push((token, outcome)),
                Event::CommandComplete { .. } => {}
                _ => panic!("unexpected {e:?}"),
            }
        }
    }
    assert_eq!(rows, 2);
    assert_eq!(descriptions, 1);
    assert_eq!(errors, 1);
    assert_eq!(
        complete,
        [
            (10, Outcome::Success),
            (11, Outcome::ServerError),
            (12, Outcome::Success)
        ]
    );
    assert_eq!(c.pending_count(), 0);
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let now = Instant::now();
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    let now = Instant::from_duration(std::time::Duration::ZERO);
    c.query(13, "slow", Some(now + Duration::from_secs(1)))
        .expect("fixture operation must succeed");
    c.query(14, "next", None)
        .expect("fixture operation must succeed");
    c.handle_timeout(now);
    assert_eq!(c.pending_count(), 2);
    c.handle_timeout(now + Duration::from_secs(1));
    for token in [13, 14] {
        assert!(
            matches!(c.next_event().expect("fixture operation must succeed"),Some(Event::Completed {token:t,outcome:Outcome::Aborted(Error::Timeout),..}) if t==token)
        );
    }
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Closed { .. })
    ));
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
}
#[test]
fn prepared_cache_recovery_and_copy_commands() {
    let mut c = ready();
    c.execute(
        1,
        ExtendedQuery {
            name: "name",
            sql: "SELECT $1::int4",
            oids: &[23],
            params: &[Parameter {
                value: Some(b"7"),
                format: 0,
            }],
            result_formats: &[1],
        },
        None,
    )
    .expect("fixture operation must succeed");
    let out = flush(&mut c);
    assert_eq!(out[0], b'P');
    c.receive(
        &[
            frame(b'1', b""),
            frame(b'2', b""),
            frame(b'C', b"SELECT 1\0"),
            frame(b'Z', b"I"),
        ]
        .concat(),
    )
    .expect("fixture operation must succeed");
    while c
        .next_event()
        .expect("fixture operation must succeed")
        .is_some()
    {}
    c.execute(
        2,
        ExtendedQuery {
            name: "name",
            sql: "SELECT $1::int4",
            oids: &[23],
            params: &[Parameter {
                value: None,
                format: 0,
            }],
            result_formats: &[1],
        },
        None,
    )
    .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c)[0], b'B');
    assert!(
        c.execute(
            3,
            ExtendedQuery {
                name: "name",
                sql: "SELECT 2",
                oids: &[],
                params: &[],
                result_formats: &[]
            },
            None
        )
        .is_err()
    );
    c.abort(Error::Cancelled);
    while c
        .next_event()
        .expect("fixture operation must succeed")
        .is_some()
    {}
    let mut c = ready();
    c.query(3, "COPY x FROM STDIN", None)
        .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(&frame(b'G', &[0, 0, 1, 0, 0]))
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::CopyIn { binary: false, .. })
    ));
    c.copy_data(b"42\n")
        .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c), frame(b'd', b"42\n"));
    c.copy_finish(Some("input failed"))
        .expect("fixture operation must succeed");
    assert_eq!(flush(&mut c), frame(b'f', b"input failed\0"));
}
#[test]
fn invalid_input_is_bounded_and_commands_are_atomic() {
    let mut c = ready();
    assert!(c.query(1, "SELECT\0oops", None).is_err());
    assert!(c.output().is_empty());
    assert_eq!(c.pending_count(), 0);
    c.query(1, "SELECT 1", None)
        .expect("fixture operation must succeed");
    assert!(c.query(1, "SELECT 2", None).is_err());
    flush(&mut c);
    c.receive(&[b'D', 0x7f, 0xff, 0xff, 0xff])
        .expect("fixture operation must succeed");
    assert_eq!(c.next_event().unwrap_err(), Error::Limit);
    c.abort(Error::Limit);
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Aborted(Error::Limit),
            ..
        })
    ));
    let mut c = ready();
    c.query(1, "select", None)
        .expect("fixture operation must succeed");
    c.receive(&frame(b'D', &[0, 1, 255, 255, 255, 254]))
        .expect("fixture operation must succeed");
    assert!(c.next_event().is_err());
}

#[path = "support/scram.rs"]
mod scram;
use scram::{base64, hex_salted_password, hmac};
#[test]
fn scram_and_plus_verify_server_signature_and_reject_bad_verifier() {
    // PBKDF2-HMAC-SHA256('secret','salt',4096), independently generated with Python hashlib.
    let salted = hex_salted_password();
    for (plus, valid) in [(false, true), (true, true), (false, false)] {
        let mut c = Connection::new(Config {
            ssl: if plus {
                SslMode::Require
            } else {
                SslMode::Disable
            },
            channel_binding_required: plus,
            ..Config::default()
        })
        .expect("fixture operation must succeed");
        flush(&mut c);
        if plus {
            c.receive(b"S").expect("fixture operation must succeed");
            assert!(matches!(
                c.next_event().expect("fixture operation must succeed"),
                Some(Event::UpgradeTls)
            ));
            c.tls_established_with_channel_binding(true)
                .expect("fixture operation must succeed");
            flush(&mut c);
        }
        let mut mechanisms = 10u32.to_be_bytes().to_vec();
        mechanisms.extend_from_slice(b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0");
        c.receive(&frame(b'R', &mechanisms))
            .expect("fixture operation must succeed");
        assert!(
            matches!(c.next_event().expect("fixture operation must succeed"),Some(Event::ScramNeeded {plus:p}) if p==plus)
        );
        let binding = if plus {
            ChannelBinding::tls_server_end_point(vec![7; 32])
        } else {
            ChannelBinding::unsupported()
        };
        let scram = ScramSha256::new(b"secret", binding);
        let first = std::str::from_utf8(scram.message())
            .expect("fixture operation must succeed")
            .to_owned();
        let bare = first
            .splitn(3, ',')
            .nth(2)
            .expect("fixture operation must succeed");
        let nonce = bare
            .split_once("r=")
            .expect("fixture operation must succeed")
            .1;
        let server_first = format!("r={nonce}server,s=c2FsdA==,i=4096");
        c.start_scram(scram)
            .expect("fixture operation must succeed");
        assert_eq!(flush(&mut c)[0], b'p');
        let mut continuation = 11u32.to_be_bytes().to_vec();
        continuation.extend_from_slice(server_first.as_bytes());
        c.receive(&frame(b'R', &continuation))
            .expect("fixture operation must succeed");
        assert!(
            c.next_event()
                .expect("fixture operation must succeed")
                .is_none()
        );
        let response = flush(&mut c);
        let client_final =
            std::str::from_utf8(&response[5..]).expect("fixture operation must succeed");
        let without_proof = client_final
            .split_once(",p=")
            .expect("fixture operation must succeed")
            .0;
        let auth_message = format!("{bare},{server_first},{without_proof}");
        let mut signature = hmac(&hmac(&salted, b"Server Key"), auth_message.as_bytes());
        if !valid {
            signature[0] ^= 1;
        }
        let mut final_message = 12u32.to_be_bytes().to_vec();
        final_message.extend_from_slice(format!("v={}", base64(&signature)).as_bytes());
        c.receive(&frame(b'R', &final_message))
            .expect("fixture operation must succeed");
        if valid {
            assert!(
                c.next_event()
                    .expect("fixture operation must succeed")
                    .is_none()
            );
            c.receive(&[frame(b'R', &0u32.to_be_bytes()), frame(b'Z', b"I")].concat())
                .expect("fixture operation must succeed");
            assert!(matches!(
                c.next_event().expect("fixture operation must succeed"),
                Some(Event::Connected)
            ));
        } else {
            assert!(c.next_event().is_err());
            assert!(!c.is_ready());
        }
    }
}

#[test]
fn extended_copy_resynchronizes_after_copy_done() {
    let mut c = ready();
    c.execute(
        1,
        ExtendedQuery {
            name: "",
            sql: "COPY t FROM STDIN",
            oids: &[],
            params: &[],
            result_formats: &[],
        },
        None,
    )
    .expect("fixture operation must succeed");
    flush(&mut c);
    c.receive(
        &[
            frame(b'1', b""),
            frame(b'2', b""),
            frame(b'n', b""),
            frame(b'G', &[0, 0, 1, 0, 0]),
        ]
        .concat(),
    )
    .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::CopyIn { .. })
    ));
    c.copy_data(b"42\n")
        .expect("fixture operation must succeed");
    c.copy_finish(None).expect("fixture operation must succeed");
    assert_eq!(
        flush(&mut c),
        [frame(b'd', b"42\n"), frame(b'c', b""), frame(b'S', b"")].concat()
    );
    c.receive(&[frame(b'C', b"COPY 1\0"), frame(b'Z', b"I")].concat())
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::CommandComplete {
            row_count: Some(1),
            ..
        })
    ));
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Success,
            ..
        })
    ));
}

fn tls_authentication(required: bool, available: bool) -> Result<Connection> {
    let mut c = Connection::new(Config {
        ssl: SslMode::Require,
        channel_binding_required: required,
        ..Default::default()
    })?;
    flush(&mut c);
    c.receive(b"S")?;
    assert!(matches!(c.next_event()?, Some(Event::UpgradeTls)));
    c.tls_established_with_channel_binding(available)?;
    flush(&mut c);
    Ok(c)
}

#[test]
fn scram_preference_is_decided_by_tls_binding_availability_and_server_offer() {
    let mut ran = 0;
    for available in [false, true] {
        for offer in [
            b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0".as_slice(),
            b"SCRAM-SHA-256\0\0",
            b"SCRAM-SHA-256-PLUS\0\0",
        ] {
            let mut c = tls_authentication(false, available).expect("TLS");
            c.receive(&frame(
                b'R',
                &[10u32.to_be_bytes().as_slice(), offer].concat(),
            ))
            .expect("mechanisms");
            let offers_plus = offer.starts_with(b"SCRAM-SHA-256-PLUS\0");
            let only_plus = offer == b"SCRAM-SHA-256-PLUS\0\0";
            if only_plus && !available {
                assert_eq!(
                    c.next_event().expect_err("no usable mechanism"),
                    Error::Protocol("unsupported SASL mechanisms")
                );
            } else {
                let plus = available && offers_plus;
                assert!(
                    matches!(c.next_event().expect("selection"), Some(Event::ScramNeeded { plus: selected }) if selected == plus)
                );
                let binding = if plus {
                    ChannelBinding::tls_server_end_point(vec![1; 32])
                } else {
                    ChannelBinding::unsupported()
                };
                c.start_scram(ScramSha256::new(b"secret", binding))
                    .expect("SCRAM start");
                let packet = flush(&mut c);
                let name = if plus {
                    b"SCRAM-SHA-256-PLUS\0".as_slice()
                } else {
                    b"SCRAM-SHA-256\0"
                };
                assert_eq!(&packet[5..5 + name.len()], name);
                let initial = &packet[5 + name.len() + 4..];
                assert!(initial.starts_with(if plus {
                    b"p=tls-server-end-point,,".as_slice()
                } else {
                    b"n,,"
                }));
            }
            ran += 1;
        }
    }
    assert_eq!(ran, 6);
}

#[test]
fn required_binding_rejects_missing_data_missing_plus_and_authentication_bypass() {
    assert!(matches!(
        tls_authentication(true, false),
        Err(Error::State(
            "channel binding required but certificate binding is unavailable"
        ))
    ));
    let mut c = tls_authentication(true, true).expect("binding available");
    c.receive(&frame(
        b'R',
        &[10u32.to_be_bytes().as_slice(), b"SCRAM-SHA-256\0\0"].concat(),
    ))
    .expect("plain-only offer");
    assert_eq!(
        c.next_event().expect_err("PLUS required"),
        Error::Protocol("channel binding required but server did not offer SCRAM-SHA-256-PLUS")
    );
    assert!(c.output().is_empty());
    let mut c = tls_authentication(true, true).expect("binding available");
    c.receive(&frame(b'R', &0u32.to_be_bytes()))
        .expect("AuthenticationOk without SCRAM");
    assert_eq!(
        c.next_event()
            .expect_err("trust cannot satisfy required binding"),
        Error::Protocol("channel binding required but SCRAM-PLUS was not authenticated")
    );
    assert!(!c.is_ready());
}

#[test]
fn legacy_tls_acknowledgement_has_no_binding_and_uses_n_gs2_flag() {
    let mut c = Connection::new(Config {
        ssl: SslMode::Require,
        ..Default::default()
    })
    .expect("core");
    flush(&mut c);
    c.receive(b"S").expect("TLS accepted");
    assert!(matches!(
        c.next_event().expect("event"),
        Some(Event::UpgradeTls)
    ));
    c.tls_established().expect("no binding acknowledgement");
    flush(&mut c);
    c.receive(&frame(
        b'R',
        &[
            10u32.to_be_bytes().as_slice(),
            b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0",
        ]
        .concat(),
    ))
    .expect("mechanisms");
    assert!(matches!(
        c.next_event().expect("selection"),
        Some(Event::ScramNeeded { plus: false })
    ));
    c.start_scram(ScramSha256::new(b"secret", ChannelBinding::unsupported()))
        .expect("plain SCRAM");
    let packet = flush(&mut c);
    assert_eq!(&packet[5..19], b"SCRAM-SHA-256\0");
    assert!(packet[23..].starts_with(b"n,,"));
}
