#[path = "support/clock.rs"]
mod clock;
use std::time::Duration;
use turnloop_mongodb::{
    Connection, ConnectionEvent, Error, ErrorKind,
    auth::{Mechanism, Scram},
    bson::{Document, doc, raw::RawDocumentBuf},
    command::{Command, Cursor, CursorBatch},
    pool::{Pool, PoolEvent, PoolOptions},
    retry::{Retry, RetryKind},
    session::{Session, TransactionState},
    uri::{Address, Credential, Options},
    wire::{self, BsonWriter, Decoder, Message},
};
fn raw(d: &Document) -> RawDocumentBuf {
    RawDocumentBuf::try_from(d).unwrap()
}
fn feed(c: &mut Connection, bytes: &[u8]) {
    let mut at = 0;
    while at < bytes.len() {
        let n = c.receive(&bytes[at..]).unwrap();
        assert!(n > 0);
        at += n;
    }
}
fn ready() -> Connection {
    let mut c = Connection::new(Options::parse("mongodb://localhost/").unwrap());
    c.connected(clock::now(), "").unwrap();
    let req = Message::parse(c.transmit(), wire::DEFAULT_MAX_MESSAGE)
        .unwrap()
        .request_id;
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    let mut b = Vec::new();
    wire::encode(
        &mut b,
        2,
        req,
        0,
        &raw(&doc! {"ok":1,"maxWireVersion":27}),
        &[],
        wire::DEFAULT_MAX_MESSAGE,
    )
    .unwrap();
    feed(&mut c, &b);
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Ready)));
    c
}
#[test]
fn op_msg_fragmentation_sequences_checksums_and_malformed() {
    let body = raw(&doc! {"insert":"x","$db":"test"});
    let item = raw(&doc! {"_id":1,"nested":{"b":[1,2,"three"]}});
    let mut b = Vec::new();
    wire::encode(
        &mut b,
        42,
        3,
        wire::CHECKSUM,
        &body,
        &[("documents", &[&item, &item])],
        10000,
    )
    .unwrap();
    assert_eq!(wire::crc32c(b"123456789"), 0xe3069283);
    let parsed = Message::parse(&b, 10000).unwrap();
    assert_eq!(parsed.request_id, 42);
    let seq = parsed.sequences().next().unwrap();
    assert_eq!(seq.identifier, "documents");
    assert_eq!(seq.documents().count(), 2);
    for split in 0..b.len() {
        let mut decoder = Decoder::new(10000);
        for part in [&b[..split], &b[split..]] {
            let mut at = 0;
            while at < part.len() {
                let n = decoder.feed(&part[at..]).unwrap();
                assert!(n > 0);
                at += n;
            }
        }
        assert!(decoder.complete());
        assert_eq!(decoder.bytes(), b);
    }
    let mut corrupt = b.clone();
    corrupt[25] ^= 1;
    assert!(Message::parse(&corrupt, 10000).is_err());
    for n in 0..b.len() {
        assert!(Message::parse(&b[..n], 10000).is_err());
    }
    let mut duplicate = Vec::new();
    wire::encode(
        &mut duplicate,
        1,
        0,
        0,
        &body,
        &[("documents", &[&item]), ("documents", &[])],
        10000,
    )
    .unwrap();
    assert!(Message::parse(&duplicate, 10000).is_err());
    let mut decoder = Decoder::new(100);
    assert!(decoder.feed(&101i32.to_le_bytes()).is_err());
}
#[test]
fn raw_builder_preserves_order_and_types() {
    let mut w = BsonWriter::new();
    w.clear();
    w.string("find", "items").unwrap();
    let options = raw(&doc! {"sort":{"x":-1},"limit":4_i64,"singleBatch":true,"$db":"bad"});
    w.append_fields(&options, &["$db"]).unwrap();
    w.string("$db", "good").unwrap();
    let d: Document = w.finish().unwrap().try_into().unwrap();
    assert_eq!(d.keys().next().unwrap(), "find");
    assert_eq!(d.get_i64("limit").unwrap(), 4);
    assert_eq!(d.get_str("$db").unwrap(), "good");
    assert_eq!(d.get_document("sort").unwrap().get_i32("x").unwrap(), -1);
}
#[test]
fn uri_dns_and_option_errors() {
    let mut o = Options::parse(
        "mongodb+srv://u:p%40ss@a.example.org/db?authSource=explicit&retryWrites=false",
    )
    .unwrap();
    assert!(o.tls);
    assert_eq!(o.resolution_requests().unwrap().len(), 2);
    o.resolve(
        &[Address::parse("b.example.org:40000").unwrap()],
        &["authSource=txt&replicaSet=r".into()],
    )
    .unwrap();
    assert_eq!(o.credential.unwrap().source, "explicit");
    assert_eq!(o.replica_set.as_deref(), Some("r"));
    assert!(!o.retry_writes);
    for uri in [
        "http://a",
        "mongodb://a:0",
        "mongodb://[bad]",
        "mongodb://u:p@ss@a",
        "mongodb://a/?tls=yes",
        "mongodb://a/?minPoolSize=10&maxPoolSize=2",
        "mongodb://a/?tls=true&ssl=false",
        "mongodb://a,b/?directConnection=true",
        "mongodb+srv://a.example:42/",
        "mongodb://a/?heartbeatFrequencyMS=499",
        "mongodb://a/?madeUp=x",
        "mongodb://a/%GG",
    ] {
        assert!(Options::parse(uri).is_err(), "{uri}");
    }
    let o = Options::parse("mongodb://u%40ser:p%3Ass@[::1]:40000/db?authMechanism=SCRAM-SHA-256")
        .unwrap();
    assert_eq!(o.seeds[0].host, "::1");
    assert_eq!(o.credential.unwrap().username, "u@ser");
    let mut srv = Options::parse("mongodb+srv://a.example.org/").unwrap();
    assert!(
        srv.resolve(&[Address::parse("evil-example.org").unwrap()], &[])
            .is_err()
    );
}
#[test]
fn scram_spec_vectors_and_rejections() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    for (mechanism, nonce, server, proof, signature) in [
        (
            Mechanism::Sha1,
            "fyko+d2lbbFgONRv9qkxdawL",
            "r=fyko+d2lbbFgONRv9qkxdawLHo+Vgk7qvUOKUwuWLIWg4l/9SraGMHEE,s=rQ9ZY3MntBeuP3E1TDVC4w==,i=10000",
            "MC2T8BvbmWRckDw8oWl5IVghwCY=",
            "UMWeI25JD1yNYZRMpZ4VHvhZ9e0=",
        ),
        (
            Mechanism::Sha256,
            "rOprNGfwEbeRWgbNEkqO",
            "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096",
            "dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=",
            "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=",
        ),
    ] {
        let cred = Credential {
            username: "user".into(),
            password: "pencil".into(),
            source: "admin".into(),
            mechanism: Some(mechanism),
        };
        let mut s = Scram::new(&cred, mechanism, nonce).unwrap();
        let start = s.start("admin", true);
        assert_eq!(start.get_str("db").unwrap(), "admin");
        let reply = doc! {"conversationId":7,"done":false,"payload":turnloop_mongodb::bson::Binary{subtype:turnloop_mongodb::bson::spec::BinarySubtype::Generic,bytes:server.as_bytes().to_vec()}};
        let next = s.receive(&reply, "admin").unwrap().unwrap();
        assert!(
            std::str::from_utf8(next.get_binary_generic("payload").unwrap())
                .unwrap()
                .ends_with(proof)
        );
        let reply = doc! {"conversationId":7,"done":true,"payload":turnloop_mongodb::bson::Binary{subtype:turnloop_mongodb::bson::spec::BinarySubtype::Generic,bytes:format!("v={signature}").into_bytes()}};
        assert!(s.receive(&reply, "admin").unwrap().is_none());
        assert_eq!(
            STANDARD.decode(signature).unwrap().len(),
            if mechanism == Mechanism::Sha1 { 20 } else { 32 }
        );
        for payload in [
            server.replace("i=4096", "i=1").replace("i=10000", "i=1"),
            server.replace("r=", "r=bad"),
            format!("{server},i=4096"),
        ] {
            let mut s = Scram::new(&cred, mechanism, nonce).unwrap();
            s.start("admin", false);
            let reply = doc! {"conversationId":1,"done":false,"payload":turnloop_mongodb::bson::Binary{subtype:turnloop_mongodb::bson::spec::BinarySubtype::Generic,bytes:payload.into_bytes()}};
            assert!(s.receive(&reply, "admin").is_err());
        }
    }
}
#[test]
fn connection_token_close_timeout_and_backpressure() {
    let mut c = ready();
    let body = raw(&doc! {"ping":1,"$db":"admin"});
    c.command(44, &body, &[], clock::now()).unwrap();
    assert!(c.command(45, &body, &[], clock::now()).is_err());
    let req = Message::parse(c.transmit(), wire::DEFAULT_MAX_MESSAGE)
        .unwrap()
        .request_id;
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    let mut b = Vec::new();
    wire::encode(&mut b, 100, req, 0, &raw(&doc! {"ok":1}), &[], 1000).unwrap();
    feed(&mut c, &b);
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Reply { token: 44 })
    ));
    assert_eq!(c.reply().unwrap().get_i32("ok").unwrap(), 1);
    assert!(c.command(46, &body, &[], clock::now()).is_err());
    c.release_reply().unwrap();
    c.command(47, &body, &[], clock::now()).unwrap();
    c.close();
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Failed {
            token: Some(47),
            ..
        })
    ));
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Closed)));
    c.close();
    assert!(c.poll_event().is_none());
    let now = clock::now();
    let mut c =
        Connection::new(Options::parse("mongodb://a/?tls=true&connectTimeoutMS=10").unwrap());
    c.connected(now, "").unwrap();
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::UpgradeTls)));
    assert!(c.transmit().is_empty());
    assert_eq!(c.next_timeout(), Some(now + Duration::from_millis(10)));
    c.handle_timeout(now + Duration::from_millis(10));
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Failed { token: None, .. })
    ));
}
#[test]
fn pool_fifo_generation_timeout_and_close() {
    let now = clock::now();
    let mut pool = Pool::new(PoolOptions {
        max_size: 1,
        min_size: 1,
        wait_timeout: Duration::from_millis(10),
        ..PoolOptions::default()
    })
    .unwrap();
    pool.ready(now);
    let lease = match pool.poll_event().unwrap() {
        PoolEvent::Connect(l) => l,
        e => panic!("{e:?}"),
    };
    pool.checkout(1, now).unwrap();
    pool.checkout(2, now).unwrap();
    pool.connected(lease, now).unwrap();
    assert!(matches!(
        pool.poll_event(),
        Some(PoolEvent::CheckedOut { token: 1, .. })
    ));
    pool.checkin(lease, now).unwrap();
    assert!(matches!(
        pool.poll_event(),
        Some(PoolEvent::CheckedOut { token: 2, .. })
    ));
    pool.checkout(3, now).unwrap();
    pool.handle_timeout(now + Duration::from_millis(10));
    assert!(matches!(
        pool.poll_event(),
        Some(PoolEvent::CheckoutFailed { token: 3, .. })
    ));
    pool.clear();
    assert_eq!(pool.generation(), 1);
    assert!(matches!(
        pool.poll_event(),
        Some(PoolEvent::Cleared { generation: 1 })
    ));
    pool.checkin(lease, now).unwrap();
    assert!(matches!(pool.poll_event(),Some(PoolEvent::Close(l))if l==lease));
    assert_eq!(pool.total(), 0);
    assert!(pool.checkin(lease, now).is_err());
    pool.close();
    assert!(pool.poll_event().is_some());
}
#[test]
fn retry_rules_sessions_cursor_limit_and_error_labels() {
    let mut r = Retry {
        kind: RetryKind::Write,
        enabled: true,
        attempts: 0,
        in_transaction: false,
        wire_version: 27,
        sessions_supported: true,
        standalone: false,
        acknowledged: true,
    };
    let mut e = Error::new(ErrorKind::Server, "stepdown");
    e.code = Some(91);
    assert!(!r.allowed(&e));
    e.labels.push("RetryableWriteError".into());
    assert!(r.retry(&e));
    assert!(!r.retry(&e));
    r.attempts = 0;
    r.in_transaction = true;
    assert!(!r.retry(&e));
    r.in_transaction = false;
    r.kind = RetryKind::Read;
    assert!(r.retry(&e));
    assert!(!Retry::read_command("getMore"));
    let reply = raw(
        &doc! {"ok":0,"code":10107,"codeName":"NotWritablePrimary","errmsg":"stepdown","errorLabels":["RetryableWriteError"]},
    );
    let err = Error::from_response(&reply).unwrap_err();
    assert_eq!(err.code, Some(10107));
    assert!(err.has_label("RetryableWriteError"));
    assert_eq!(err.message, "stepdown");
    let mut session = Session::new([1; 16]);
    session.start_transaction().unwrap();
    assert!(session.start_transaction().is_err());
    let mut w = BsonWriter::new();
    w.clear();
    session.decorate(&mut w, false).unwrap();
    let raw = w.finish().unwrap();
    assert!(raw.get_bool("startTransaction").unwrap());
    assert_eq!(session.state, TransactionState::InProgress);
    assert!(session.commit(&mut w, false).unwrap());
    assert_eq!(w.as_raw().unwrap().get_i64("txnNumber").unwrap(), 1);
    let reply = raw_doc_cursor();
    let batch = CursorBatch::parse(&reply).unwrap();
    let mut cursor = Cursor::new("db.items", "server".into(), Some(1), Some(2)).unwrap();
    assert_eq!(cursor.accept(&batch).unwrap(), 1);
    assert!(cursor.needs_kill());
    let mut cmd = Command::new();
    assert!(!cursor.get_more(&mut cmd).unwrap());
}
fn raw_doc_cursor() -> RawDocumentBuf {
    raw(&doc! {"ok":1,"cursor":{"id":42_i64,"ns":"db.items","firstBatch":[{"a":1},{"a":2}]}})
}

#[test]
fn operation_retries_once_with_same_session_and_excludes_multi() {
    use turnloop_mongodb::operation::*;
    let now = clock::now();
    let mut op = Operation::new();
    let options = OperationOptions {
        token: 9,
        kind: OperationKind::Write,
        retry: true,
        timeout: Some(Duration::from_secs(2)),
        session: Some(RetrySession {
            id: [1; 16],
            txn_number: 7,
        }),
        ..OperationOptions::default()
    };
    let body = raw(&doc! {"insert":"items","$db":"db"});
    let entry = raw(&doc! {"_id":7});
    op.begin(&body, &[("documents", &[&entry])], options, now)
        .unwrap();
    let cap = ServerCapabilities {
        wire_version: 27,
        sessions: true,
        standalone: false,
        direct: false,
    };
    op.selected("a", cap).unwrap();
    op.checked_out().unwrap();
    let first = op.encoded_command().unwrap().to_vec();
    let m = Message::parse(&first, 10000).unwrap();
    assert_eq!(m.body.get_i64("txnNumber").unwrap(), 7);
    assert_eq!(m.sequences().next().unwrap().documents().count(), 1);
    op.sent().unwrap();
    assert!(
        !op.response(&raw(
            &doc! {"ok":0,"code":91,"errmsg":"shutdown","errorLabels":["RetryableWriteError"]}
        ))
        .unwrap()
    );
    assert!(matches!(
        op.action(),
        OperationAction::Select {
            deprioritized: Some("a"),
            ..
        }
    ));
    op.selected("b", cap).unwrap();
    op.checked_out().unwrap();
    assert_eq!(op.encoded_command().unwrap(), first);
    op.sent().unwrap();
    assert!(op.response(&raw(&doc! {"ok":1,"n":1})).unwrap());
    assert!(matches!(
        op.action(),
        OperationAction::Complete { token: 9 }
    ));
    assert!(op.next_timeout().is_none());
    let body = raw(&doc! {"update":"items","$db":"db"});
    let update = raw(&doc! {"q":{},"u":{"$inc":{"x":1}},"multi":true});
    op.begin(
        &body,
        &[("updates", &[&update])],
        OperationOptions {
            kind: OperationKind::Write,
            retry: true,
            session: Some(RetrySession {
                id: [1; 16],
                txn_number: 8,
            }),
            ..OperationOptions::default()
        },
        now,
    )
    .unwrap();
    op.selected("a", cap).unwrap();
    op.checked_out().unwrap();
    let msg = Message::parse(op.encoded_command().unwrap(), 10000).unwrap();
    assert!(msg.body.get("txnNumber").unwrap().is_none());
    op.sent().unwrap();
    op.failed(Error::new(ErrorKind::Network, "reset"));
    assert!(matches!(op.action(), OperationAction::Failed { .. }));
}
#[test]
fn bulk_batch_limits_object_ids_and_unacknowledged_send() {
    use turnloop_mongodb::command::{BulkBatcher, ObjectIdGenerator};
    let body = raw(&doc! {"insert":"items","$db":"db"});
    let a = raw(&doc! {"_id":1});
    let docs = [a.as_ref(); 5];
    let mut batches = BulkBatcher::new(&docs, &body, "documents", 10000, 1000, 2).unwrap();
    let mut sizes = Vec::new();
    while let Some(b) = batches.next_batch() {
        sizes.push((b.offset, b.documents.len()));
    }
    assert_eq!(sizes, [(0, 2), (2, 2), (4, 1)]);
    let mut g = ObjectIdGenerator::new([2; 5], 0xffffff);
    assert_eq!(&g.generate(123).bytes()[9..], &[255; 3]);
    assert_eq!(&g.generate(123).bytes()[9..], &[0; 3]);
    let mut c = ready();
    let body =
        raw(&doc! {"insert":"items","documents":[{"x":1}],"writeConcern":{"w":0},"$db":"db"});
    c.command(20, &body, &[], clock::now()).unwrap();
    assert!(c.poll_event().is_none());
    assert_eq!(
        Message::parse(c.transmit(), 10000).unwrap().flags,
        wire::MORE_TO_COME
    );
    let n = c.transmit().len();
    c.consume_transmit(n - 1).unwrap();
    assert!(c.poll_event().is_none());
    c.consume_transmit(1).unwrap();
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Unacknowledged { token: 20 })
    ));
    assert!(c.is_ready());
}

#[test]
fn sessions_causal_pool_recovery_and_end_retry() {
    use turnloop_mongodb::{
        bson::Timestamp,
        session::{EndAction, EndKind, SessionPool, TransactionEnd},
        time::HostInstant,
    };
    let now = clock::now();
    let mut s = Session::new([33; 16]);
    s.next_retryable_write().unwrap();
    s.observe(&doc!{"operationTime":Timestamp{time:5,increment:2},"$clusterTime":{"clusterTime":Timestamp{time:5,increment:2},"signature":{"hash":0}}});
    let mut w = BsonWriter::new();
    w.clear();
    w.string("find", "items").unwrap();
    s.decorate_causal_read(&mut w, Some("majority")).unwrap();
    let raw = w.finish().unwrap();
    assert_eq!(
        raw.get_document("readConcern")
            .unwrap()
            .get_timestamp("afterClusterTime")
            .unwrap(),
        Timestamp {
            time: 5,
            increment: 2
        }
    );
    assert!(raw.get_document("$clusterTime").is_ok());
    let mut pool = SessionPool::new(Duration::from_secs(120));
    assert!(pool.checkin(s, now));
    assert_eq!(pool.next_timeout(), Some(now + Duration::from_secs(60)));
    let mut s = pool.checkout(now).unwrap();
    assert_eq!(s.txn_number, 1);
    assert!(s.operation_time.is_none());
    s.start_transaction().unwrap();
    assert!(!s.commit(&mut w, false).unwrap());
    assert!(!s.commit(&mut w, false).unwrap());
    s.start_transaction().unwrap();
    w.clear();
    s.decorate(&mut w, false).unwrap();
    w.finish().unwrap();
    s.observe_recovery_token(&raw_recovery()).unwrap();
    s.transaction_write_concern = Some(raw_wc());
    s.commit(&mut w, false).unwrap();
    assert_eq!(
        w.as_raw()
            .unwrap()
            .get_document("recoveryToken")
            .unwrap()
            .get_i32("id")
            .unwrap(),
        4
    );
    let mut end = TransactionEnd::new(EndKind::Commit);
    assert!(matches!(
        end.failed(&mut s, Error::new(ErrorKind::Network, "reset"), 27, &mut w)
            .unwrap(),
        EndAction::Retry
    ));
    assert_eq!(
        w.as_raw()
            .unwrap()
            .get_document("writeConcern")
            .unwrap()
            .get_i32("wtimeout")
            .unwrap(),
        123
    );
    assert!(
        matches!(end.failed(&mut s,Error::new(ErrorKind::Network,"reset"),27,&mut w).unwrap(),EndAction::Failed(e)if e.has_label("UnknownTransactionCommitResult"))
    );
    s.start_transaction().unwrap();
    w.clear();
    s.decorate(&mut w, false).unwrap();
    w.finish().unwrap();
    s.abort(&mut w).unwrap();
    let mut end = TransactionEnd::new(EndKind::Abort);
    assert!(matches!(
        end.failed(&mut s, Error::new(ErrorKind::Network, "reset"), 27, &mut w)
            .unwrap(),
        EndAction::Retry
    ));
    assert_eq!(w.as_raw().unwrap().get_i32("abortTransaction").unwrap(), 1);
    assert!(matches!(
        end.failed(&mut s, Error::new(ErrorKind::Network, "reset"), 27, &mut w)
            .unwrap(),
        EndAction::Complete
    ));
    assert!(pool.checkin(s, now));
    pool.handle_timeout(now + Duration::from_secs(60));
    assert!(pool.is_empty());
    let start = HostInstant::from_duration(Duration::from_millis(42));
    assert_eq!(
        (start + Duration::from_millis(5)).duration_since(start),
        Duration::from_millis(5)
    );
}
fn raw_recovery() -> RawDocumentBuf {
    raw(&doc! {"ok":1,"recoveryToken":{"id":4}})
}
fn raw_wc() -> RawDocumentBuf {
    raw(&doc! {"w":1,"wtimeout":123})
}
#[test]
fn close_preserves_received_reply_and_response_mismatch_completes_once() {
    let mut c = ready();
    let body = raw(&doc! {"ping":1,"$db":"admin"});
    c.command(80, &body, &[], clock::now()).unwrap();
    let req = wire::i32_at(c.transmit(), 4).unwrap();
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    let mut reply = Vec::new();
    wire::encode(&mut reply, 1, req, 0, &raw(&doc! {"ok":1}), &[], 1000).unwrap();
    feed(&mut c, &reply);
    c.close();
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Reply { token: 80 })
    ));
    assert_eq!(c.reply().unwrap().get_i32("ok").unwrap(), 1);
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Closed)));
    c.release_reply().unwrap();
    assert!(!c.is_ready());
    assert!(c.poll_event().is_none());
    let mut c = ready();
    c.command(81, &body, &[], clock::now()).unwrap();
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    wire::encode(&mut reply, 1, 10000, 0, &raw(&doc! {"ok":1}), &[], 1000).unwrap();
    let mut at = 0;
    let mut failed = false;
    while at < reply.len() {
        match c.receive(&reply[at..]) {
            Ok(n) => {
                assert!(n > 0);
                at += n;
            }
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(failed);
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Failed {
            token: Some(81),
            ..
        })
    ));
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Closed)));
    c.close();
    assert!(c.poll_event().is_none());
}

#[test]
fn client_option_inheritance_keeps_explicit_command_fields() {
    let options=Options::parse("mongodb://a/?w=majority&journal=true&wtimeoutMS=30&readPreference=secondary&readPreferenceTags=region:west&maxStalenessSeconds=100&readConcernLevel=majority").unwrap();
    let mut c = Command::new();
    c.insert("db", "items", true, None).unwrap();
    c.apply_client_options(&options, true).unwrap();
    let wc = c.raw().get_document("writeConcern").unwrap();
    assert_eq!(wc.get_str("w").unwrap(), "majority");
    assert_eq!(wc.get_i64("wtimeout").unwrap(), 30);
    assert!(wc.get_bool("j").unwrap());
    c.insert(
        "db",
        "items",
        true,
        Some(&raw(&doc! {"writeConcern":{"w":1}})),
    )
    .unwrap();
    c.apply_client_options(&options, true).unwrap();
    assert_eq!(
        c.raw()
            .get_document("writeConcern")
            .unwrap()
            .get_i32("w")
            .unwrap(),
        1
    );
    c.find("db", "items", &raw(&doc! {}), None).unwrap();
    c.apply_client_options(&options, false).unwrap();
    let p = c.raw().get_document("$readPreference").unwrap();
    assert_eq!(p.get_str("mode").unwrap(), "secondary");
    assert_eq!(
        p.get_array("tags")
            .unwrap()
            .get_document(0)
            .unwrap()
            .get_str("region")
            .unwrap(),
        "west"
    );
    assert_eq!(
        c.raw()
            .get_document("readConcern")
            .unwrap()
            .get_str("level")
            .unwrap(),
        "majority"
    );
}

#[test]
fn incompatible_handshake_and_change_stream_empty_batch_resume_budget() {
    let mut c = Connection::new(Options::parse("mongodb://a/").unwrap());
    c.connected(clock::now(), "").unwrap();
    let req = wire::i32_at(c.transmit(), 4).unwrap();
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    let mut reply = Vec::new();
    wire::encode(
        &mut reply,
        1,
        req,
        0,
        &raw(&doc! {"ok":1,"minWireVersion":99,"maxWireVersion":100}),
        &[],
        1000,
    )
    .unwrap();
    let mut at = 0;
    let mut failed = false;
    while at < reply.len() {
        match c.receive(&reply[at..]) {
            Ok(n) => {
                assert!(n > 0);
                at += n;
            }
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(failed);
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Failed { token: None, .. })
    ));
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Closed)));
    let mut stream = turnloop_mongodb::session::ChangeStream::default();
    let e = Error::new(ErrorKind::Network, "reset");
    assert!(stream.should_resume(&e, 27));
    assert!(!stream.should_resume(&e, 27));
    stream.finish_batch(None).unwrap();
    assert!(stream.should_resume(&e, 27));
}

#[test]
fn discarded_lease_preserves_other_connections_and_generation() {
    let now = clock::now();
    let mut pool = Pool::new(PoolOptions {
        min_size: 2,
        max_size: 2,
        ..Default::default()
    })
    .unwrap();
    pool.ready(now);
    let first = match pool.poll_event().unwrap() {
        PoolEvent::Connect(l) => l,
        e => panic!("{e:?}"),
    };
    let second = match pool.poll_event().unwrap() {
        PoolEvent::Connect(l) => l,
        e => panic!("{e:?}"),
    };
    pool.connected(first, now).unwrap();
    pool.connected(second, now).unwrap();
    pool.checkout(1, now).unwrap();
    let discarded = match pool.poll_event().unwrap() {
        PoolEvent::CheckedOut {
            token: 1,
            connection,
        } => connection,
        e => panic!("{e:?}"),
    };
    pool.checkout(2, now).unwrap();
    let retained = match pool.poll_event().unwrap() {
        PoolEvent::CheckedOut {
            token: 2,
            connection,
        } => connection,
        e => panic!("{e:?}"),
    };
    pool.discard(discarded, now).unwrap();
    assert_eq!(pool.generation(), 0);
    assert_eq!(pool.checked_out(), 1);
    assert!(matches!(pool.poll_event(), Some(PoolEvent::Close(l)) if l == discarded));
    assert!(
        matches!(pool.poll_event(), Some(PoolEvent::Connect(l)) if l != retained && l != discarded)
    );
    assert!(pool.poll_event().is_none(), "no pool-wide clear");
    pool.checkin(retained, now).unwrap();
    pool.checkout(3, now).unwrap();
    assert!(
        matches!(pool.poll_event(), Some(PoolEvent::CheckedOut { token: 3, connection }) if connection == retained)
    );
    assert!(
        pool.discard(discarded, now).is_err(),
        "stale lease rejected"
    );
}
/// One retained deflate state must still emit one independently decodable RFC 1950
/// stream per command (`src/zlib.rs`). Every round is decoded by a fresh upstream
/// decoder that never saw the previous message, exactly as a server does.
#[test]
fn compressed_commands_are_independent_zlib_streams() {
    use std::io::Read;
    let mut compressed =
        Connection::new(Options::parse("mongodb://localhost/?compressors=zlib").unwrap());
    let mut plain = ready();
    compressed.connected(clock::now(), "").unwrap();
    let req = Message::parse(compressed.transmit(), wire::DEFAULT_MAX_MESSAGE)
        .unwrap()
        .request_id;
    let n = compressed.transmit().len();
    compressed.consume_transmit(n).unwrap();
    let mut hello = Vec::new();
    wire::encode(
        &mut hello,
        2,
        req,
        0,
        &raw(&doc! {"ok":1,"maxWireVersion":27,"compression":["zlib"]}),
        &[],
        wire::DEFAULT_MAX_MESSAGE,
    )
    .unwrap();
    feed(&mut compressed, &hello);
    assert!(matches!(
        compressed.poll_event(),
        Some(ConnectionEvent::Ready)
    ));
    let mut decoded_rounds = 0;
    let mut shrank = 0;
    for round in 0..64_u64 {
        // Repetitive filters make real deflate output smaller than the input.
        let body = raw(&doc! {
            "find":"items","$db":"db","filter":{"tag":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            "comment":format!("round {round} aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        });
        compressed.command(round, &body, &[], clock::now()).unwrap();
        plain.command(round, &body, &[], clock::now()).unwrap();
        let framed = compressed.transmit().to_vec();
        let expected = plain.transmit().to_vec();
        assert_eq!(wire::i32_at(&framed, 12).unwrap(), wire::OP_COMPRESSED);
        assert_eq!(wire::i32_at(&framed, 0).unwrap() as usize, framed.len());
        assert_eq!(wire::i32_at(&framed, 16).unwrap(), wire::OP_MSG);
        let size = wire::i32_at(&framed, 20).unwrap() as usize;
        assert_eq!(size, expected.len() - 16);
        assert_eq!(framed[24], 2, "zlib compressor id");
        if framed.len() < expected.len() {
            shrank += 1;
        }
        let mut body_out = Vec::new();
        let read = flate2::read::ZlibDecoder::new(&framed[25..])
            .read_to_end(&mut body_out)
            .unwrap();
        assert_eq!(read, size, "round {round} declared size");
        // The decompressed payload must be the exact OP_MSG an uncompressed
        // connection sends, header excluded.
        assert_eq!(body_out, expected[16..], "round {round} payload");
        decoded_rounds += 1;
        for (c, reply_to) in [
            (&mut compressed, wire::i32_at(&framed, 4).unwrap()),
            (&mut plain, wire::i32_at(&expected, 4).unwrap()),
        ] {
            let n = c.transmit().len();
            c.consume_transmit(n).unwrap();
            let mut reply = Vec::new();
            wire::encode(
                &mut reply,
                900,
                reply_to,
                0,
                &raw(&doc! {"ok":1}),
                &[],
                wire::DEFAULT_MAX_MESSAGE,
            )
            .unwrap();
            feed(c, &reply);
            assert!(matches!(
                c.poll_event(),
                Some(ConnectionEvent::Reply { token }) if token == round
            ));
            assert_eq!(c.reply().unwrap().get_i32("ok").unwrap(), 1);
            c.release_reply().unwrap();
        }
    }
    assert_eq!(decoded_rounds, 64, "every round must be decoded");
    assert_eq!(shrank, 64, "every round must actually compress");
}

/// Writes the pending request and returns a server reply to it.
fn answer(c: &mut Connection, body: &Document) -> Vec<u8> {
    let req = wire::i32_at(c.transmit(), 4).unwrap();
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    let mut reply = Vec::new();
    wire::encode(&mut reply, 1, req, 0, &raw(body), &[], 1000).unwrap();
    reply
}

/// `accepts_receive` must be the check `receive` makes: a refused `receive`
/// changes nothing, and an accepted empty one consumes nothing.
fn accepts(c: &mut Connection) -> bool {
    let accepts = c.accepts_receive();
    assert_eq!(c.receive(&[]).is_ok(), accepts);
    accepts
}

#[test]
fn accepts_receive_tracks_every_connection_state() {
    let body = raw(&doc! {"ping":1,"$db":"admin"});
    // New, then a requested TLS upgrade, then the handshake it releases.
    let mut c = Connection::new(Options::parse("mongodb://a/?tls=true").unwrap());
    assert!(!accepts(&mut c));
    c.connected(clock::now(), "").unwrap();
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::UpgradeTls)));
    assert!(!accepts(&mut c));
    c.tls_established().unwrap();
    assert!(accepts(&mut c));

    // Authentication continues to expect replies after the handshake.
    let mut c = Connection::new(Options::parse("mongodb://user:pencil@a/").unwrap());
    c.connected(clock::now(), "fyko+d2lbbFgONRv9qkxdawL")
        .unwrap();
    assert!(accepts(&mut c));
    let hello = answer(&mut c, &doc! {"ok":1,"maxWireVersion":27});
    feed(&mut c, &hello);
    assert!(c.poll_event().is_none(), "saslStart, not Ready, follows");
    assert!(!c.transmit().is_empty());
    assert!(accepts(&mut c));

    // Ready, an outstanding command, then its unreleased reply.
    let mut c = ready();
    assert!(!accepts(&mut c));
    c.command(1, &body, &[], clock::now()).unwrap();
    assert!(accepts(&mut c));
    let reply = answer(&mut c, &doc! {"ok":1});
    feed(&mut c, &reply);
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Reply { token: 1 })
    ));
    assert!(!accepts(&mut c));
    c.release_reply().unwrap();
    assert!(!accepts(&mut c));
    assert!(c.is_ready());

    // An unacknowledged write expects no reply while or after it drains.
    let unack = raw(&doc! {"insert":"x","writeConcern":{"w":0},"$db":"test"});
    c.command(2, &unack, &[], clock::now()).unwrap();
    assert!(!accepts(&mut c));
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Unacknowledged { token: 2 })
    ));
    assert!(!accepts(&mut c));

    // Closed, including when a command was outstanding.
    c.command(3, &body, &[], clock::now()).unwrap();
    assert!(accepts(&mut c));
    c.close();
    assert!(!accepts(&mut c));
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Failed { token: Some(3), .. })
    ));
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Closed)));
    assert!(c.poll_event().is_none());
}

#[test]
fn write_result_verdict_fails_on_write_errors_despite_ok() {
    use turnloop_mongodb::command::{BulkResult, WriteResult};
    // A duplicate key is `ok: 1` with the failure in writeErrors.
    let duplicate = raw(&doc! {"ok":1,"n":0,"writeErrors":[
        {"index":0,"code":11000,"codeName":"DuplicateKey","errmsg":"E11000 duplicate key"}
    ]});
    let e = WriteResult::parse(&duplicate).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BulkWrite);
    assert_eq!(e.code, Some(11000));
    assert_eq!(e.message, "E11000 duplicate key");
    assert!(
        e.response
            .as_ref()
            .unwrap()
            .get_array("writeErrors")
            .is_ok()
    );
    let decoded = WriteResult::decode(&duplicate).unwrap();
    assert!(!decoded.succeeded());
    assert_eq!(decoded.count, 0);
    assert_eq!(decoded.write_errors.unwrap().into_iter().count(), 1);

    // The write applied, but the requested durability was not confirmed.
    let concern = raw(&doc! {"ok":1,"n":1,"writeConcernError":
        {"code":64,"codeName":"WriteConcernFailed","errmsg":"waiting for replication timed out"}
    });
    let e = WriteResult::parse(&concern).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Server);
    assert_eq!(e.code, Some(64));
    let decoded = WriteResult::decode(&concern).unwrap();
    assert!(!decoded.succeeded());
    assert_eq!(decoded.count, 1);

    // Clean writes, including an explicitly empty writeErrors, succeed.
    for clean in [
        raw(&doc! {"ok":1,"n":2,"nModified":1}),
        raw(&doc! {"ok":1.0,"n":2,"nModified":1,"writeErrors":[]}),
    ] {
        let parsed = WriteResult::parse(&clean).unwrap();
        assert!(parsed.succeeded());
        assert_eq!((parsed.count, parsed.modified_count), (2, 1));
        assert!(WriteResult::decode(&clean).unwrap().succeeded());
    }

    // A failed command is an error either way.
    let failed = raw(&doc! {"ok":0,"code":13,"errmsg":"unauthorized"});
    assert_eq!(WriteResult::parse(&failed).unwrap_err().code, Some(13));
    assert_eq!(WriteResult::decode(&failed).unwrap_err().code, Some(13));

    // Aggregation still sees per-document errors instead of an early return.
    let mut bulk = BulkResult::default();
    assert!(!bulk.accept(&duplicate, 5, true).unwrap());
    assert!(bulk.accept(&concern, 6, false).unwrap());
    assert_eq!(bulk.count, 1);
    assert_eq!(bulk.write_errors[0].get_i32("index").unwrap(), 5);
    assert_eq!(bulk.write_concern_errors[0].get_i32("code").unwrap(), 64);
}

/// Drains the queue and returns the settlement of `token` plus whether the
/// terminal `Closed` came last, asserting no event for it appears twice.
fn settlements(c: &mut Connection, token: u64) -> (Vec<&'static str>, bool) {
    let mut seen = Vec::new();
    let mut closed_last = false;
    while let Some(event) = c.poll_event() {
        closed_last = matches!(event, ConnectionEvent::Closed);
        match event {
            ConnectionEvent::Reply { token: t } if t == token => seen.push("reply"),
            ConnectionEvent::Failed {
                token: Some(t),
                error,
            } if t == token => {
                assert_eq!(error.kind, ErrorKind::Network);
                seen.push("failed");
            }
            ConnectionEvent::Closed => {}
            e => panic!("unexpected event {e:?}"),
        }
    }
    (seen, closed_last)
}

#[test]
fn fail_settles_an_unreleased_reply_exactly_once() {
    let body = raw(&doc! {"ping":1,"$db":"admin"});
    let reset = || Error::new(ErrorKind::Network, "connection reset");

    // The reply arrived but its event was never polled: the queued Reply is
    // withdrawn so the token's only settlement is Failed.
    let mut c = ready();
    c.command(90, &body, &[], clock::now()).unwrap();
    let reply = answer(&mut c, &doc! {"ok":1});
    feed(&mut c, &reply);
    c.fail(reset());
    assert_eq!(settlements(&mut c, 90), (vec!["failed"], true));
    assert!(c.reply().is_err());
    assert!(c.release_reply().is_err());

    // The host took the Reply but had not released it: Failed settles it.
    let mut c = ready();
    c.command(91, &body, &[], clock::now()).unwrap();
    let reply = answer(&mut c, &doc! {"ok":1});
    feed(&mut c, &reply);
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Reply { token: 91 })
    ));
    assert_eq!(c.reply().unwrap().get_i32("ok").unwrap(), 1);
    c.fail(reset());
    assert_eq!(settlements(&mut c, 91), (vec!["failed"], true));
    assert!(c.reply().is_err());
    assert!(c.release_reply().is_err());
    // Nothing further is settled by a second failure or a close.
    c.fail(reset());
    c.close();
    assert!(c.poll_event().is_none());

    // A released reply was settled by the release; failing afterwards reports
    // only the connection failure, for no token.
    let mut c = ready();
    c.command(92, &body, &[], clock::now()).unwrap();
    let reply = answer(&mut c, &doc! {"ok":1});
    feed(&mut c, &reply);
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Reply { token: 92 })
    ));
    c.release_reply().unwrap();
    c.fail(reset());
    assert!(matches!(
        c.poll_event(),
        Some(ConnectionEvent::Failed { token: None, .. })
    ));
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Closed)));
    assert!(c.poll_event().is_none());
}
