//! Run through `python3 scripts/servers.py test`. Missing servers are a failure,
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
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let tls = config.tls;
        let mut core = Connection::new(config);
        core.connect(Instant::now()).unwrap();
        assert_eq!(core.poll_event(), Some(Event::Connect));
        core.transport_connected().unwrap();
        let socket: Box<dyn Socket> = if tls {
            assert_eq!(core.poll_event(), Some(Event::UpgradeTls));
            let mut roots = rustls::RootCertStore::empty();
            roots
                .add(rustls::pki_types::CertificateDer::from(
                    include_bytes!("fixtures/ca.der").to_vec(),
                ))
                .unwrap();
            let config = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let mut tls = rustls::StreamOwned::new(
                rustls::ClientConnection::new(Arc::new(config), "localhost".try_into().unwrap())
                    .unwrap(),
                stream,
            );
            while tls.conn.is_handshaking() {
                tls.conn.complete_io(&mut tls.sock).unwrap();
            }
            core.tls_established().unwrap();
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
            let n = self.socket.read(&mut bytes).unwrap();
            assert!(n > 0, "server EOF");
            self.core.receive(&bytes[..n]).unwrap();
        }
    }
    fn flush(&mut self) {
        self.socket.write_all(self.core.output()).unwrap();
        self.socket.flush().unwrap();
        self.core.consume_output(self.core.output().len());
    }
    fn query(&mut self, args: &[&[u8]]) -> Result<Value, turnloop_redis::Error> {
        self.id += 1;
        self.core.command(self.id, args, None).unwrap();
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
        .expect("run scripts/servers.py test")
        .parse()
        .unwrap()
}
fn auth() -> Config {
    Config {
        username: Some("lane".into()),
        password: Some(std::env::var("REDIS_PASSWORD").unwrap()),
        database: 2,
        client_name: Some("turnloop-lane".into()),
        ..Config::default()
    }
}
#[test]
#[ignore = "private Redis: python3 scripts/servers.py test"]
fn real_redis_commands_transactions_pubsub_reconnect_tls() {
    let mut d = Driver::connect(port("REDIS_PORT"), auth());
    assert_eq!(
        d.query(&[b"CLIENT", b"GETNAME"]).unwrap().bytes(),
        Some(b"turnloop-lane".as_slice())
    );
    assert_eq!(
        d.query(&[b"SET", b"binary", b"a\0\xff"]).unwrap().bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(
        d.query(&[b"GET", b"binary"]).unwrap().bytes(),
        Some(b"a\0\xff".as_slice())
    );
    assert_eq!(d.query(&[b"GET", b"missing"]).unwrap(), Value::Null);
    for (id, args) in [
        (100, vec![b"INCR".as_slice(), b"counter"]),
        (101, vec![b"INCR".as_slice(), b"counter"]),
        (102, vec![b"GET".as_slice(), b"counter"]),
    ] {
        d.core.command(id, &args, None).unwrap();
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
        d.query(&[b"MULTI"]).unwrap().bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(
        d.query(&[b"SET", b"tx", b"committed"]).unwrap().bytes(),
        Some(b"QUEUED".as_slice())
    );
    assert_eq!(
        d.query(&[b"EXEC"]).unwrap(),
        Value::Array(vec![Value::Simple(b"OK".to_vec())])
    );
    d.query(&[b"MULTI"]).unwrap();
    d.query(&[b"SET", b"tx", b"discarded"]).unwrap();
    d.query(&[b"DISCARD"]).unwrap();
    assert_eq!(
        d.query(&[b"GET", b"tx"]).unwrap().bytes(),
        Some(b"committed".as_slice())
    );
    let mut other = Driver::connect(port("REDIS_PORT"), auth());
    d.query(&[b"WATCH", b"tx"]).unwrap();
    other.query(&[b"SET", b"tx", b"conflict"]).unwrap();
    d.query(&[b"MULTI"]).unwrap();
    d.query(&[b"SET", b"tx", b"wrong"]).unwrap();
    assert_eq!(d.query(&[b"EXEC"]).unwrap(), Value::Null);
    assert_eq!(
        d.query(&[b"HSET", b"hash", b"a", b"b"]).unwrap(),
        Value::Integer(1)
    );
    assert!(
        matches!(d.query(&[b"HGETALL", b"hash"]).unwrap(), Value::Map(v) if v.len() == 1 && v[0].0.bytes() == Some(b"a"))
    );
    d.query(&[b"SADD", b"set", b"x", b"y"]).unwrap();
    assert!(matches!(d.query(&[b"SMEMBERS", b"set"]).unwrap(), Value::Set(v) if v.len() == 2));
    let error = d.query(&[b"GET", b"hash"]).unwrap_err();
    assert_eq!(error.name, "ReplyError");
    assert!(error.message.starts_with("WRONGTYPE"));
    assert_eq!(
        d.query(&[b"BLPOP", b"queue", b"0.05"]).unwrap(),
        Value::Null
    );
    d.core
        .command(
            500,
            &[b"BLPOP", b"queue", b"0"],
            Some(Instant::now() + Duration::from_secs(2)),
        )
        .unwrap();
    d.flush();
    other.query(&[b"LPUSH", b"queue", b"pushed"]).unwrap();
    assert!(
        matches!(d.event(), Event::Reply { token: 500, result: Ok(Value::Array(v)) } if v[1].bytes() == Some(b"pushed"))
    );
    assert_eq!(
        d.query(&[b"SUBSCRIBE", b"news", b"alerts"]).unwrap(),
        Value::Integer(2)
    );
    assert_eq!(
        d.query(&[b"PSUBSCRIBE", b"news*"]).unwrap(),
        Value::Integer(3)
    );
    assert!(d.core.command(501, &[b"GET", b"tx"], None).is_err());
    assert_eq!(
        other.query(&[b"PUBLISH", b"news", b"payload"]).unwrap(),
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
    d.core.retry(now, Some(Duration::ZERO)).unwrap();
    d.core.handle_timeout(now);
    assert_eq!(d.event(), Event::Connect);
    let stream = TcpStream::connect(("127.0.0.1", port("REDIS_PORT"))).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    d.socket = Box::new(stream);
    d.core.transport_connected().unwrap();
    assert!(matches!(d.event(), Event::Ready { .. }));
    other.query(&[b"PUBLISH", b"news", b"again"]).unwrap();
    assert!(matches!(d.event(), Event::Message { payload, .. } if payload == b"again"));
    d.event();
    assert_eq!(d.query(&[b"UNSUBSCRIBE"]).unwrap(), Value::Integer(1));
    assert_eq!(d.query(&[b"PUNSUBSCRIBE"]).unwrap(), Value::Integer(0));
    assert_eq!(
        d.query(&[b"CLIENT", b"GETNAME"]).unwrap().bytes(),
        Some(b"turnloop-lane".as_slice())
    );
    let mut tls = Driver::connect(
        port("REDIS_TLS_PORT"),
        Config {
            tls: true,
            prefer_resp3: false,
            ..auth()
        },
    );
    assert_eq!(
        tls.query(&[b"GET", b"binary"]).unwrap().bytes(),
        Some(b"a\0\xff".as_slice())
    );
    assert_eq!(
        tls.query(&[b"QUIT"]).unwrap().bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(tls.core.state(), State::Closed);
}
#[test]
#[ignore = "private three-master cluster and Sentinel: python3 scripts/servers.py test"]
fn real_cluster_slots_shards_moved_ask_and_sentinel() {
    let mut seed = Driver::connect(port("CLUSTER_PORT"), Config::default());
    let mut map = SlotMap::new();
    map.update_slots(&seed.query(&[b"CLUSTER", b"SLOTS"]).unwrap(), "127.0.0.1")
        .unwrap();
    for slot in 0..16384 {
        assert!(map.endpoint(slot).is_some());
    }
    map.update_shards(&seed.query(&[b"CLUSTER", b"SHARDS"]).unwrap(), "127.0.0.1")
        .unwrap();
    let key = (0..10000)
        .map(|i| format!("key{i}"))
        .find(|k| map.endpoint(key_slot(k.as_bytes())).unwrap().port != port("CLUSTER_PORT"))
        .unwrap();
    let redirect = Redirect::parse(
        &seed
            .query(&[b"SET", key.as_bytes(), b"routed"])
            .unwrap_err(),
    )
    .unwrap();
    assert!(!redirect.asking);
    map.apply_redirect(&redirect).unwrap();
    let endpoint = map
        .route_command(&[b"GET", key.as_bytes()])
        .unwrap()
        .unwrap()
        .clone();
    assert_eq!(endpoint, redirect.endpoint);
    let mut owner = Driver::connect(endpoint.port, Config::default());
    owner.query(&[b"SET", key.as_bytes(), b"routed"]).unwrap();
    assert_eq!(
        owner.query(&[b"GET", key.as_bytes()]).unwrap().bytes(),
        Some(b"routed".as_slice())
    );
    assert!(
        map.route_command(&[b"MGET", b"a", b"b"])
            .unwrap_err()
            .message
            .starts_with("CROSSSLOT")
    );
    let tags = [b"{pair}:1".as_slice(), b"{pair}:2".as_slice()];
    let endpoint = map.route_keys(&tags).unwrap().unwrap();
    let mut same = Driver::connect(endpoint.port, Config::default());
    same.query(&[b"MSET", tags[0], b"one", tags[1], b"two"])
        .unwrap();
    assert_eq!(
        same.query(&[b"MGET", tags[0], tags[1]]).unwrap(),
        Value::Array(vec![
            Value::Bulk(b"one".to_vec()),
            Value::Bulk(b"two".to_vec())
        ])
    );
    // Put one empty key's slot into the migration state to get a REAL ASK.
    owner.query(&[b"DEL", key.as_bytes()]).unwrap();
    let slot = key_slot(key.as_bytes()).to_string();
    let source_id = owner.query(&[b"CLUSTER", b"MYID"]).unwrap();
    let dest_id = seed.query(&[b"CLUSTER", b"MYID"]).unwrap();
    seed.query(&[
        b"CLUSTER",
        b"SETSLOT",
        slot.as_bytes(),
        b"IMPORTING",
        source_id.bytes().unwrap(),
    ])
    .unwrap();
    owner
        .query(&[
            b"CLUSTER",
            b"SETSLOT",
            slot.as_bytes(),
            b"MIGRATING",
            dest_id.bytes().unwrap(),
        ])
        .unwrap();
    let ask = Redirect::parse(&owner.query(&[b"GET", key.as_bytes()]).unwrap_err()).unwrap();
    assert!(ask.asking);
    assert_eq!(ask.endpoint.port, port("CLUSTER_PORT"));
    map.apply_redirect(&ask).unwrap();
    assert_eq!(map.endpoint(ask.slot).unwrap().port, redirect.endpoint.port);
    seed.query(&[b"ASKING"]).unwrap();
    assert_eq!(seed.query(&[b"GET", key.as_bytes()]).unwrap(), Value::Null);
    owner
        .query(&[b"CLUSTER", b"SETSLOT", slot.as_bytes(), b"STABLE"])
        .unwrap();
    seed.query(&[b"CLUSTER", b"SETSLOT", slot.as_bytes(), b"STABLE"])
        .unwrap();
    let mut discovery = SentinelDiscovery::new(
        vec![Endpoint {
            host: "127.0.0.1".into(),
            port: port("SENTINEL_PORT"),
        }],
        "turnloop".into(),
    )
    .unwrap();
    let Some(DiscoveryAction::QueryMaster { sentinel, name }) = discovery.poll_action() else {
        panic!("query Sentinel")
    };
    let mut sentinel = Driver::connect(sentinel.port, Config::default());
    discovery
        .reply(
            &sentinel
                .query(&[b"SENTINEL", b"get-master-addr-by-name", name.as_bytes()])
                .unwrap(),
        )
        .unwrap();
    let Some(DiscoveryAction::VerifyRole { candidate }) = discovery.poll_action() else {
        panic!("verify master")
    };
    assert_eq!(candidate.port, port("REDIS_PORT"));
    let mut discovered = Driver::connect(candidate.port, auth());
    discovery
        .reply(&discovered.query(&[b"ROLE"]).unwrap())
        .unwrap();
    assert_eq!(
        discovery.poll_action(),
        Some(DiscoveryAction::Discovered(candidate))
    );
    assert_eq!(
        discovered.query(&[b"PING"]).unwrap().bytes(),
        Some(b"PONG".as_slice())
    );
}
