use std::time::{Duration, Instant};
use turnloop_redis::{
    Config, Connection, Event, State,
    resp::{self, DecodeError, Limits, Value},
    routing::{Redirect, SlotMap, key_slot},
};
fn ready(config: Config) -> Connection {
    let mut c = Connection::new(config);
    c.connect(Instant::now())
        .expect("fixture operation must succeed");
    assert_eq!(c.poll_event(), Some(Event::Connect));
    c.transport_connected()
        .expect("fixture operation must succeed");
    if !c.output().is_empty() {
        let n = c.output().len();
        c.consume_output(n);
        c.receive(b"%1\r\n+proto\r\n:3\r\n")
            .expect("fixture operation must succeed");
    }
    assert!(matches!(c.poll_event(), Some(Event::Ready { .. })));
    c
}
#[test]
fn codec_fragments_all_types_and_limits() {
    let frames: &[&[u8]] = &[
        b"+OK\r\n",
        b"-ERR failure\r\n",
        b":-123\r\n",
        b"$3\r\na\0b\r\n",
        b"$-1\r\n",
        b"*-1\r\n",
        b"*2\r\n:1\r\n$1\r\nx\r\n",
        b"%1\r\n+k\r\n:1\r\n",
        b"~2\r\n+a\r\n+b\r\n",
        b">2\r\n+invalidate\r\n_\r\n",
        b"#t\r\n",
        b",1.25\r\n",
        b"(123456789012345678901\r\n",
        b"=7\r\ntxt:abc\r\n",
        b"!3\r\nERR\r\n",
        b"|1\r\n+ttl\r\n:2\r\n+OK\r\n",
    ];
    for frame in frames {
        for end in 0..frame.len() {
            assert_eq!(
                resp::decode(&frame[..end], Limits::default())
                    .expect("fixture operation must succeed"),
                None,
                "{frame:?} at {end}"
            );
        }
        assert_eq!(
            resp::decode(frame, Limits::default())
                .expect("fixture operation must succeed")
                .expect("fixture operation must succeed")
                .1,
            frame.len()
        );
    }
    for bad in [
        b"$3\r\nabcxx".as_slice(),
        b"#x\r\n",
        b":+1\r\n",
        b"*-2\r\n",
        b"+x\n",
        b"=2\r\nxx\r\n",
    ] {
        assert_eq!(
            resp::decode(bad, Limits::default()),
            Err(DecodeError::Invalid)
        );
    }
    assert_eq!(
        resp::decode(b"$999999999\r\n", Limits::default()),
        Err(DecodeError::Limit)
    );
    assert_eq!(
        resp::decode(b"*?\r\n", Limits::default()),
        Err(DecodeError::StreamingUnsupported)
    );
    assert_eq!(
        resp::decode(
            b"*1\r\n*1\r\n*1\r\n:1\r\n",
            Limits {
                depth: 2,
                ..Limits::default()
            }
        ),
        Err(DecodeError::Limit)
    );
}
#[test]
fn fallback_auth_select_name_and_bad_auth() {
    let mut c = Connection::new(Config {
        password: Some("pw".into()),
        database: 2,
        client_name: Some("lane".into()),
        ..Config::default()
    });
    c.connect(Instant::now())
        .expect("fixture operation must succeed");
    c.poll_event();
    c.transport_connected()
        .expect("fixture operation must succeed");
    assert!(c.output().windows(5).any(|w| w == b"HELLO"));
    c.consume_output(c.output().len());
    c.receive(b"-ERR unknown command 'HELLO'\r\n")
        .expect("fixture operation must succeed");
    assert_eq!(c.output(), b"*2\r\n$4\r\nAUTH\r\n$2\r\npw\r\n");
    c.consume_output(c.output().len());
    c.receive(b"+OK\r\n")
        .expect("fixture operation must succeed");
    assert!(c.output().windows(6).any(|w| w == b"SELECT"));
    c.consume_output(c.output().len());
    c.receive(b"+OK\r\n")
        .expect("fixture operation must succeed");
    assert!(c.output().windows(7).any(|w| w == b"SETNAME"));
    c.consume_output(c.output().len());
    c.receive(b"+OK\r\n")
        .expect("fixture operation must succeed");
    assert_eq!(c.poll_event(), Some(Event::Ready { resp3: false }));
    let mut c = Connection::new(Config::default());
    c.connect(Instant::now())
        .expect("fixture operation must succeed");
    c.poll_event();
    c.transport_connected()
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b"-WRONGPASS invalid username-password pair\r\n")
        .expect("fixture operation must succeed");
    assert!(
        matches!(c.poll_event(), Some(Event::Error(e)) if e.name == "ReplyError" && e.message.starts_with("WRONGPASS"))
    );
    assert_eq!(c.state(), State::Closed);
}
#[test]
fn ordered_pipeline_timeout_tombstone_and_close() {
    let mut c = ready(Config::default());
    let now = Instant::now();
    c.command(1, &[b"BLPOP", b"queue", b"0"], Some(now))
        .expect("fixture operation must succeed");
    c.command(2, &[b"INCR", b"counter"], None)
        .expect("fixture operation must succeed");
    c.consume_output(3);
    assert!(!c.output().is_empty());
    c.consume_output(c.output().len());
    assert_eq!(c.next_timeout(), Some(now));
    c.handle_timeout(now);
    assert!(
        matches!(c.poll_event(), Some(Event::Reply { token: 1, result: Err(e) }) if e.message == "Command timed out")
    );
    c.receive(b"_\r\n:42\r\n")
        .expect("fixture operation must succeed");
    assert_eq!(
        c.poll_event(),
        Some(Event::Reply {
            token: 2,
            result: Ok(Value::Integer(42))
        })
    );
    assert_eq!(c.poll_event(), None);
    c.command(3, &[b"PING"], None)
        .expect("fixture operation must succeed");
    c.close();
    c.close();
    assert!(matches!(
        c.poll_event(),
        Some(Event::Reply {
            token: 3,
            result: Err(_)
        })
    ));
    assert_eq!(c.poll_event(), Some(Event::CloseTransport));
    assert_eq!(c.poll_event(), Some(Event::Closed));
    assert_eq!(c.poll_event(), None);
}
#[test]
fn reconnect_offline_order_resubscribe_and_retry_stop() {
    let now = Instant::now();
    let mut c = ready(Config::default());
    c.command(1, &[b"SUBSCRIBE", b"news"], None)
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b">3\r\n+subscribe\r\n$4\r\nnews\r\n:1\r\n")
        .expect("fixture operation must succeed");
    c.poll_event();
    assert!(c.command(2, &[b"GET", b"key"], None).is_err());
    c.transport_lost();
    assert_eq!(c.poll_event(), Some(Event::CloseTransport));
    assert_eq!(c.poll_event(), Some(Event::Retry { attempt: 1 }));
    c.retry(now, Some(Duration::from_millis(50)))
        .expect("fixture operation must succeed");
    assert_eq!(c.next_timeout(), Some(now + Duration::from_millis(50)));
    c.handle_timeout(now + Duration::from_millis(49));
    assert_eq!(c.poll_event(), None);
    c.handle_timeout(now + Duration::from_millis(50));
    assert_eq!(c.poll_event(), Some(Event::Connect));
    c.transport_connected()
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b"%0\r\n")
        .expect("fixture operation must succeed");
    assert!(c.output().windows(9).any(|w| w == b"SUBSCRIBE"));
    c.consume_output(c.output().len());
    c.receive(b">3\r\n+subscribe\r\n$4\r\nnews\r\n:1\r\n")
        .expect("fixture operation must succeed");
    assert_eq!(c.poll_event(), Some(Event::Ready { resp3: true }));
    c.receive(b">3\r\n+message\r\n$4\r\nnews\r\n$3\r\nabc\r\n")
        .expect("fixture operation must succeed");
    assert_eq!(
        c.poll_event(),
        Some(Event::Message {
            pattern: None,
            channel: b"news".to_vec(),
            payload: b"abc".to_vec()
        })
    );
    c.transport_lost();
    c.poll_event();
    c.poll_event();
    c.retry(now, None).expect("fixture operation must succeed");
    assert_eq!(c.state(), State::Closed);
    let mut c = ready(Config::default());
    c.command(7, &[b"INCR", b"x"], None)
        .expect("fixture operation must succeed");
    c.transport_lost();
    c.poll_event();
    c.poll_event();
    c.command(8, &[b"GET", b"x"], None)
        .expect("fixture operation must succeed");
    c.retry(now, Some(Duration::ZERO))
        .expect("fixture operation must succeed");
    c.handle_timeout(now);
    c.poll_event();
    c.transport_connected()
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b"%0\r\n")
        .expect("fixture operation must succeed");
    c.poll_event();
    c.consume_output(c.output().len());
    c.receive(b":1\r\n$1\r\n1\r\n")
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.poll_event(),
        Some(Event::Reply { token: 7, .. })
    ));
    assert!(matches!(
        c.poll_event(),
        Some(Event::Reply { token: 8, .. })
    ));
}
#[test]
fn redis_tls_blocks_all_protocol_bytes() {
    let mut c = Connection::new(Config {
        tls: true,
        ..Config::default()
    });
    c.connect(Instant::now())
        .expect("fixture operation must succeed");
    c.poll_event();
    c.transport_connected()
        .expect("fixture operation must succeed");
    assert_eq!(c.poll_event(), Some(Event::UpgradeTls));
    assert!(c.output().is_empty());
    assert!(c.receive(b"+OK\r\n").is_err());
    c.tls_established().expect("fixture operation must succeed");
    assert!(!c.output().is_empty());
}
#[test]
fn cluster_hashes_and_redirects() {
    assert_eq!(key_slot(b"123456789"), 12739);
    assert_eq!(
        key_slot(b"{user1000}.following"),
        key_slot(b"{user1000}.followers")
    );
    assert_ne!(key_slot(b"foo{}{bar}"), key_slot(b"bar"));
    let e = turnloop_redis::Error {
        name: "ReplyError",
        message: "MOVED 123 [::1]:32100".into(),
    };
    let redirect = Redirect::parse(&e).expect("fixture operation must succeed");
    assert_eq!(redirect.endpoint.host, "::1");
    let mut map = SlotMap::new();
    map.apply_redirect(&redirect)
        .expect("fixture operation must succeed");
    assert_eq!(
        map.endpoint(123)
            .expect("fixture operation must succeed")
            .port,
        32100
    );
    let ask = Redirect {
        asking: true,
        endpoint: turnloop_redis::routing::Endpoint {
            host: "other".into(),
            port: 1,
        },
        ..redirect
    };
    map.apply_redirect(&ask)
        .expect("fixture operation must succeed");
    assert_eq!(
        map.endpoint(123)
            .expect("fixture operation must succeed")
            .port,
        32100
    );
    assert!(
        map.route_keys(&[b"a", b"b"])
            .unwrap_err()
            .message
            .starts_with("CROSSSLOT")
    );
    assert!(map.route_command(&[b"MODULECOMMAND", b"key"]).is_err());
}

#[test]
fn pipelined_subscribe_unsubscribe_all_waits_for_every_ack() {
    let mut c = ready(Config::default());
    c.command(1, &[b"SUBSCRIBE", b"a", b"b"], None)
        .expect("fixture operation must succeed");
    c.command(2, &[b"UNSUBSCRIBE"], None)
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b">3\r\n+subscribe\r\n+a\r\n:1\r\n>3\r\n+subscribe\r\n+b\r\n:2\r\n>3\r\n+unsubscribe\r\n+a\r\n:1\r\n").expect("fixture operation must succeed");
    assert_eq!(
        c.poll_event(),
        Some(Event::Reply {
            token: 1,
            result: Ok(Value::Integer(2))
        })
    );
    assert_eq!(c.poll_event(), None);
    assert!(c.command(3, &[b"GET", b"a"], None).is_err());
    c.receive(b">3\r\n+unsubscribe\r\n+b\r\n:0\r\n")
        .expect("fixture operation must succeed");
    assert_eq!(
        c.poll_event(),
        Some(Event::Reply {
            token: 2,
            result: Ok(Value::Integer(0))
        })
    );
    c.command(3, &[b"GET", b"a"], None)
        .expect("fixture operation must succeed");
    let mut c = ready(Config::default());
    c.command(4, &[b"SUBSCRIBE"], None)
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b"-ERR wrong number of arguments\r\n")
        .expect("fixture operation must succeed");
    c.poll_event();
    c.command(5, &[b"GET", b"a"], None)
        .expect("fixture operation must succeed");
}
#[test]
fn sentinel_seed_iteration_stale_master_retry_and_redirect_budget() {
    use turnloop_redis::routing::{DiscoveryAction, Endpoint, RedirectTracker, SentinelDiscovery};
    let endpoint = Endpoint {
        host: "localhost".into(),
        port: 33333,
    };
    let mut discovery =
        SentinelDiscovery::new(vec![endpoint.clone(), endpoint.clone()], "group".into())
            .expect("fixture operation must succeed");
    assert!(matches!(
        discovery.poll_action(),
        Some(DiscoveryAction::QueryMaster { .. })
    ));
    discovery
        .reply(&Value::Null)
        .expect("fixture operation must succeed");
    assert!(matches!(
        discovery.poll_action(),
        Some(DiscoveryAction::QueryMaster { .. })
    ));
    discovery
        .reply(&Value::Array(vec![
            Value::Bulk(b"localhost".to_vec()),
            Value::Bulk(b"33333".to_vec()),
        ]))
        .expect("fixture operation must succeed");
    assert_eq!(
        discovery.poll_action(),
        Some(DiscoveryAction::VerifyRole {
            candidate: endpoint
        })
    );
    discovery
        .reply(&Value::Array(vec![Value::Bulk(b"slave".to_vec())]))
        .expect("fixture operation must succeed");
    assert_eq!(
        discovery.poll_action(),
        Some(DiscoveryAction::Retry { attempt: 1 })
    );
    let now = Instant::now();
    discovery
        .retry(now, Some(Duration::from_millis(10)))
        .expect("fixture operation must succeed");
    assert_eq!(
        discovery.next_timeout(),
        Some(now + Duration::from_millis(10))
    );
    discovery.handle_timeout(now);
    assert_eq!(discovery.poll_action(), None);
    discovery.handle_timeout(now + Duration::from_millis(10));
    assert!(matches!(
        discovery.poll_action(),
        Some(DiscoveryAction::QueryMaster { .. })
    ));
    let mut tracker = RedirectTracker::new(1);
    let e = turnloop_redis::Error {
        name: "ReplyError",
        message: "MOVED 123 localhost:33333".into(),
    };
    let mut map = SlotMap::new();
    assert!(
        tracker
            .follow(&e, &mut map)
            .expect("fixture operation must succeed")
            .is_some()
    );
    assert!(
        tracker
            .follow(&e, &mut map)
            .unwrap_err()
            .message
            .contains("Too many")
    );
}
