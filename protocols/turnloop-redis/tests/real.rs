#![cfg(not(target_arch = "wasm32"))]
#![deny(unsafe_op_in_unsafe_fn)]
//! Run through `python3 scripts/test-servers.py run cargo test --workspace -- --include-ignored`. Missing servers are a failure,
//! never a skipped assertion. Only explicitly supplied private ports are used.
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::{Duration, Instant},
};
use turnloop_redis::{
    Config, Connection, Event, State,
    resp::Value,
    routing::{DiscoveryAction, Endpoint, Redirect, SentinelDiscovery, SlotMap, key_slot},
};
trait Socket: Read + Write {}
impl<T: Read + Write> Socket for T {}
struct Driver {
    core: Connection,
    socket: Box<dyn Socket>,
    id: u64,
}
impl Driver {
    fn connect(port: u16, config: Config) -> Self {
        let stream =
            TcpStream::connect(("127.0.0.1", port)).expect("fixture operation must succeed");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fixture operation must succeed");
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .expect("fixture operation must succeed");
        let tls = config.tls;
        let mut core = Connection::new(config);
        core.connect(Instant::now())
            .expect("fixture operation must succeed");
        assert_eq!(core.poll_event(), Some(Event::Connect));
        core.transport_connected()
            .expect("fixture operation must succeed");
        let socket: Box<dyn Socket> = if tls {
            assert_eq!(core.poll_event(), Some(Event::UpgradeTls));
            let mut roots = rustls::RootCertStore::empty();
            roots
                .add(rustls::pki_types::CertificateDer::from(
                    include_bytes!("fixtures/ca.der").to_vec(),
                ))
                .expect("fixture operation must succeed");
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let mut tls = rustls::StreamOwned::new(
                rustls::ClientConnection::new(
                    Arc::new(config),
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
            core.tls_established()
                .expect("fixture operation must succeed");
            Box::new(tls)
        } else {
            Box::new(stream)
        };
        let mut d = Self {
            core,
            socket,
            id: 0,
        };
        assert!(matches!(d.event(), Event::Ready { .. }));
        d
    }
    fn event(&mut self) -> Event {
        loop {
            if let Some(e) = self.core.poll_event() {
                return e;
            }
            self.flush();
            let mut bytes = [0; 8192];
            let n = self
                .socket
                .read(&mut bytes)
                .expect("fixture operation must succeed");
            assert!(n > 0, "server EOF");
            self.core
                .receive(&bytes[..n])
                .expect("fixture operation must succeed");
        }
    }
    fn flush(&mut self) {
        self.socket
            .write_all(self.core.output())
            .expect("fixture operation must succeed");
        self.socket.flush().expect("fixture operation must succeed");
        self.core.consume_output(self.core.output().len());
    }
    fn query(&mut self, args: &[&[u8]]) -> Result<Value, turnloop_redis::Error> {
        self.id += 1;
        self.core
            .command(self.id, args, None)
            .expect("fixture operation must succeed");
        match self.event() {
            Event::Reply { token, result } => {
                assert_eq!(token, self.id);
                result
            }
            event => panic!("Unexpected {event:?}"),
        }
    }
}
fn port(name: &str) -> u16 {
    std::env::var(name)
        .expect("run scripts/test-servers.py run cargo test --workspace -- --include-ignored")
        .parse()
        .expect("fixture operation must succeed")
}
fn auth() -> Config {
    Config {
        username: Some("lane".into()),
        password: Some(
            std::env::var("TURNLOOP_TEST_REDIS_PASSWORD").expect("fixture operation must succeed"),
        ),
        database: 2,
        client_name: Some("turnloop-lane".into()),
        ..Config::default()
    }
}
#[test]
#[ignore = "private Redis: python3 scripts/test-servers.py run cargo test --workspace -- --include-ignored"]
fn real_redis_commands_transactions_pubsub_reconnect_tls() {
    let mut d = Driver::connect(port("TURNLOOP_TEST_REDIS_PORT"), auth());
    assert_eq!(
        d.query(&[b"CLIENT", b"GETNAME"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"turnloop-lane".as_slice())
    );
    assert_eq!(
        d.query(&[b"SET", b"binary", b"a\0\xff"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(
        d.query(&[b"GET", b"binary"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"a\0\xff".as_slice())
    );
    assert_eq!(
        d.query(&[b"GET", b"missing"])
            .expect("fixture operation must succeed"),
        Value::Null
    );
    for (id, args) in [
        (100, vec![b"INCR".as_slice(), b"counter"]),
        (101, vec![b"INCR".as_slice(), b"counter"]),
        (102, vec![b"GET".as_slice(), b"counter"]),
    ] {
        d.core
            .command(id, &args, None)
            .expect("fixture operation must succeed");
    }
    assert!(matches!(
        d.event(),
        Event::Reply {
            token: 100,
            result: Ok(Value::Integer(1))
        }
    ));
    assert!(matches!(
        d.event(),
        Event::Reply {
            token: 101,
            result: Ok(Value::Integer(2))
        }
    ));
    assert!(
        matches!(d.event(), Event::Reply { token: 102, result: Ok(Value::Bulk(v)) } if v == b"2")
    );
    assert_eq!(
        d.query(&[b"MULTI"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(
        d.query(&[b"SET", b"tx", b"committed"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"QUEUED".as_slice())
    );
    assert_eq!(
        d.query(&[b"EXEC"]).expect("fixture operation must succeed"),
        Value::Array(vec![Value::Simple(b"OK".to_vec())])
    );
    d.query(&[b"MULTI"])
        .expect("fixture operation must succeed");
    d.query(&[b"SET", b"tx", b"discarded"])
        .expect("fixture operation must succeed");
    d.query(&[b"DISCARD"])
        .expect("fixture operation must succeed");
    assert_eq!(
        d.query(&[b"GET", b"tx"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"committed".as_slice())
    );
    let mut other = Driver::connect(port("TURNLOOP_TEST_REDIS_PORT"), auth());
    d.query(&[b"WATCH", b"tx"])
        .expect("fixture operation must succeed");
    other
        .query(&[b"SET", b"tx", b"conflict"])
        .expect("fixture operation must succeed");
    d.query(&[b"MULTI"])
        .expect("fixture operation must succeed");
    d.query(&[b"SET", b"tx", b"wrong"])
        .expect("fixture operation must succeed");
    assert_eq!(
        d.query(&[b"EXEC"]).expect("fixture operation must succeed"),
        Value::Null
    );
    assert_eq!(
        d.query(&[b"HSET", b"hash", b"a", b"b"])
            .expect("fixture operation must succeed"),
        Value::Integer(1)
    );
    assert!(
        matches!(d.query(&[b"HGETALL", b"hash"]).expect("fixture operation must succeed"), Value::Map(v) if v.len() == 1 && v[0].0.bytes() == Some(b"a"))
    );
    d.query(&[b"SADD", b"set", b"x", b"y"])
        .expect("fixture operation must succeed");
    assert!(
        matches!(d.query(&[b"SMEMBERS", b"set"]).expect("fixture operation must succeed"), Value::Set(v) if v.len() == 2)
    );
    let error = d.query(&[b"GET", b"hash"]).unwrap_err();
    assert_eq!(error.name, "ReplyError");
    assert!(error.message.starts_with("WRONGTYPE"));
    assert_eq!(
        d.query(&[b"BLPOP", b"queue", b"0.05"])
            .expect("fixture operation must succeed"),
        Value::Null
    );
    d.core
        .command(
            500,
            &[b"BLPOP", b"queue", b"0"],
            Some(Instant::now() + Duration::from_secs(2)),
        )
        .expect("fixture operation must succeed");
    d.flush();
    other
        .query(&[b"LPUSH", b"queue", b"pushed"])
        .expect("fixture operation must succeed");
    assert!(
        matches!(d.event(), Event::Reply { token: 500, result: Ok(Value::Array(v)) } if v[1].bytes() == Some(b"pushed"))
    );
    assert_eq!(
        d.query(&[b"SUBSCRIBE", b"news", b"alerts"])
            .expect("fixture operation must succeed"),
        Value::Integer(2)
    );
    assert_eq!(
        d.query(&[b"PSUBSCRIBE", b"news*"])
            .expect("fixture operation must succeed"),
        Value::Integer(3)
    );
    assert!(d.core.command(501, &[b"GET", b"tx"], None).is_err());
    assert_eq!(
        other
            .query(&[b"PUBLISH", b"news", b"payload"])
            .expect("fixture operation must succeed"),
        Value::Integer(2)
    );
    let mut messages = vec![d.event(), d.event()];
    assert!(messages.iter().any(|e| matches!(e, Event::Message { pattern: None, channel, payload } if channel == b"news" && payload == b"payload")));
    assert!(
        messages
            .iter()
            .any(|e| matches!(e, Event::Message { pattern: Some(p), .. } if p == b"news*"))
    );
    messages.clear();
    // Real reconnect reruns AUTH/SELECT/SETNAME and restores subscriptions.
    d.core.transport_lost();
    assert_eq!(d.event(), Event::CloseTransport);
    assert_eq!(d.event(), Event::Retry { attempt: 1 });
    let now = Instant::now();
    d.core
        .retry(now, Some(Duration::ZERO))
        .expect("fixture operation must succeed");
    d.core.handle_timeout(now);
    assert_eq!(d.event(), Event::Connect);
    let stream = TcpStream::connect(("127.0.0.1", port("TURNLOOP_TEST_REDIS_PORT")))
        .expect("fixture operation must succeed");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fixture operation must succeed");
    d.socket = Box::new(stream);
    d.core
        .transport_connected()
        .expect("fixture operation must succeed");
    assert!(matches!(d.event(), Event::Ready { .. }));
    other
        .query(&[b"PUBLISH", b"news", b"again"])
        .expect("fixture operation must succeed");
    assert!(matches!(d.event(), Event::Message { payload, .. } if payload == b"again"));
    d.event();
    assert_eq!(
        d.query(&[b"UNSUBSCRIBE"])
            .expect("fixture operation must succeed"),
        Value::Integer(1)
    );
    assert_eq!(
        d.query(&[b"PUNSUBSCRIBE"])
            .expect("fixture operation must succeed"),
        Value::Integer(0)
    );
    assert_eq!(
        d.query(&[b"CLIENT", b"GETNAME"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"turnloop-lane".as_slice())
    );
    let mut tls = Driver::connect(
        port("TURNLOOP_TEST_REDIS_TLS_PORT"),
        Config {
            tls: true,
            prefer_resp3: false,
            ..auth()
        },
    );
    assert_eq!(
        tls.query(&[b"GET", b"binary"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"a\0\xff".as_slice())
    );
    assert_eq!(
        tls.query(&[b"QUIT"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(tls.core.state(), State::Closed);
}
#[test]
#[ignore = "private three-master cluster and Sentinel: python3 scripts/test-servers.py run cargo test --workspace -- --include-ignored"]
fn real_cluster_slots_shards_moved_ask_and_sentinel() {
    let mut seed = Driver::connect(port("TURNLOOP_TEST_REDIS_CLUSTER_PORT"), Config::default());
    let mut map = SlotMap::new();
    map.update_slots(
        &seed
            .query(&[b"CLUSTER", b"SLOTS"])
            .expect("fixture operation must succeed"),
        "127.0.0.1",
    )
    .expect("fixture operation must succeed");
    for slot in 0..16384 {
        assert!(map.endpoint(slot).is_some());
    }
    map.update_shards(
        &seed
            .query(&[b"CLUSTER", b"SHARDS"])
            .expect("fixture operation must succeed"),
        "127.0.0.1",
    )
    .expect("fixture operation must succeed");
    let key = (0..10000)
        .map(|i| format!("key{i}"))
        .find(|k| {
            map.endpoint(key_slot(k.as_bytes()))
                .expect("fixture operation must succeed")
                .port
                != port("TURNLOOP_TEST_REDIS_CLUSTER_PORT")
        })
        .expect("fixture operation must succeed");
    let redirect = Redirect::parse(
        &seed
            .query(&[b"SET", key.as_bytes(), b"routed"])
            .unwrap_err(),
    )
    .expect("fixture operation must succeed");
    assert!(!redirect.asking);
    map.apply_redirect(&redirect)
        .expect("fixture operation must succeed");
    let endpoint = map
        .route_command(&[b"GET", key.as_bytes()])
        .expect("fixture operation must succeed")
        .expect("fixture operation must succeed")
        .clone();
    assert_eq!(endpoint, redirect.endpoint);
    let mut owner = Driver::connect(endpoint.port, Config::default());
    owner
        .query(&[b"SET", key.as_bytes(), b"routed"])
        .expect("fixture operation must succeed");
    assert_eq!(
        owner
            .query(&[b"GET", key.as_bytes()])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"routed".as_slice())
    );
    assert!(
        map.route_command(&[b"MGET", b"a", b"b"])
            .unwrap_err()
            .message
            .starts_with("CROSSSLOT")
    );
    let tags = [b"{pair}:1".as_slice(), b"{pair}:2".as_slice()];
    let endpoint = map
        .route_keys(&tags)
        .expect("fixture operation must succeed")
        .expect("fixture operation must succeed");
    let mut same = Driver::connect(endpoint.port, Config::default());
    same.query(&[b"MSET", tags[0], b"one", tags[1], b"two"])
        .expect("fixture operation must succeed");
    assert_eq!(
        same.query(&[b"MGET", tags[0], tags[1]])
            .expect("fixture operation must succeed"),
        Value::Array(vec![
            Value::Bulk(b"one".to_vec()),
            Value::Bulk(b"two".to_vec())
        ])
    );
    // Put one empty key's slot into the migration state to get a REAL ASK.
    owner
        .query(&[b"DEL", key.as_bytes()])
        .expect("fixture operation must succeed");
    let slot = key_slot(key.as_bytes()).to_string();
    let source_id = owner
        .query(&[b"CLUSTER", b"MYID"])
        .expect("fixture operation must succeed");
    let dest_id = seed
        .query(&[b"CLUSTER", b"MYID"])
        .expect("fixture operation must succeed");
    seed.query(&[
        b"CLUSTER",
        b"SETSLOT",
        slot.as_bytes(),
        b"IMPORTING",
        source_id.bytes().expect("fixture operation must succeed"),
    ])
    .expect("fixture operation must succeed");
    owner
        .query(&[
            b"CLUSTER",
            b"SETSLOT",
            slot.as_bytes(),
            b"MIGRATING",
            dest_id.bytes().expect("fixture operation must succeed"),
        ])
        .expect("fixture operation must succeed");
    let ask = Redirect::parse(&owner.query(&[b"GET", key.as_bytes()]).unwrap_err())
        .expect("fixture operation must succeed");
    assert!(ask.asking);
    assert_eq!(ask.endpoint.port, port("TURNLOOP_TEST_REDIS_CLUSTER_PORT"));
    map.apply_redirect(&ask)
        .expect("fixture operation must succeed");
    assert_eq!(
        map.endpoint(ask.slot)
            .expect("fixture operation must succeed")
            .port,
        redirect.endpoint.port
    );
    seed.query(&[b"ASKING"])
        .expect("fixture operation must succeed");
    assert_eq!(
        seed.query(&[b"GET", key.as_bytes()])
            .expect("fixture operation must succeed"),
        Value::Null
    );
    owner
        .query(&[b"CLUSTER", b"SETSLOT", slot.as_bytes(), b"STABLE"])
        .expect("fixture operation must succeed");
    seed.query(&[b"CLUSTER", b"SETSLOT", slot.as_bytes(), b"STABLE"])
        .expect("fixture operation must succeed");
    let mut discovery = SentinelDiscovery::new(
        vec![Endpoint {
            host: "127.0.0.1".into(),
            port: port("TURNLOOP_TEST_REDIS_SENTINEL_PORT"),
        }],
        "turnloop".into(),
    )
    .expect("fixture operation must succeed");
    let Some(DiscoveryAction::QueryMaster { sentinel, name }) = discovery.poll_action() else {
        panic!("query Sentinel")
    };
    let mut sentinel = Driver::connect(sentinel.port, Config::default());
    discovery
        .reply(
            &sentinel
                .query(&[b"SENTINEL", b"get-master-addr-by-name", name.as_bytes()])
                .expect("fixture operation must succeed"),
        )
        .expect("fixture operation must succeed");
    let Some(DiscoveryAction::VerifyRole { candidate }) = discovery.poll_action() else {
        panic!("verify master")
    };
    assert_eq!(candidate.port, port("TURNLOOP_TEST_REDIS_PORT"));
    let mut discovered = Driver::connect(candidate.port, auth());
    discovery
        .reply(
            &discovered
                .query(&[b"ROLE"])
                .expect("fixture operation must succeed"),
        )
        .expect("fixture operation must succeed");
    assert_eq!(
        discovery.poll_action(),
        Some(DiscoveryAction::Discovered(candidate))
    );
    assert_eq!(
        discovered
            .query(&[b"PING"])
            .expect("fixture operation must succeed")
            .bytes(),
        Some(b"PONG".as_slice())
    );
}
