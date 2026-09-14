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
        .unwrap();
        let port: u16 = std::env::var("TURNLOOP_MYSQL_PORT")
            .expect("private server port required")
            .parse()
            .unwrap();
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
        self.io.write_all(self.core.output()).unwrap();
        let n = self.core.output().len();
        self.core.consume_output(n).unwrap();
        self.io.flush().unwrap();
    }
    fn step(&mut self, r: &mut Results, infile: Option<&[u8]>) {
        self.flush();
        let mut any = false;
        while let Some(event) = self.core.next_event().unwrap() {
            any = true;
            match event {
                Event::Progress | Event::Connected { .. } | Event::ColumnCount { .. } => {}
                Event::UpgradeTls => {
                    self.flush();
                    self.io.upgrade();
                    self.core.tls_established().unwrap();
                    self.tls += 1;
                }
                Event::AuthFastSuccess => self.fast += 1,
                Event::AuthFull => self.full += 1,
                Event::RsaSeedNeeded => {
                    let mut seed = [0; 20];
                    rustls::crypto::ring::default_provider()
                        .secure_random
                        .fill(&mut seed)
                        .unwrap();
                    self.core.rsa_seed(seed).unwrap();
                    self.rsa += 1;
                }
                Event::Column { column, .. } => r
                    .columns
                    .push(String::from_utf8(column.name.to_vec()).unwrap()),
                Event::Row { row, .. } => r.rows.push(
                    row.map(|v| match v.unwrap() {
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
                        .unwrap();
                    self.core.local_infile_finish().unwrap();
                }
                Event::Closed { reason } => panic!("unexpected close: {reason}"),
            }
        }
        self.flush();
        if !any {
            let mut b = [0; 4096];
            let n = self.io.read(&mut b).unwrap();
            assert!(n > 0, "unexpected EOF");
            self.core.receive(&b[..n]).unwrap();
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
        self.core.query(1, sql, None).unwrap();
        self.drain(None)
    }
}
#[test]
#[ignore = "requires private MySQL 9.6; use scripts/sql-servers.py run"]
fn auth_prepared_transactions_compression_and_infile() {
    // First connection must encounter an empty caching_sha2 server cache.
    let mut d = Driver::connect("sql_user", false, false, true);
    assert_eq!(d.full, 1);
    assert_eq!(d.rsa, 1);
    let cached = Driver::connect("sql_user", false, false, false);
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
        .unwrap();
    let stmt = d.drain(None).statement.unwrap();
    assert_eq!(stmt.parameters, 2);
    assert_eq!(stmt.columns, 2);
    d.core
        .execute(
            3,
            stmt.id,
            &[Value::Int(42), Value::Bytes(b"hello".to_vec())],
            None,
        )
        .unwrap();
    let r = d.drain(None);
    assert_eq!(
        r.rows,
        vec![vec![Value::Int(42), Value::Bytes(b"hello".to_vec())]]
    );
    d.core.reset_statement(4, stmt.id).unwrap();
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    d.core.close_statement(5, stmt.id).unwrap();
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
    d.core.ping(7).unwrap();
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    d.core
        .query(
            8,
            "LOAD DATA LOCAL INFILE 'fixture.tsv' INTO TABLE items (name)",
            None,
        )
        .unwrap();
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
    d.core.reset_connection(9).unwrap();
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    assert_eq!(d.query("SELECT * FROM items").errors[0].0, 1146);
    d.core
        .change_user(
            10,
            "sql_user".into(),
            b"fixture-password".to_vec(),
            Some("turnloop_test".into()),
        )
        .unwrap();
    assert_eq!(d.drain(None).completed, [Outcome::Success]);
    assert_eq!(
        d.query("SELECT 123").rows[0][0],
        Value::Bytes(b"123".to_vec())
    );
    d.core.quit().unwrap();
    d.flush();
    assert!(matches!(
        d.core.next_event().unwrap(),
        Some(Event::Closed { .. })
    ));
}
