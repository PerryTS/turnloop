use std::time::{Duration, Instant};
use turnloop_postgres::*;
fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    b.extend_from_slice(body);
    b
}
fn flush(c: &mut Connection) -> Vec<u8> {
    let b = c.output().to_vec();
    c.consume_output(b.len()).unwrap();
    b
}
fn ready() -> Connection {
    let mut c = Connection::new(Config::default()).unwrap();
    flush(&mut c);
    c.receive(&frame(b'R', &0u32.to_be_bytes())).unwrap();
    assert!(c.next_event().unwrap().is_none());
    c.receive(&frame(b'Z', b"I")).unwrap();
    assert!(matches!(c.next_event().unwrap(), Some(Event::Connected)));
    c
}
#[test]
fn cleartext_md5_and_tls_transitions() {
    let mut c = Connection::new(Config {
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .unwrap();
    assert!(flush(&mut c).windows(9).any(|v| v == b"postgres\0"));
    c.receive(&frame(b'R', &3u32.to_be_bytes())).unwrap();
    assert!(c.next_event().unwrap().is_none());
    assert_eq!(flush(&mut c), frame(b'p', b"secret\0"));
    let mut c = Connection::new(Config {
        user: "user".into(),
        password: b"secret".to_vec(),
        ..Config::default()
    })
    .unwrap();
    flush(&mut c);
    c.receive(&frame(b'R', &[0, 0, 0, 5, 1, 2, 3, 4])).unwrap();
    assert!(c.next_event().unwrap().is_none());
    assert_eq!(
        flush(&mut c),
        frame(b'p', b"md5fccef98e4f1cf6cbe96b743fad4e8bd0\0")
    );
    let mut c = Connection::new(Config {
        ssl: SslMode::Require,
        ..Config::default()
    })
    .unwrap();
    assert_eq!(flush(&mut c), [0, 0, 0, 8, 4, 210, 22, 47]);
    c.receive(b"S").unwrap();
    assert!(matches!(c.next_event().unwrap(), Some(Event::UpgradeTls)));
    assert!(c.query(1, "SELECT 1", None).is_err());
    c.tls_established().unwrap();
    assert!(!c.output().is_empty());
    let mut c = Connection::new(Config {
        ssl: SslMode::Require,
        ..Config::default()
    })
    .unwrap();
    flush(&mut c);
    c.receive(b"N").unwrap();
    assert!(c.next_event().is_err());
}
#[test]
fn fragmented_pipeline_errors_metadata_and_exact_completion() {
    let mut c = ready();
    c.query(10, "SELECT 42; SELECT 43", None).unwrap();
    c.query(11, "bad", None).unwrap();
    c.query(12, "SELECT 99", None).unwrap();
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
        c.receive(&[b]).unwrap();
        while let Some(e) = c.next_event().unwrap() {
            match e {
                Event::Fields { fields, .. } => {
                    let f = fields.into_iter().next().unwrap().unwrap();
                    assert_eq!(f.name, "answer");
                    assert_eq!(f.data_type_size, 4);
                    descriptions += 1;
                }
                Event::Row { token, mut row } => {
                    assert_eq!(
                        row.next().unwrap().unwrap().unwrap(),
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
    let now = Instant::now();
    c.query(13, "slow", Some(now + Duration::from_secs(1)))
        .unwrap();
    c.query(14, "next", None).unwrap();
    c.handle_timeout(now);
    assert_eq!(c.pending_count(), 2);
    c.handle_timeout(now + Duration::from_secs(1));
    for token in [13, 14] {
        assert!(
            matches!(c.next_event().unwrap(),Some(Event::Completed {token:t,outcome:Outcome::Aborted(Error::Timeout),..}) if t==token)
        );
    }
    assert!(matches!(
        c.next_event().unwrap(),
        Some(Event::Closed { .. })
    ));
    assert!(c.next_event().unwrap().is_none());
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
    .unwrap();
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
    .unwrap();
    while c.next_event().unwrap().is_some() {}
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
    .unwrap();
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
    while c.next_event().unwrap().is_some() {}
    let mut c = ready();
    c.query(3, "COPY x FROM STDIN", None).unwrap();
    flush(&mut c);
    c.receive(&frame(b'G', &[0, 0, 1, 0, 0])).unwrap();
    assert!(matches!(
        c.next_event().unwrap(),
        Some(Event::CopyIn { binary: false, .. })
    ));
    c.copy_data(b"42\n").unwrap();
    assert_eq!(flush(&mut c), frame(b'd', b"42\n"));
    c.copy_finish(Some("input failed")).unwrap();
    assert_eq!(flush(&mut c), frame(b'f', b"input failed\0"));
}
#[test]
fn invalid_input_is_bounded_and_commands_are_atomic() {
    let mut c = ready();
    assert!(c.query(1, "SELECT\0oops", None).is_err());
    assert!(c.output().is_empty());
    assert_eq!(c.pending_count(), 0);
    c.query(1, "SELECT 1", None).unwrap();
    assert!(c.query(1, "SELECT 2", None).is_err());
    flush(&mut c);
    c.receive(&[b'D', 0x7f, 0xff, 0xff, 0xff]).unwrap();
    assert_eq!(c.next_event().unwrap_err(), Error::Limit);
    c.abort(Error::Limit);
    assert!(matches!(
        c.next_event().unwrap(),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Aborted(Error::Limit),
            ..
        })
    ));
    let mut c = ready();
    c.query(1, "select", None).unwrap();
    c.receive(&frame(b'D', &[0, 1, 255, 255, 255, 254]))
        .unwrap();
    assert!(c.next_event().is_err());
}

fn sha256(b: &[u8]) -> [u8; 32] {
    let p = rustls::crypto::ring::default_provider();
    let h = p
        .cipher_suites
        .iter()
        .find(|s| s.suite() == rustls::CipherSuite::TLS13_AES_128_GCM_SHA256)
        .unwrap()
        .tls13()
        .unwrap()
        .common
        .hash_provider;
    h.hash(b).as_ref().try_into().unwrap()
}
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut inner = vec![0x36; 64];
    let mut outer = vec![0x5c; 64];
    for (i, b) in key.iter().enumerate() {
        inner[i] ^= *b;
        outer[i] ^= *b;
    }
    inner.extend_from_slice(message);
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}
fn base64(bytes: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        s.push(alphabet[a >> 2] as char);
        s.push(alphabet[((a & 3) << 4) | (b >> 4)] as char);
        s.push(if chunk.len() > 1 {
            alphabet[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            alphabet[c & 63] as char
        } else {
            '='
        });
    }
    s
}
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
        .unwrap();
        flush(&mut c);
        if plus {
            c.receive(b"S").unwrap();
            assert!(matches!(c.next_event().unwrap(), Some(Event::UpgradeTls)));
            c.tls_established().unwrap();
            flush(&mut c);
        }
        let mut mechanisms = 10u32.to_be_bytes().to_vec();
        mechanisms.extend_from_slice(b"SCRAM-SHA-256-PLUS\0SCRAM-SHA-256\0\0");
        c.receive(&frame(b'R', &mechanisms)).unwrap();
        assert!(matches!(c.next_event().unwrap(),Some(Event::ScramNeeded {plus:p}) if p==plus));
        let binding = if plus {
            ChannelBinding::tls_server_end_point(vec![7; 32])
        } else {
            ChannelBinding::unsupported()
        };
        let scram = ScramSha256::new(b"secret", binding);
        let first = std::str::from_utf8(scram.message()).unwrap().to_owned();
        let bare = first.splitn(3, ',').nth(2).unwrap();
        let nonce = bare.split_once("r=").unwrap().1;
        let server_first = format!("r={nonce}server,s=c2FsdA==,i=4096");
        c.start_scram(scram).unwrap();
        assert_eq!(flush(&mut c)[0], b'p');
        let mut continuation = 11u32.to_be_bytes().to_vec();
        continuation.extend_from_slice(server_first.as_bytes());
        c.receive(&frame(b'R', &continuation)).unwrap();
        assert!(c.next_event().unwrap().is_none());
        let response = flush(&mut c);
        let client_final = std::str::from_utf8(&response[5..]).unwrap();
        let without_proof = client_final.split_once(",p=").unwrap().0;
        let auth_message = format!("{bare},{server_first},{without_proof}");
        let mut signature = hmac(&hmac(&salted, b"Server Key"), auth_message.as_bytes());
        if !valid {
            signature[0] ^= 1;
        }
        let mut final_message = 12u32.to_be_bytes().to_vec();
        final_message.extend_from_slice(format!("v={}", base64(&signature)).as_bytes());
        c.receive(&frame(b'R', &final_message)).unwrap();
        if valid {
            assert!(c.next_event().unwrap().is_none());
            c.receive(&[frame(b'R', &0u32.to_be_bytes()), frame(b'Z', b"I")].concat())
                .unwrap();
            assert!(matches!(c.next_event().unwrap(), Some(Event::Connected)));
        } else {
            assert!(c.next_event().is_err());
            assert!(!c.is_ready());
        }
    }
}
fn hex_salted_password() -> [u8; 32] {
    [
        96, 154, 98, 181, 182, 135, 186, 101, 146, 177, 42, 85, 44, 121, 254, 59, 241, 158, 78, 20,
        90, 22, 123, 121, 91, 122, 202, 181, 232, 159, 160, 246,
    ]
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
    .unwrap();
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
    .unwrap();
    assert!(matches!(
        c.next_event().unwrap(),
        Some(Event::CopyIn { .. })
    ));
    c.copy_data(b"42\n").unwrap();
    c.copy_finish(None).unwrap();
    assert_eq!(
        flush(&mut c),
        [frame(b'd', b"42\n"), frame(b'c', b""), frame(b'S', b"")].concat()
    );
    c.receive(&[frame(b'C', b"COPY 1\0"), frame(b'Z', b"I")].concat())
        .unwrap();
    assert!(matches!(
        c.next_event().unwrap(),
        Some(Event::CommandComplete {
            row_count: Some(1),
            ..
        })
    ));
    assert!(matches!(
        c.next_event().unwrap(),
        Some(Event::Completed {
            token: 1,
            outcome: Outcome::Success,
            ..
        })
    ));
}
