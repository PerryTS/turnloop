#![cfg(not(target_arch = "wasm32"))]
#![deny(unsafe_op_in_unsafe_fn)]
mod support;
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};
use support::Transport;
use turnloop_postgres::*;

#[derive(Default, Debug)]
struct Results {
    rows: Vec<Vec<Option<Vec<u8>>>>,
    tags: Vec<(String, Option<u64>)>,
    completed: Vec<(u64, Outcome, TransactionStatus)>,
    fields: Vec<(String, u32, i16)>,
    errors: Vec<(String, String)>,
    notices: usize,
    notifications: Vec<(String, String)>,
    copies: Vec<u8>,
}
struct Driver {
    core: Connection,
    io: Transport,
    password: Vec<u8>,
    ssl: bool,
    scram: bool,
    plus: bool,
    parameters: usize,
}
impl Driver {
    fn connect(user: &str, ssl: SslMode) -> Self {
        let port: u16 = std::env::var("TURNLOOP_TEST_POSTGRES_PORT")
            .expect("private server port required")
            .parse()
            .expect("fixture operation must succeed");
        Self::connect_to(port, user, ssl)
    }
    fn connect_to(port: u16, user: &str, ssl: SslMode) -> Self {
        eprintln!("PostgreSQL connecting as {user}, SSL mode {ssl:?}");
        let password = b"fixture-password".to_vec();
        let config = Config {
            user: user.into(),
            password: password.clone(),
            ssl,
            connect_deadline: Some(Instant::now() + Duration::from_secs(10)),
            ..Config::default()
        };
        let core = Connection::new(config).expect("fixture operation must succeed");
        let mut d = Self {
            core,
            io: Transport::connect(port),
            password,
            ssl: false,
            scram: false,
            plus: false,
            parameters: 0,
        };
        while !d.core.is_ready() {
            d.step(&mut Results::default(), None);
        }
        assert!(d.parameters > 3);
        d
    }
    fn flush(&mut self) {
        self.io
            .write_all(self.core.output())
            .expect("fixture operation must succeed");
        let n = self.core.output().len();
        self.core
            .consume_output(n)
            .expect("fixture operation must succeed");
        self.io.flush().expect("fixture operation must succeed");
    }
    fn step(&mut self, results: &mut Results, copy: Option<&[u8]>) {
        self.flush();
        let mut any = false;
        while let Some(event) = self
            .core
            .next_event()
            .expect("fixture operation must succeed")
        {
            any = true;
            match event {
                Event::UpgradeTls => {
                    self.io.upgrade();
                    self.core
                        .tls_established()
                        .expect("fixture operation must succeed");
                    self.ssl = true;
                }
                Event::ScramNeeded { plus } => {
                    // RFC 5929 tls-server-end-point for the SHA256 fixture certificate.
                    let binding = if plus {
                        let cert = std::fs::read(support::tools().join("server.der"))
                            .expect("fixture operation must succeed");
                        let digest = rustls::crypto::ring::default_provider()
                            .cipher_suites
                            .iter()
                            .find(|suite| {
                                suite.suite() == rustls::CipherSuite::TLS13_AES_128_GCM_SHA256
                            })
                            .and_then(|suite| suite.tls13())
                            .expect("fixture operation must succeed")
                            .common
                            .hash_provider
                            .hash(&cert);
                        // First ring TLS1.3 suite uses SHA256 in this provider.
                        assert_eq!(digest.as_ref().len(), 32);
                        ChannelBinding::tls_server_end_point(digest.as_ref().to_vec())
                    } else {
                        ChannelBinding::unrequested()
                    };
                    self.core
                        .start_scram(ScramSha256::new(&self.password, binding))
                        .expect("fixture operation must succeed");
                    self.scram = true;
                    self.plus = plus;
                }
                Event::ParameterStatus { .. } => self.parameters += 1,
                Event::Connected => {}
                Event::Row { row, .. } => results.rows.push(
                    row.map(|v| v.expect("fixture operation must succeed").map(Vec::from))
                        .collect(),
                ),
                Event::Fields { fields, .. } => {
                    results.fields.extend(fields.map(|f| {
                        let f = f.expect("fixture operation must succeed");
                        (f.name.into(), f.data_type_id, f.format)
                    }));
                }
                Event::CommandComplete { tag, row_count, .. } => {
                    results.tags.push((tag.into(), row_count))
                }
                Event::Completed {
                    token,
                    outcome,
                    transaction,
                } => results.completed.push((token, outcome, transaction)),
                Event::Error { error, .. } => results
                    .errors
                    .push((error.code().into(), error.message().into())),
                Event::Notice(_) => results.notices += 1,
                Event::Notification {
                    channel, payload, ..
                } => results.notifications.push((channel.into(), payload.into())),
                Event::CopyIn { .. } => {
                    self.core
                        .copy_data(copy.expect("COPY input fixture required"))
                        .expect("fixture operation must succeed");
                    self.core
                        .copy_finish(None)
                        .expect("fixture operation must succeed");
                }
                Event::CopyOut { .. } | Event::CopyDone { .. } => {}
                Event::CopyData { data, .. } => results.copies.extend_from_slice(data),
                Event::Closed { reason } => {
                    panic!(
                        "unexpected close: {reason}; server errors: {:?}",
                        results.errors
                    )
                }
            }
        }
        self.flush();
        if !any {
            let mut bytes = [0; 4096];
            let n = self
                .io
                .read(&mut bytes)
                .expect("fixture operation must succeed");
            assert!(n > 0, "unexpected EOF");
            self.core
                .receive(&bytes[..n])
                .expect("fixture operation must succeed");
        }
    }
    fn drain(&mut self, copy: Option<&[u8]>) -> Results {
        let mut result = Results::default();
        while self.core.pending_count() > 0 {
            self.step(&mut result, copy);
        }
        result
    }
    fn query(&mut self, sql: &str) -> Results {
        self.core
            .query(1, sql, None)
            .expect("fixture operation must succeed");
        self.drain(None)
    }
}

#[test]
fn rejected_startup_reports_server_sqlstate_and_message() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("private listener");
    let port = listener.local_addr().expect("private address").port();
    let server = std::thread::spawn(move || {
        let (mut io, _) = listener.accept().expect("client connects");
        io.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut size = [0; 4];
        io.read_exact(&mut size).expect("startup length");
        let size = u32::from_be_bytes(size) as usize;
        assert!((8..4096).contains(&size));
        let mut startup = vec![0; size - 4];
        io.read_exact(&mut startup).expect("startup body");
        assert!(
            startup
                .windows(14)
                .any(|value| value == b"user\0postgres\0")
        );
        let body = b"SFATAL\0VFATAL\0C28000\0Mrole \"postgres\" does not exist\0\0";
        io.write_all(b"E").expect("ErrorResponse tag");
        io.write_all(&((body.len() + 4) as u32).to_be_bytes())
            .expect("ErrorResponse length");
        io.write_all(body).expect("ErrorResponse body");
    });
    let error = std::panic::catch_unwind(|| Driver::connect_to(port, "postgres", SslMode::Disable))
        .err()
        .expect("missing role must fail startup");
    server
        .join()
        .expect("server must receive and reject startup");
    let message = error.downcast_ref::<String>().expect("diagnostic panic");
    assert!(message.contains("28000"), "{message}");
    assert!(message.contains("does not exist"), "{message}");
}

#[test]
#[ignore = "requires private PostgreSQL 16; use scripts/test-servers.py run"]
fn authentication_queries_pipeline_copy_cancel() {
    for user in ["clear_user", "md5_user", "scram_user", "tls_user"] {
        let tls = if user == "tls_user" {
            SslMode::Require
        } else {
            SslMode::Disable
        };
        let mut d = Driver::connect(user, tls);
        let result = d.query("SELECT current_user, 42 AS answer");
        assert_eq!(
            result.rows,
            vec![vec![Some(user.as_bytes().to_vec()), Some(b"42".to_vec())]]
        );
        assert_eq!(d.ssl, user == "tls_user");
        assert_eq!(d.scram, user == "scram_user" || user == "tls_user");
        if d.ssl {
            assert!(d.plus);
        }
    }
    let mut d = Driver::connect("scram_user", SslMode::Disable);
    let r = d.query("CREATE TEMP TABLE items(id int primary key, data text); INSERT INTO items VALUES(1,'one'),(2,'two'); SELECT * FROM items ORDER BY id");
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.tags.len(), 3);
    assert_eq!(r.tags[1], ("INSERT 0 2".into(), Some(2)));
    d.core
        .execute(
            2,
            ExtendedQuery {
                name: "cached",
                sql: "SELECT $1::int4 AS answer",
                oids: &[23],
                params: &[Parameter {
                    value: Some(b"42"),
                    format: 0,
                }],
                result_formats: &[1],
            },
            None,
        )
        .expect("fixture operation must succeed");
    let r = d.drain(None);
    assert_eq!(r.rows[0][0], Some(42i32.to_be_bytes().to_vec()));
    assert_eq!(r.fields[0], ("answer".into(), 23, 1));
    let n = 43i32.to_be_bytes();
    d.core
        .execute(
            3,
            ExtendedQuery {
                name: "cached",
                sql: "SELECT $1::int4 AS answer",
                oids: &[23],
                params: &[Parameter {
                    value: Some(&n),
                    format: 1,
                }],
                result_formats: &[0],
            },
            None,
        )
        .expect("fixture operation must succeed");
    d.core
        .query(4, "SELECT missing_column FROM items", None)
        .expect("fixture operation must succeed");
    d.core
        .query(5, "SELECT 99", None)
        .expect("fixture operation must succeed");
    let r = d.drain(None);
    assert_eq!(
        r.completed.iter().map(|r| r.0).collect::<Vec<_>>(),
        [3, 4, 5]
    );
    assert_eq!(r.completed[1].1, Outcome::ServerError);
    assert_eq!(r.errors[0].0, "42703");
    assert_eq!(r.rows[0][0], Some(b"43".to_vec()));
    assert_eq!(r.rows[1][0], Some(b"99".to_vec()));
    assert_eq!(
        d.query("BEGIN").completed[0].2,
        TransactionStatus::InTransaction
    );
    assert_eq!(
        d.query("SELECT 1/0").completed[0].2,
        TransactionStatus::Failed
    );
    assert_eq!(d.query("ROLLBACK").completed[0].2, TransactionStatus::Idle);
    let mut r = d.query("DO $$ BEGIN RAISE NOTICE 'fixture notice'; END $$; LISTEN changes; NOTIFY changes, 'payload'");
    while r.notifications.is_empty() {
        d.step(&mut r, None);
    }
    assert_eq!(r.notices, 1);
    assert_eq!(r.notifications, [("changes".into(), "payload".into())]);
    d.core
        .query(6, "COPY items FROM STDIN", None)
        .expect("fixture operation must succeed");
    let r = d.drain(Some(b"3\tthree\n4\tfour\n"));
    assert_eq!(r.tags, [("COPY 2".into(), Some(2))]);
    let r = d.query("COPY (SELECT * FROM items ORDER BY id) TO STDOUT");
    assert_eq!(r.copies, b"1\tone\n2\ttwo\n3\tthree\n4\tfour\n");
    d.core
        .query(7, "SELECT pg_sleep(10)", None)
        .expect("fixture operation must succeed");
    d.flush();
    // The Docker image initializes the superuser as `turnloop`, so a `postgres`
    // role need not exist. A same-role session can inspect its own backends.
    let mut observer = Driver::connect("scram_user", SslMode::Disable);
    let cancel = d.core.cancel_request().expect("BackendKeyData required");
    let pid = i32::from_be_bytes(cancel[8..12].try_into().expect("four PID bytes"));
    assert!(pid > 0);
    let active = format!(
        "SELECT count(*) FROM pg_stat_activity WHERE pid={pid} AND query='SELECT pg_sleep(10)' AND state='active'"
    );
    // Prove this exact backend started before sending the separate CancelRequest.
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let r = observer.query(&active);
        assert!(r.errors.is_empty(), "observer errors: {:?}", r.errors);
        if r.rows[0][0].as_deref() == Some(b"1") {
            break;
        }
        assert!(Instant::now() < until, "backend {pid} never became active");
        std::thread::sleep(Duration::from_millis(10));
    }
    let port: u16 = std::env::var("TURNLOOP_TEST_POSTGRES_PORT")
        .expect("fixture operation must succeed")
        .parse()
        .expect("fixture operation must succeed");
    let mut cancel_io = TcpStream::connect(("127.0.0.1", port)).expect("cancel connection");
    cancel_io
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("cancel timeout");
    cancel_io.write_all(&cancel).expect("send CancelRequest");
    // PostgreSQL closes this connection without a response after processing it.
    assert_eq!(cancel_io.read(&mut [0; 1]).expect("cancel EOF"), 0);
    let r = d.drain(None);
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].0, "57014");
    assert_eq!(
        r.completed,
        [(7, Outcome::ServerError, TransactionStatus::Idle)]
    );
    assert_eq!(
        d.query("SELECT count(*) FROM items").rows[0][0],
        Some(b"4".to_vec())
    );
}

#[test]
#[ignore = "requires private PostgreSQL 16; use scripts/test-servers.py run"]
fn all_common_types_in_text_binary_and_arrays() {
    use turnloop_postgres::types::{Value, decode};
    let mut d = Driver::connect("scram_user", SslMode::Disable);
    let cases = [
        (21, "7::int2"),
        (23, "42::int4"),
        (20, "9223372036854775807::int8"),
        (700, "1.5::float4"),
        (701, "1.25::float8"),
        (1700, "1234567890.2300::numeric"),
        (16, "true"),
        (25, "'hello'::text"),
        (1043, "'hello'::varchar"),
        (17, "decode('00ff','hex')"),
        (1082, "'2026-09-14'::date"),
        (1114, "'2026-09-14 12:34:56.123456'::timestamp"),
        (1184, "'2026-09-14 12:34:56.123456+00'::timestamptz"),
        (114, "'{\"x\":1}'::json"),
        (3802, "'{\"x\":1}'::jsonb"),
        (2950, "'550e8400-e29b-41d4-a716-446655440000'::uuid"),
    ];
    for (oid, expression) in cases {
        let sql = format!("SELECT {expression} AS value");
        let text = d.query(&sql);
        assert!(text.errors.is_empty(), "{text:?}");
        assert_eq!(text.rows.len(), 1);
        assert_eq!(text.fields[0].1, oid);
        let text_value =
            decode(oid, 0, text.rows[0][0].as_deref()).expect("fixture operation must succeed");
        assert!(!matches!(text_value, Value::Null | Value::Raw { .. }));
        d.core
            .execute(
                2,
                ExtendedQuery {
                    name: "",
                    sql: &sql,
                    oids: &[],
                    params: &[],
                    result_formats: &[1],
                },
                None,
            )
            .expect("fixture operation must succeed");
        let binary = d.drain(None);
        assert!(binary.errors.is_empty(), "{binary:?}");
        assert_eq!(binary.rows.len(), 1);
        let binary_value =
            decode(oid, 1, binary.rows[0][0].as_deref()).expect("fixture operation must succeed");
        if !matches!(oid, 1082 | 1114 | 1184) {
            assert_eq!(text_value, binary_value, "OID {oid}");
        } else {
            assert!(matches!(
                binary_value,
                Value::Date(_) | Value::Timestamp { .. }
            ));
        }
        let sql = format!("SELECT ARRAY[{expression},NULL] AS values");
        let r = d.query(&sql);
        assert!(r.errors.is_empty(), "{r:?}");
        let array_oid = r.fields[0].1;
        let v =
            decode(array_oid, 0, r.rows[0][0].as_deref()).expect("fixture operation must succeed");
        let Value::Array(values) = v else {
            panic!("array not decoded")
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], text_value);
        assert_eq!(values[1], Value::Null);
        d.core
            .execute(
                3,
                ExtendedQuery {
                    name: "",
                    sql: &sql,
                    oids: &[],
                    params: &[],
                    result_formats: &[1],
                },
                None,
            )
            .expect("fixture operation must succeed");
        let r = d.drain(None);
        let Value::Array(values) =
            decode(array_oid, 1, r.rows[0][0].as_deref()).expect("fixture operation must succeed")
        else {
            panic!("binary array not decoded")
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], binary_value);
        assert_eq!(values[1], Value::Null);
    }
}

#[test]
#[ignore = "requires private PostgreSQL 16; use scripts/test-servers.py run"]
fn pool_reuses_real_connection_and_explicit_idle_timeout() {
    use turnloop_postgres::pool::{Config, Event, Pool};
    let now = Instant::now();
    let mut pool = Pool::new(Config {
        max: 1,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    pool.checkout(1, now, None)
        .expect("fixture operation must succeed");
    let Some(Event::Connect(id)) = pool.next_event() else {
        panic!()
    };
    let mut connection = Driver::connect("scram_user", SslMode::Disable);
    assert!(
        connection
            .query("CREATE TEMP TABLE pooled(id int); INSERT INTO pooled VALUES(42)")
            .errors
            .is_empty()
    );
    pool.connected(id, now)
        .expect("fixture operation must succeed");
    pool.next_event();
    let Some(Event::Acquired(lease)) = pool.next_event() else {
        panic!()
    };
    pool.checkin(lease, now, false)
        .expect("fixture operation must succeed");
    pool.next_event();
    pool.checkout(2, now, None)
        .expect("fixture operation must succeed");
    let Some(Event::Acquired(lease)) = pool.next_event() else {
        panic!()
    };
    assert_eq!(lease.connection, id);
    assert_eq!(
        connection.query("SELECT id FROM pooled").rows[0][0],
        Some(b"42".to_vec())
    );
    pool.checkin(lease, now, false)
        .expect("fixture operation must succeed");
    pool.next_event();
    pool.handle_timeout(pool.next_timeout().expect("fixture operation must succeed"));
    assert_eq!(pool.next_event(), Some(Event::Close(id)));
    connection
        .core
        .end()
        .expect("fixture operation must succeed");
    connection.flush();
    assert!(matches!(
        connection
            .core
            .next_event()
            .expect("fixture operation must succeed"),
        Some(turnloop_postgres::Event::Closed { .. })
    ));
    pool.closed(id, now)
        .expect("fixture operation must succeed");
    assert_eq!(pool.total_count(), 0);
}
