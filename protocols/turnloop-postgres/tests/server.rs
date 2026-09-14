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
        let password = b"fixture-password".to_vec();
        let config = Config {
            user: user.into(),
            password: password.clone(),
            ssl,
            connect_deadline: Some(Instant::now() + Duration::from_secs(10)),
            ..Config::default()
        };
        let core = Connection::new(config).unwrap();
        let port: u16 = std::env::var("TURNLOOP_PG_PORT")
            .expect("private server port required")
            .parse()
            .unwrap();
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
        self.io.write_all(self.core.output()).unwrap();
        let n = self.core.output().len();
        self.core.consume_output(n).unwrap();
        self.io.flush().unwrap();
    }
    fn step(&mut self, results: &mut Results, copy: Option<&[u8]>) {
        self.flush();
        let mut any = false;
        while let Some(event) = self.core.next_event().unwrap() {
            any = true;
            match event {
                Event::UpgradeTls => {
                    self.io.upgrade();
                    self.core.tls_established().unwrap();
                    self.ssl = true;
                }
                Event::ScramNeeded { plus } => {
                    // RFC 5929 tls-server-end-point for the SHA256 fixture certificate.
                    let binding = if plus {
                        let cert = std::fs::read(support::tools().join("server.der")).unwrap();
                        let digest = rustls::crypto::ring::default_provider()
                            .cipher_suites
                            .iter()
                            .find(|suite| {
                                suite.suite() == rustls::CipherSuite::TLS13_AES_128_GCM_SHA256
                            })
                            .and_then(|suite| suite.tls13())
                            .unwrap()
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
                        .unwrap();
                    self.scram = true;
                    self.plus = plus;
                }
                Event::ParameterStatus { .. } => self.parameters += 1,
                Event::Connected => {}
                Event::Row { row, .. } => results
                    .rows
                    .push(row.map(|v| v.unwrap().map(Vec::from)).collect()),
                Event::Fields { fields, .. } => {
                    results.fields.extend(fields.map(|f| {
                        let f = f.unwrap();
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
                        .unwrap();
                    self.core.copy_finish(None).unwrap();
                }
                Event::CopyOut { .. } | Event::CopyDone { .. } => {}
                Event::CopyData { data, .. } => results.copies.extend_from_slice(data),
                Event::Closed { reason } => panic!("unexpected close: {reason}"),
            }
        }
        self.flush();
        if !any {
            let mut bytes = [0; 4096];
            let n = self.io.read(&mut bytes).unwrap();
            assert!(n > 0, "unexpected EOF");
            self.core.receive(&bytes[..n]).unwrap();
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
        self.core.query(1, sql, None).unwrap();
        self.drain(None)
    }
}

#[test]
#[ignore = "requires private PostgreSQL 16; use scripts/sql-servers.py run"]
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
        .unwrap();
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
        .unwrap();
    d.core
        .query(4, "SELECT missing_column FROM items", None)
        .unwrap();
    d.core.query(5, "SELECT 99", None).unwrap();
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
    let r = d.query("DO $$ BEGIN RAISE NOTICE 'fixture notice'; END $$; LISTEN changes; NOTIFY changes, 'payload'");
    assert_eq!(r.notices, 1);
    assert_eq!(r.notifications, [("changes".into(), "payload".into())]);
    d.core.query(6, "COPY items FROM STDIN", None).unwrap();
    let r = d.drain(Some(b"3\tthree\n4\tfour\n"));
    assert_eq!(r.tags, [("COPY 2".into(), Some(2))]);
    let r = d.query("COPY (SELECT * FROM items ORDER BY id) TO STDOUT");
    assert_eq!(r.copies, b"1\tone\n2\ttwo\n3\tthree\n4\tfour\n");
    d.core.query(7, "SELECT pg_sleep(10)", None).unwrap();
    d.flush();
    // Separate query proves the backend started before sending CancelRequest.
    let mut observer = Driver::connect("postgres", SslMode::Disable);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let r = observer.query("SELECT count(*) FROM pg_stat_activity WHERE query='SELECT pg_sleep(10)' AND state='active'");
        if r.rows[0][0].as_deref() == Some(b"1") {
            break;
        }
        assert!(Instant::now() < until);
    }
    let port: u16 = std::env::var("TURNLOOP_PG_PORT").unwrap().parse().unwrap();
    TcpStream::connect(("127.0.0.1", port))
        .unwrap()
        .write_all(&d.core.cancel_request().unwrap())
        .unwrap();
    let r = d.drain(None);
    assert_eq!(r.errors[0].0, "57014");
    assert_eq!(r.completed[0].1, Outcome::ServerError);
    assert_eq!(
        d.query("SELECT count(*) FROM items").rows[0][0],
        Some(b"4".to_vec())
    );
}
