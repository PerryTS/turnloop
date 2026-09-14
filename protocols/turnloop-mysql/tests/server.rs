#![cfg(not(target_arch = "wasm32"))]
#![deny(unsafe_op_in_unsafe_fn)]
mod support;
use std::io::{Read, Write};
use support::Transport;
use turnloop_mysql::*;
#[derive(Default, Debug)]
struct Results {
    rows: Vec<Vec<Value>>,
    oks: Vec<(u64, u64, u16)>,
    errors: Vec<(u16, String, String)>,
    completed: Vec<Outcome>,
    statement: Option<Statement>,
    columns: Vec<String>,
    infile: usize,
}
struct Driver {
    core: Connection,
    io: Transport,
    fast: usize,
    full: usize,
    rsa: usize,
    tls: usize,
}
impl Driver {
    fn connect(user: &str, tls: bool, compression: bool, infile: bool) -> Self {
        let core = Connection::new(Config {
            user: user.into(),
            password: b"fixture-password".to_vec(),
            database: Some("turnloop_test".into()),
            tls,
            compression,
            local_infile: infile,
            multiple_statements: true,
            ..Config::default()
        })
        .expect("fixture operation must succeed");
        let port: u16 = std::env::var("TURNLOOP_TEST_MYSQL_PORT")
            .expect("private server port required")
            .parse()
            .expect("fixture operation must succeed");
        let mut d = Self {
            core,
            io: Transport::connect(port),
            fast: 0,
            full: 0,
            rsa: 0,
            tls: 0,
        };
        while !d.core.is_ready() {
            d.step(&mut Results::default(), None);
        }
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
    fn step(&mut self, r: &mut Results, infile: Option<&[u8]>) {
        self.flush();
        let mut any = false;
        while let Some(event) = self
            .core
            .next_event()
            .expect("fixture operation must succeed")
        {
            any = true;
            match event {
                Event::Progress | Event::Connected { .. } | Event::ColumnCount { .. } => {}
                Event::UpgradeTls => {
                    self.flush();
                    self.io.upgrade();
                    self.core
                        .tls_established()
                        .expect("fixture operation must succeed");
                    self.tls += 1;
                }
                Event::AuthFastSuccess => self.fast += 1,
                Event::AuthFull => self.full += 1,
                Event::RsaSeedNeeded => {
                    let mut seed = [0; 20];
                    rustls::crypto::ring::default_provider()
                        .secure_random
                        .fill(&mut seed)
                        .expect("fixture operation must succeed");
                    self.core
                        .rsa_seed(seed)
                        .expect("fixture operation must succeed");
                    self.rsa += 1;
                }
                Event::Column { column, .. } => r.columns.push(
                    String::from_utf8(column.name.to_vec())
                        .expect("fixture operation must succeed"),
                ),
                Event::Row { row, .. } => r.rows.push(
                    row.map(|v| match v.expect("fixture operation must succeed") {
                        RawValue::Null => Value::NULL,
                        RawValue::Bytes(b) => Value::Bytes(b.to_vec()),
                        RawValue::Scalar(v) => v,
                    })
                    .collect(),
                ),
                Event::Ok { packet, .. } => r.oks.push((
                    packet.affected_rows(),
                    packet.last_insert_id().unwrap_or(0),
                    packet.warnings(),
                )),
                Event::Prepared { statement, .. } => r.statement = Some(statement),
                Event::Error { error, .. } => r.errors.push((
                    error.errno,
                    error.sql_state.into(),
                    error.sql_message.into(),
                )),
                Event::Completed { outcome, .. } => r.completed.push(outcome),
                Event::LocalInfile { file_name, .. } => {
                    assert_eq!(file_name, b"fixture.tsv");
                    r.infile += 1;
                    self.core
                        .local_infile_data(infile.expect("infile fixture required"))
                        .expect("fixture operation must succeed");
                    self.core
                        .local_infile_finish()
                        .expect("fixture operation must succeed");
                }
                Event::Closed { reason } => panic!("unexpected close: {reason}"),
            }
        }
        self.flush();
        if !any {
            let mut b = [0; 4096];
            let n = self
                .io
                .read(&mut b)
                .expect("fixture operation must succeed");
            assert!(n > 0, "unexpected EOF");
            self.core
                .receive(&b[..n])
                .expect("fixture operation must succeed");
        }
    }
    fn drain(&mut self, infile: Option<&[u8]>) -> Results {
        let mut r = Results::default();
        while r.completed.is_empty() {
            self.step(&mut r, infile);
        }
        r
    }
    fn query(&mut self, sql: &str) -> Results {
        self.core
            .query(1, sql, None)
            .expect("fixture operation must succeed");
        self.drain(None)
    }
}
#[test]
#[ignore = "requires private MySQL 9.6; use scripts/test-servers.py run"]
fn auth_prepared_transactions_compression_and_infile() {
    // First connection must encounter an empty caching_sha2 server cache.
    let mut d = Driver::connect("auth_rsa_user", false, false, true);
    assert_eq!(d.full, 1);
    assert_eq!(d.rsa, 1);
    let cached = Driver::connect("auth_rsa_user", false, false, false);
    assert_eq!(cached.fast, 1);
    assert_eq!(cached.rsa, 0);
    let secure = Driver::connect("tls_user", true, false, false);
    assert_eq!(secure.tls, 1);
    assert_eq!(secure.full, 1);
    assert_eq!(secure.rsa, 0);
    let r=d.query("CREATE TEMPORARY TABLE items(id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY, name TEXT); INSERT INTO items(name) VALUES('one'),('two'); SELECT * FROM items ORDER BY id");
    assert_eq!(r.rows.len(), 2);
    assert!(r.oks.contains(&(2, 1, 0)));
    assert_eq!(
        r.rows[0],
        vec![Value::Bytes(b"1".to_vec()), Value::Bytes(b"one".to_vec())]
    );
    d.core
        .prepare(
            2,
            "SELECT CAST(? AS SIGNED) AS n, CAST(? AS CHAR) AS s",
            None,
        )
        .expect("fixture operation must succeed");
    let stmt = d
        .drain(None)
        .statement
        .expect("fixture operation must succeed");
    assert_eq!(stmt.parameters, 2);
    assert_eq!(stmt.columns, 2);
    d.core
        .execute(
            3,
            stmt.id,
            &[Value::Int(42), Value::Bytes(b"hello".to_vec())],
            None,
        )
        .expect("fixture operation must succeed");
    let r = d.drain(None);
    assert_eq!(
        r.rows,
        vec![vec![Value::Int(42), Value::Bytes(b"hello".to_vec())]]
    );
    d.core
        .reset_statement(4, stmt.id)
        .expect("fixture operation must succeed");
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    d.core
        .close_statement(5, stmt.id)
        .expect("fixture operation must succeed");
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    assert!(d.core.execute(6, stmt.id, &[], None).is_err());
    d.query("START TRANSACTION");
    assert!(
        d.core
            .status()
            .contains(StatusFlags::SERVER_STATUS_IN_TRANS)
    );
    d.query("INSERT INTO items(name) VALUES('rollback')");
    d.query("ROLLBACK");
    assert!(
        !d.core
            .status()
            .contains(StatusFlags::SERVER_STATUS_IN_TRANS)
    );
    assert_eq!(
        d.query("SELECT count(*) FROM items").rows[0][0],
        Value::Bytes(b"2".to_vec())
    );
    let r = d.query("SELECT missing FROM items");
    assert_eq!(r.errors[0].0, 1054);
    assert_eq!(r.errors[0].1, "42S22");
    assert_eq!(error_code(r.errors[0].0), Some("ER_BAD_FIELD_ERROR"));
    d.core.ping(7).expect("fixture operation must succeed");
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    d.core
        .query(
            8,
            "LOAD DATA LOCAL INFILE 'fixture.tsv' INTO TABLE items (name)",
            None,
        )
        .expect("fixture operation must succeed");
    let r = d.drain(Some(b"three\nfour\n"));
    assert_eq!(r.infile, 1);
    assert!(r.oks.iter().any(|v| v.0 == 2));
    assert_eq!(
        d.query("SELECT count(*) FROM items").rows[0][0],
        Value::Bytes(b"4".to_vec())
    );
    let mut compressed = Driver::connect("sql_user", false, true, false);
    let r = compressed.query("SELECT REPEAT('x',100000)");
    assert_eq!(r.rows[0][0], Value::Bytes(vec![b'x'; 100000]));
    d.core
        .reset_connection(9)
        .expect("fixture operation must succeed");
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    assert_eq!(d.query("SELECT * FROM items").errors[0].0, 1146);
    d.core
        .change_user(
            10,
            "sql_user".into(),
            b"fixture-password".to_vec(),
            Some("turnloop_test".into()),
        )
        .expect("fixture operation must succeed");
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    assert_eq!(
        d.query("SELECT 123").rows[0][0],
        Value::Bytes(b"123".to_vec())
    );
    d.core.quit().expect("fixture operation must succeed");
    d.flush();
    assert!(matches!(
        d.core.next_event().expect("fixture operation must succeed"),
        Some(Event::Closed { .. })
    ));
}

#[test]
#[ignore = "requires private MySQL 9.6; use scripts/test-servers.py run"]
fn common_column_types_and_binary_null_bitmap() {
    let mut d = Driver::connect("sql_user", false, false, false);
    let r=d.query("CREATE TEMPORARY TABLE types_fixture (id TINYINT, n BIGINT UNSIGNED, deci DECIMAL(30,4), d DATE, dt DATETIME(6), j JSON, b BLOB, s VARCHAR(30), f DOUBLE); INSERT INTO types_fixture VALUES (1,18446744073709551615,12345678901234567890.2300,'2026-09-14','2026-09-14 12:34:56.123456','{\"x\":1}',X'00FF','hello',1.25)");
    assert!(r.errors.is_empty(), "{r:?}");
    assert!(r.oks.iter().any(|v| v.0 == 1));
    let text = d.query("SELECT * FROM types_fixture");
    assert_eq!(text.rows.len(), 1);
    assert_eq!(
        text.rows[0][1],
        Value::Bytes(b"18446744073709551615".to_vec())
    );
    assert_eq!(
        text.rows[0][2],
        Value::Bytes(b"12345678901234567890.2300".to_vec())
    );
    assert_eq!(text.rows[0][6], Value::Bytes(vec![0, 255]));
    d.core
        .prepare(2, "SELECT * FROM types_fixture WHERE id=?", None)
        .expect("fixture operation must succeed");
    let stmt = d
        .drain(None)
        .statement
        .expect("fixture operation must succeed");
    d.core
        .execute(3, stmt.id, &[Value::Int(1)], None)
        .expect("fixture operation must succeed");
    let r = d.drain(None);
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::Int(1));
    assert_eq!(r.rows[0][1], Value::UInt(u64::MAX));
    assert_eq!(r.rows[0][2], text.rows[0][2]);
    assert_eq!(r.rows[0][3], Value::Date(2026, 9, 14, 0, 0, 0, 0));
    assert_eq!(r.rows[0][4], Value::Date(2026, 9, 14, 12, 34, 56, 123456));
    assert_eq!(r.rows[0][5], text.rows[0][5]);
    assert_eq!(r.rows[0][6], Value::Bytes(vec![0, 255]));
    assert_eq!(r.rows[0][7], Value::Bytes(b"hello".to_vec()));
    assert_eq!(r.rows[0][8], Value::Double(1.25));
    d.query("UPDATE types_fixture SET n=NULL, deci=NULL, d=NULL, dt=NULL, j=NULL, b=NULL, s=NULL, f=NULL");
    d.core
        .execute(4, stmt.id, &[Value::Int(1)], None)
        .expect("fixture operation must succeed");
    let r = d.drain(None);
    assert_eq!(r.rows.len(), 1);
    assert!(r.rows[0][1..].iter().all(|v| *v == Value::NULL));
}

#[test]
#[ignore = "requires private MySQL 9.6; use scripts/test-servers.py run"]
fn pool_reuses_real_session_and_closes_on_end() {
    use turnloop_mysql::pool::{Config, Event, Pool};
    let now = std::time::Instant::now();
    let mut p = Pool::new(Config {
        max: 1,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    p.checkout(1, now, None)
        .expect("fixture operation must succeed");
    let Some(Event::Connect(id)) = p.next_event() else {
        panic!()
    };
    let mut connection = Driver::connect("sql_user", false, false, false);
    assert_eq!(
        connection.query("SET @pooled_value=42").completed,
        [Outcome::Success]
    );
    p.connected(id, now)
        .expect("fixture operation must succeed");
    p.next_event();
    let Some(Event::Acquired(a)) = p.next_event() else {
        panic!()
    };
    p.checkout(2, now, None)
        .expect("fixture operation must succeed");
    assert_eq!(p.waiting_count(), 1);
    p.checkin(a, now, false)
        .expect("fixture operation must succeed");
    p.next_event();
    let Some(Event::Acquired(b)) = p.next_event() else {
        panic!()
    };
    assert_eq!(b.connection, id);
    assert_eq!(b.token, 2);
    assert_eq!(
        connection.query("SELECT @pooled_value").rows[0][0],
        Value::Bytes(b"42".to_vec())
    );
    p.end().expect("fixture operation must succeed");
    assert_eq!(p.next_event(), Some(Event::Close(id)));
    connection
        .core
        .quit()
        .expect("fixture operation must succeed");
    connection.flush();
    assert!(matches!(
        connection
            .core
            .next_event()
            .expect("fixture operation must succeed"),
        Some(turnloop_mysql::Event::Closed { .. })
    ));
    p.closed(id, now).expect("fixture operation must succeed");
    assert_eq!(p.next_event(), Some(Event::Removed(id)));
    assert_eq!(p.next_event(), Some(Event::Ended));
    assert_eq!(p.total_count(), 0);
}
