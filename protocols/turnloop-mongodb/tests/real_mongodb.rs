#![cfg(not(target_arch = "wasm32"))]
#![deny(unsafe_op_in_unsafe_fn)]
mod common;
use common::{ports, raw, Driver};
use std::time::{Duration, Instant};
use turnloop_mongodb::{
    bson::{doc, raw::RawDocument, Document},
    command::{BulkResult, Command, CursorBatch, WriteModel},
    session::{ChangeStream, Session},
    topology::{ServerType, Topology, TopologyType},
    uri::{Options, ReadPreference},
    wire::BsonWriter,
    Error,
};
fn user(d: &mut Driver) {
    let write_concern = if d
        .core
        .hello
        .as_ref()
        .is_some_and(|h| h.contains_key("setName"))
    {
        3
    } else {
        1
    };
    d.run("admin",doc!{"createUser":"lane","pwd":"pencil","roles":[{"role":"root","db":"admin"}],"mechanisms":["SCRAM-SHA-1","SCRAM-SHA-256"],"writeConcern":{"w":write_concern,"wtimeout":20000}}).expect("fixture operation must succeed");
}
fn uri(port: u16, extra: &str) -> String {
    format!("mongodb://lane:pencil@127.0.0.1:{port}/admin?directConnection=true&{extra}")
}
fn command(d: &mut Driver, c: &Command, seq: &[(&str, &[&RawDocument])]) -> Document {
    let out = d.raw_command(c.raw(), seq).expect("fixture operation must succeed");
    Error::from_response(&raw(&out)).expect("fixture operation must succeed");
    out
}
fn rows(reply: &Document) -> Vec<Document> {
    let r = raw(reply);
    CursorBatch::parse(&r)
        .expect("fixture operation must succeed")
        .rows()
        .map(|d| Document::try_from(d.expect("fixture operation must succeed")).expect("fixture operation must succeed"))
        .collect()
}
fn crud(d: &mut Driver) {
    let db = "lane_crud";
    let coll = "items";
    d.run(db, doc! {"dropDatabase":1}).expect("fixture operation must succeed");
    let mut c = Command::new();
    let docs = [
        raw(&doc! {"_id":1,"x":1,"payload":vec!["abcdefgh";100]}),
        raw(&doc! {"_id":2,"x":2}),
        raw(&doc! {"_id":3,"x":3}),
    ];
    let refs: Vec<_> = docs.iter().map(|d| d.as_ref()).collect();
    c.insert(db, coll, true, None).expect("fixture operation must succeed");
    let result = command(d, &c, &[("documents", &refs)]);
    assert_eq!(result.get_i32("n").expect("fixture operation must succeed"), 3);
    // Ordered batch stops at duplicate; unordered batch continues and reports original index.
    let duplicate = [raw(&doc! {"_id":1}), raw(&doc! {"_id":4,"x":4})];
    let refs = [duplicate[0].as_ref(), duplicate[1].as_ref()];
    let result = d.raw_command(c.raw(), &[("documents", &refs)]).expect("fixture operation must succeed");
    let mut bulk = BulkResult::default();
    assert!(!bulk.accept(&raw(&result), 7, true).expect("fixture operation must succeed"));
    assert_eq!(bulk.write_errors[0].get_i32("index").expect("fixture operation must succeed"), 7);
    assert_eq!(bulk.count, 0);
    c.insert(db, coll, false, None).expect("fixture operation must succeed");
    let result = d.raw_command(c.raw(), &[("documents", &refs)]).expect("fixture operation must succeed");
    let e = Error::from_response(&raw(&result)).unwrap_err();
    assert_eq!(e.code, Some(11000));
    assert_eq!(e.name(), "MongoBulkWriteError");
    let mut bulk = BulkResult::default();
    assert!(bulk.accept(&raw(&result), 0, false).expect("fixture operation must succeed"));
    assert_eq!(bulk.count, 1);
    let empty = raw(&doc! {});
    c.find(
        db,
        coll,
        &empty,
        Some(&raw(&doc! {"batchSize":1,"sort":{"_id":1}})),
    )
    .expect("fixture operation must succeed");
    let first = command(d, &c, &[]);
    let r = raw(&first);
    let batch = CursorBatch::parse(&r).expect("fixture operation must succeed");
    assert_eq!(batch.rows().count(), 1);
    assert_ne!(batch.id, 0);
    let mut count = 1;
    let mut id = batch.id;
    while id != 0 {
        c.get_more(db, coll, id, Some(1), None).expect("fixture operation must succeed");
        let reply = command(d, &c, &[]);
        let r = raw(&reply);
        let batch = CursorBatch::parse(&r).expect("fixture operation must succeed");
        count += batch.rows().count();
        id = batch.id;
    }
    assert_eq!(count, 4);
    c.find_one(db, coll, &raw(&doc! {"_id":2})).expect("fixture operation must succeed");
    assert_eq!(rows(&command(d, &c, &[]))[0].get_i32("x").expect("fixture operation must succeed"), 2);
    c.find(db, coll, &empty, Some(&raw(&doc! {"batchSize":1})))
        .expect("fixture operation must succeed");
    let reply = command(d, &c, &[]);
    let r = raw(&reply);
    let id = CursorBatch::parse(&r).expect("fixture operation must succeed").id;
    assert_ne!(id, 0);
    c.kill_cursor(db, coll, id).expect("fixture operation must succeed");
    let reply = command(d, &c, &[]);
    assert_eq!(
        reply.get_array("cursorsKilled").expect("fixture operation must succeed")[0].as_i64(),
        Some(id)
    );
    let mut model = WriteModel::default();
    let update = model
        .update(
            &raw(&doc! {"_id":2}),
            &raw(&doc! {"$set":{"x":20}}),
            false,
            false,
            None,
        )
        .expect("fixture operation must succeed");
    c.update(db, coll, true, None).expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[("updates", &[update])])
            .get_i32("nModified")
            .expect("fixture operation must succeed"),
        1
    );
    let update = model
        .update(&empty, &raw(&doc! {"$inc":{"x":1}}), true, false, None)
        .expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[("updates", &[update])])
            .get_i32("nModified")
            .expect("fixture operation must succeed"),
        4
    );
    c.count_documents(db, coll, &empty, 0, 0).expect("fixture operation must succeed");
    assert_eq!(rows(&command(d, &c, &[]))[0].get_i32("n").expect("fixture operation must succeed"), 4);
    c.estimated_document_count(db, coll, None).expect("fixture operation must succeed");
    assert_eq!(command(d, &c, &[]).get_i32("n").expect("fixture operation must succeed"), 4);
    c.distinct(db, coll, "x", &empty, None).expect("fixture operation must succeed");
    assert_eq!(command(d, &c, &[]).get_array("values").expect("fixture operation must succeed").len(), 4);
    c.aggregate(db, coll, &[&raw(&doc! {"$match":{"x":{"$gt":10}}})], None)
        .expect("fixture operation must succeed");
    assert_eq!(rows(&command(d, &c, &[])).len(), 1);
    c.find_one_and_modify(
        db,
        coll,
        &raw(&doc! {"_id":2}),
        Some(&raw(&doc! {"$set":{"x":99}})),
        Some(&raw(&doc! {"new":true})),
    )
    .expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[])
            .get_document("value")
            .expect("fixture operation must succeed")
            .get_i32("x")
            .expect("fixture operation must succeed"),
        99
    );
    c.find_one_and_modify(
        db,
        coll,
        &raw(&doc! {"_id":2}),
        Some(&raw(&doc! {"x":100})),
        Some(&raw(&doc! {"new":true})),
    )
    .expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[])
            .get_document("value")
            .expect("fixture operation must succeed")
            .get_i32("x")
            .expect("fixture operation must succeed"),
        100
    );
    c.create_indexes(db, coll, &[&raw(&doc! {"key":{"x":1},"name":"x_1"})], None)
        .expect("fixture operation must succeed");
    assert_eq!(command(d, &c, &[]).get_i32("numIndexesAfter").expect("fixture operation must succeed"), 2);
    c.list_indexes(db, coll).expect("fixture operation must succeed");
    assert_eq!(rows(&command(d, &c, &[])).len(), 2);
    c.drop_index(db, coll, "x_1").expect("fixture operation must succeed");
    assert_eq!(command(d, &c, &[]).get_i32("nIndexesWas").expect("fixture operation must succeed"), 2);
    c.list_databases(true).expect("fixture operation must succeed");
    assert!(command(d, &c, &[])
        .get_array("databases")
        .expect("fixture operation must succeed")
        .iter()
        .any(|v| v.as_document().expect("fixture operation must succeed").get_str("name").ok() == Some(db)));
    c.list_collections(db, true, &empty).expect("fixture operation must succeed");
    assert_eq!(rows(&command(d, &c, &[]))[0].get_str("name").expect("fixture operation must succeed"), coll);
    c.find_one_and_modify(db, coll, &raw(&doc! {"_id":2}), None, None)
        .expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[])
            .get_document("value")
            .expect("fixture operation must succeed")
            .get_i32("_id")
            .expect("fixture operation must succeed"),
        2
    );
    let delete = model.delete(&raw(&doc! {"_id":1}), false, None).expect("fixture operation must succeed");
    c.delete(db, coll, true, None).expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[("deletes", &[delete])])
            .get_i32("n")
            .expect("fixture operation must succeed"),
        1
    );
    let delete = model.delete(&empty, true, None).expect("fixture operation must succeed");
    assert_eq!(
        command(d, &c, &[("deletes", &[delete])])
            .get_i32("n")
            .expect("fixture operation must succeed"),
        2
    );
    c.find(db, coll, &empty, None).expect("fixture operation must succeed");
    assert!(rows(&command(d, &c, &[])).is_empty());
}
#[test]
#[ignore = "private MongoDB required: scripts/test-servers.py run"]
fn real_standalone_replica_scram_tls() {
    let ports = ports();
    let port = |name: &str| ports.iter().find(|(n, _)| n == name).expect("fixture operation must succeed").1;
    let mut unauth = Driver::connect(&format!(
        "mongodb://127.0.0.1:{}/?directConnection=true",
        port("standalone")
    ))
    .expect("fixture operation must succeed");
    user(&mut unauth);
    for mechanism in ["SCRAM-SHA-1", "SCRAM-SHA-256"] {
        let mut d = Driver::connect(&uri(
            port("standalone"),
            &format!("authMechanism={mechanism}"),
        ))
        .expect("fixture operation must succeed");
        assert_eq!(
            d.run("admin", doc! {"connectionStatus":1})
                .expect("fixture operation must succeed")
                .get_document("authInfo")
                .expect("fixture operation must succeed")
                .get_array("authenticatedUsers")
                .expect("fixture operation must succeed")
                .len(),
            1
        );
    }
    assert!(
        Driver::connect(&uri(port("standalone"), "").replace("pencil", "wrong-password")).is_err()
    );
    let mut d = Driver::connect(&uri(port("standalone"), "compressors=zlib")).expect("fixture operation must succeed");
    crud(&mut d);
    let mut tls = Driver::connect(&format!(
        "mongodb://127.0.0.1:{}/?tls=true&directConnection=true",
        port("tls")
    ))
    .expect("fixture operation must succeed");
    user(&mut tls);
    let mut tls = Driver::connect(&uri(port("tls"), "tls=true")).expect("fixture operation must succeed");
    assert_eq!(
        tls.run("admin", doc! {"connectionStatus":1})
            .expect("fixture operation must succeed")
            .get_document("authInfo")
            .expect("fixture operation must succeed")
            .get_array("authenticatedUsers")
            .expect("fixture operation must succeed")
            .len(),
        1
    );
    crud(&mut tls);
    let members: Vec<_> = (0..3)
        .map(|i| doc! {"_id":i,"host":format!("127.0.0.1:{}",port(&format!("rs{i}")))})
        .collect();
    let mut init = Driver::connect(&format!(
        "mongodb://127.0.0.1:{}/?directConnection=true",
        port("rs0")
    ))
    .expect("fixture operation must succeed");
    init.run(
        "admin",
        doc! {"replSetInitiate":{"_id":"turnloop_test","members":members}},
    )
    .expect("fixture operation must succeed");
    let deadline = Instant::now() + Duration::from_secs(45);
    let primary = loop {
        let mut p = None;
        for i in 0..3 {
            let n = port(&format!("rs{i}"));
            if let Ok(mut d) =
                Driver::connect(&format!("mongodb://127.0.0.1:{n}/?directConnection=true"))
                && let Ok(h) = d.run("admin", doc! {"hello":1})
                    && h.get_bool("isWritablePrimary").unwrap_or(false) {
                        p = Some((n, d));
                        break;
                    }
        }
        if let Some(p) = p {
            break p;
        }
        assert!(
            Instant::now() < deadline,
            "Replica set did not elect a primary"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let (primary_port, mut bootstrap) = primary;
    user(&mut bootstrap);
    let mut d = Driver::connect(&uri(primary_port, "compressors=zlib")).expect("fixture operation must succeed");
    let opts = Options::parse(&format!(
        "mongodb://127.0.0.1:{primary_port}/?replicaSet=turnloop_test"
    ))
    .expect("fixture operation must succeed");
    let mut topology = Topology::new(&opts, Instant::now());
    let h = d.run("admin", doc! {"hello":1}).expect("fixture operation must succeed");
    topology.update(
        &format!("127.0.0.1:{primary_port}"),
        &h,
        Instant::now(),
        Duration::from_millis(2),
    );
    assert_eq!(topology.kind, TopologyType::ReplicaSetWithPrimary);
    assert_eq!(topology.servers.len(), 3);
    for i in 0..3 {
        let n = port(&format!("rs{i}"));
        let mut peer = Driver::connect(&uri(n, "")).expect("fixture operation must succeed");
        let h = peer.run("admin", doc! {"hello":1}).expect("fixture operation must succeed");
        topology.update(
            &format!("127.0.0.1:{n}"),
            &h,
            Instant::now(),
            Duration::from_millis(3),
        );
    }
    assert_eq!(
        topology
            .servers
            .values()
            .filter(|s| s.kind == ServerType::RSSecondary)
            .count(),
        2
    );
    let mut candidates = Vec::new();
    topology
        .candidates(ReadPreference::Secondary, &[], None, &mut candidates)
        .expect("fixture operation must succeed");
    assert_eq!(candidates.len(), 2);
    crud(&mut d);
    transactions_and_changes(&mut d);
    retry_coordinator(&mut d);
    failover(&mut d, primary_port, &ports, &mut topology);
    eprintln!("Verified standalone, 3-member replica set, SCRAM SHA-1/SHA-256/default, TLS, zlib, CRUD, cursors, transactions, retry identity and change stream events");
}
fn transactions_and_changes(d: &mut Driver) {
    let db = "lane_tx";
    d.run(db, doc! {"dropDatabase":1}).expect("fixture operation must succeed");
    d.run(db, doc! {"create":"items"}).expect("fixture operation must succeed");
    let mut session = Session::new([42; 16]);
    session.start_transaction().expect("fixture operation must succeed");
    let mut w = BsonWriter::new();
    w.clear();
    w.string("insert", "items").expect("fixture operation must succeed");
    session.decorate(&mut w, false).expect("fixture operation must succeed");
    w.string("$db", db).expect("fixture operation must succeed");
    w.finish().expect("fixture operation must succeed");
    let entry = raw(&doc! {"_id":1,"x":1});
    let out = d
        .raw_command(w.as_raw().expect("fixture operation must succeed"), &[("documents", &[&entry])])
        .expect("fixture operation must succeed");
    Error::from_response(&raw(&out)).expect("fixture operation must succeed");
    assert!(session.commit(&mut w, false).expect("fixture operation must succeed"));
    let out = d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed");
    Error::from_response(&raw(&out)).expect("fixture operation must succeed");
    assert_eq!(
        d.run(db, doc! {"count":"items"})
            .expect("fixture operation must succeed")
            .get_i32("n")
            .expect("fixture operation must succeed"),
        1
    );
    session.start_transaction().expect("fixture operation must succeed");
    w.clear();
    w.string("insert", "items").expect("fixture operation must succeed");
    session.decorate(&mut w, false).expect("fixture operation must succeed");
    w.string("$db", db).expect("fixture operation must succeed");
    w.finish().expect("fixture operation must succeed");
    let entry = raw(&doc! {"_id":2});
    let out = d
        .raw_command(w.as_raw().expect("fixture operation must succeed"), &[("documents", &[&entry])])
        .expect("fixture operation must succeed");
    Error::from_response(&raw(&out)).expect("fixture operation must succeed");
    assert!(session.abort(&mut w).expect("fixture operation must succeed"));
    Error::from_response(&raw(&d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed"))).expect("fixture operation must succeed");
    assert_eq!(
        d.run(db, doc! {"count":"items"})
            .expect("fixture operation must succeed")
            .get_i32("n")
            .expect("fixture operation must succeed"),
        1
    );
    // Resend a retryable update with identical lsid/txnNumber; side effect once.
    session.next_retryable_write().expect("fixture operation must succeed");
    w.clear();
    w.string("update", "items").expect("fixture operation must succeed");
    session.decorate(&mut w, true).expect("fixture operation must succeed");
    w.string("$db", db).expect("fixture operation must succeed");
    w.finish().expect("fixture operation must succeed");
    let model = raw(&doc! {"q":{"_id":1},"u":{"$inc":{"x":1}},"multi":false});
    for _ in 0..2 {
        Error::from_response(&raw(&d
            .raw_command(w.as_raw().expect("fixture operation must succeed"), &[("updates", &[&model])])
            .expect("fixture operation must succeed")))
        .expect("fixture operation must succeed");
    }
    let reply = d.run(db, doc! {"find":"items","filter":{"_id":1}}).expect("fixture operation must succeed");
    assert_eq!(rows(&reply)[0].get_i32("x").expect("fixture operation must succeed"), 2);
    let reply = d
        .run(
            db,
            doc! {"aggregate":"items","pipeline":[{"$changeStream":{}}],"cursor":{"batchSize":1}},
        )
        .expect("fixture operation must succeed");
    let r = raw(&reply);
    let id = CursorBatch::parse(&r).expect("fixture operation must succeed").id;
    assert_ne!(id, 0);
    d.run(db, doc! {"insert":"items","documents":[{"_id":3}]})
        .expect("fixture operation must succeed");
    let mut c = Command::new();
    c.get_more(db, "items", id, Some(1), Some(5000)).expect("fixture operation must succeed");
    let reply = command(d, &c, &[]);
    let r = raw(&reply);
    let batch = CursorBatch::parse(&r).expect("fixture operation must succeed");
    let event = batch.rows().next().expect("change event").expect("fixture operation must succeed");
    assert_eq!(event.get_str("operationType").expect("fixture operation must succeed"), "insert");
    let mut change = ChangeStream::default();
    change.observe_document(event).expect("fixture operation must succeed");
    assert!(change.resume_token.is_some());
    change.finish_batch(batch.post_batch_resume_token).expect("fixture operation must succeed");
    c.kill_cursor(db, "items", id).expect("fixture operation must succeed");
    command(d, &c, &[]);
    let resumed = d
        .run(
            db,
            doc! {"aggregate":"items","pipeline":[change.stage()],"cursor":{"batchSize":1}},
        )
        .expect("fixture operation must succeed");
    let r = raw(&resumed);
    let resumed_id = CursorBatch::parse(&r).expect("fixture operation must succeed").id;
    assert_ne!(resumed_id, 0);
    d.run(db, doc! {"insert":"items","documents":[{"_id":4}]})
        .expect("fixture operation must succeed");
    c.get_more(db, "items", resumed_id, Some(1), Some(5000))
        .expect("fixture operation must succeed");
    let reply = command(d, &c, &[]);
    let r = raw(&reply);
    let batch = CursorBatch::parse(&r).expect("fixture operation must succeed");
    let event = batch.rows().next().expect("resumed event").expect("fixture operation must succeed");
    assert_eq!(
        event
            .get_document("documentKey")
            .expect("fixture operation must succeed")
            .get_i32("_id")
            .expect("fixture operation must succeed"),
        4
    );
    c.kill_cursor(db, "items", resumed_id).expect("fixture operation must succeed");
    command(d, &c, &[]);
}

fn retry_coordinator(d: &mut Driver) {
    use turnloop_mongodb::{operation::*, ConnectionEvent};
    let db = "lane_retry";
    d.run(db, doc! {"dropDatabase":1}).expect("fixture operation must succeed");
    d.run(db, doc! {"create":"items"}).expect("fixture operation must succeed");
    for kind in [OperationKind::Write, OperationKind::Read] {
        let name = if kind == OperationKind::Write {
            "insert"
        } else {
            "find"
        };
        d.run("admin",doc!{"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":[name],"errorCode":91,"errorLabels":["RetryableWriteError"]}}).expect("fixture operation must succeed");
        let mut op = Operation::new();
        let body = raw(&if kind == OperationKind::Write {
            doc! {"insert":"items","$db":db}
        } else {
            doc! {"find":"items","filter":{},"$db":db}
        });
        let entry = raw(&doc! {"_id":1,"x":1});
        let refs = [entry.as_ref()];
        let seq = [("documents", refs.as_slice())];
        op.begin(
            &body,
            if kind == OperationKind::Write {
                &seq
            } else {
                &[]
            },
            OperationOptions {
                token: 505,
                kind,
                retry: true,
                timeout: Some(Duration::from_secs(10)),
                session: Some(RetrySession {
                    id: [11; 16],
                    txn_number: 1,
                }),
                ..OperationOptions::default()
            },
            Instant::now(),
        )
        .expect("fixture operation must succeed");
        let mut sends = 0;
        let mut success = None;
        loop {
            match op.action() {
                OperationAction::Select { .. } => op
                    .selected(
                        "private-primary",
                        ServerCapabilities {
                            wire_version: 27,
                            sessions: true,
                            standalone: false,
                            direct: false,
                        },
                    )
                    .expect("fixture operation must succeed"),
                OperationAction::Checkout { .. } => op.checked_out().expect("fixture operation must succeed"),
                OperationAction::Send { .. } => {
                    op.send(&mut d.core, Instant::now()).expect("fixture operation must succeed");
                    sends += 1;
                }
                OperationAction::Waiting => {
                    if let Some(event) = d.core.poll_event() {
                        match event {
                            ConnectionEvent::Reply { token } => {
                                assert_eq!(token, 505);
                                if op.response(d.core.reply().expect("fixture operation must succeed")).expect("fixture operation must succeed") {
                                    success =
                                        Some(Document::try_from(d.core.reply().expect("fixture operation must succeed")).expect("fixture operation must succeed"));
                                }
                                d.core.release_reply().expect("fixture operation must succeed");
                            }
                            ConnectionEvent::Failed { error, .. } => op.failed(error),
                            e => panic!("Unexpected event {e:?}"),
                        }
                    } else {
                        d.turn().expect("fixture operation must succeed");
                    }
                }
                OperationAction::Complete { token } => {
                    assert_eq!(token, 505);
                    break;
                }
                a => panic!("Unexpected operation action {a:?}"),
            }
        }
        assert_eq!(sends, 2, "failpoint must force exactly one retry");
        let reply = success.expect("fixture operation must succeed");
        if kind == OperationKind::Write {
            assert_eq!(reply.get_i32("n").expect("fixture operation must succeed"), 1);
        } else {
            assert_eq!(rows(&reply).len(), 1);
        }
    }
    assert_eq!(
        d.run(db, doc! {"count":"items"})
            .expect("fixture operation must succeed")
            .get_i32("n")
            .expect("fixture operation must succeed"),
        1
    );
    // A transient commit error is retried with majority and identical transaction identity.
    let mut s = Session::new([19; 16]);
    s.start_transaction().expect("fixture operation must succeed");
    let mut w = BsonWriter::new();
    w.clear();
    w.string("insert", "items").expect("fixture operation must succeed");
    s.decorate(&mut w, false).expect("fixture operation must succeed");
    w.string("$db", db).expect("fixture operation must succeed");
    w.finish().expect("fixture operation must succeed");
    let item = raw(&doc! {"_id":2});
    Error::from_response(&raw(&d
        .raw_command(w.as_raw().expect("fixture operation must succeed"), &[("documents", &[&item])])
        .expect("fixture operation must succeed")))
    .expect("fixture operation must succeed");
    d.run("admin",doc!{"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["commitTransaction"],"errorCode":91,"errorLabels":["RetryableWriteError"]}}).expect("fixture operation must succeed");
    s.commit(&mut w, false).expect("fixture operation must succeed");
    let first = d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed");
    let err = Error::from_response(&raw(&first)).unwrap_err();
    assert!(Session::retry_commit(&err));
    s.commit(&mut w, true).expect("fixture operation must succeed");
    Error::from_response(&raw(&d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed"))).expect("fixture operation must succeed");
    assert_eq!(
        d.run(db, doc! {"count":"items"})
            .expect("fixture operation must succeed")
            .get_i32("n")
            .expect("fixture operation must succeed"),
        2
    );
    // Abort retries exactly once and leaves no inserted document visible.
    use turnloop_mongodb::session::{EndAction, EndKind, TransactionEnd};
    s.start_transaction().expect("fixture operation must succeed");
    w.clear();
    w.string("insert", "items").expect("fixture operation must succeed");
    s.decorate(&mut w, false).expect("fixture operation must succeed");
    w.string("$db", db).expect("fixture operation must succeed");
    w.finish().expect("fixture operation must succeed");
    let item = raw(&doc! {"_id":3});
    Error::from_response(&raw(&d
        .raw_command(w.as_raw().expect("fixture operation must succeed"), &[("documents", &[&item])])
        .expect("fixture operation must succeed")))
    .expect("fixture operation must succeed");
    d.run("admin",doc!{"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["abortTransaction"],"errorCode":91,"errorLabels":["RetryableWriteError"]}}).expect("fixture operation must succeed");
    s.abort(&mut w).expect("fixture operation must succeed");
    let first = d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed");
    let error = Error::from_response(&raw(&first)).unwrap_err();
    let mut end = TransactionEnd::new(EndKind::Abort);
    assert!(matches!(
        end.failed(&mut s, error, 27, &mut w).expect("fixture operation must succeed"),
        EndAction::Retry
    ));
    Error::from_response(&raw(&d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed"))).expect("fixture operation must succeed");
    let result = d.run(db, doc! {"count":"items"}).expect("fixture operation must succeed");
    assert_eq!(result.get_i32("n").expect("fixture operation must succeed"), 2);
    s.observe(&result);
    w.clear();
    w.string("find", "items").expect("fixture operation must succeed");
    s.decorate_causal_read(&mut w, Some("majority")).expect("fixture operation must succeed");
    w.string("$db", db).expect("fixture operation must succeed");
    w.finish().expect("fixture operation must succeed");
    let found = d.raw_command(w.as_raw().expect("fixture operation must succeed"), &[]).expect("fixture operation must succeed");
    Error::from_response(&raw(&found)).expect("fixture operation must succeed");
    assert_eq!(rows(&found).len(), 2);
}

fn failover(d: &mut Driver, old_primary: u16, ports: &[(String, u16)], topology: &mut Topology) {
    let response = d.run("admin", doc! {"replSetStepDown":60,"force":true});
    assert!(
        response.is_ok() || response.is_err_and(|e| e.kind == turnloop_mongodb::ErrorKind::Network)
    );
    let deadline = Instant::now() + Duration::from_secs(40);
    let new_primary = loop {
        let mut primary = None;
        for (name, port) in ports {
            if !name.starts_with("rs") || *port == old_primary {
                continue;
            }
            if let Ok(mut peer) = Driver::connect(&uri(*port, ""))
                && let Ok(hello) = peer.run("admin", doc! {"hello":1}) {
                    topology.update(
                        &format!("127.0.0.1:{port}"),
                        &hello,
                        Instant::now(),
                        Duration::from_millis(3),
                    );
                    if hello.get_bool("isWritablePrimary").unwrap_or(false) {
                        primary = Some((*port, peer));
                        break;
                    }
                }
        }
        if let Some(p) = primary {
            break p;
        }
        assert!(Instant::now() < deadline, "No new primary after stepdown");
        std::thread::sleep(Duration::from_millis(100));
    };
    let (port, mut peer) = new_primary;
    assert_ne!(port, old_primary);
    assert_eq!(topology.kind, TopologyType::ReplicaSetWithPrimary);
    let mut candidates = Vec::new();
    topology
        .candidates(ReadPreference::Primary, &[], None, &mut candidates)
        .expect("fixture operation must succeed");
    assert_eq!(candidates, [format!("127.0.0.1:{port}")]);
    assert_eq!(
        peer.run("lane_retry", doc! {"count":"items"})
            .expect("fixture operation must succeed")
            .get_i32("n")
            .expect("fixture operation must succeed"),
        2
    );
}
