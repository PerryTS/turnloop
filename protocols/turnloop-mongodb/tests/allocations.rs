#![deny(unsafe_op_in_unsafe_fn)]
#[path = "support/allocation.rs"]
mod allocation;
#[path = "support/clock.rs"]
mod clock;
use turnloop_mongodb::{
    Connection, ConnectionEvent,
    bson::{doc, raw::RawDocumentBuf},
    command::Command,
    uri::Options,
    wire::{self},
};
#[test]
fn warmed_command_and_borrowed_reply_allocate_zero() {
    for compressed in [false, true] {
        for coordinator in [false, true] {
            measure(compressed, coordinator);
        }
    }
}
fn measure(compressed: bool, coordinator: bool) {
    let now = clock::now();
    let mut c = Connection::new(
        Options::parse(if compressed {
            "mongodb://localhost/?compressors=zlib"
        } else {
            "mongodb://localhost/"
        })
        .unwrap(),
    );
    c.connected(now, "").unwrap();
    let n = c.transmit().len();
    c.consume_transmit(n).unwrap();
    let hello = RawDocumentBuf::try_from(&doc! {"ok":1,"maxWireVersion":27,"compression":["zlib"]})
        .unwrap();
    let mut reply = Vec::new();
    wire::encode(&mut reply, 2, 1, 0, &hello, &[], 10000).unwrap();
    feed(&mut c, &reply);
    assert!(matches!(c.poll_event(), Some(ConnectionEvent::Ready)));
    let filter = RawDocumentBuf::try_from(&doc! {"x":1}).unwrap();
    let result = RawDocumentBuf::try_from(
        &doc! {"ok":1,"cursor":{"id":0_i64,"ns":"db.items","firstBatch":[{"x":1},{"x":1}]}},
    )
    .unwrap();
    wire::encode(&mut reply, 77, 1, 0, &result, &[], 10000).unwrap();
    let mut compressed_reply = Vec::new();
    if compressed {
        use std::io::Write;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&reply[16..]).unwrap();
        let payload = encoder.finish().unwrap();
        compressed_reply.extend_from_slice(&reply[..16]);
        compressed_reply[12..16].copy_from_slice(&wire::OP_COMPRESSED.to_le_bytes());
        compressed_reply.extend_from_slice(&wire::OP_MSG.to_le_bytes());
        compressed_reply.extend_from_slice(&((reply.len() - 16) as i32).to_le_bytes());
        compressed_reply.push(2);
        compressed_reply.extend_from_slice(&payload);
        let n = compressed_reply.len() as i32;
        compressed_reply[..4].copy_from_slice(&n.to_le_bytes());
    }
    let mut cmd = Command::new();
    use turnloop_mongodb::operation::*;
    let mut operation = Operation::new();
    let mut rows = 0;
    let mut commands = 0;
    let mut run = |round| {
        cmd.find("db", "items", &filter, None).unwrap();
        if coordinator {
            operation
                .begin(
                    cmd.raw(),
                    &[],
                    OperationOptions {
                        token: round,
                        kind: OperationKind::Read,
                        retry: true,
                        session: Some(RetrySession {
                            id: [12; 16],
                            txn_number: 0,
                        }),
                        ..OperationOptions::default()
                    },
                    now,
                )
                .unwrap();
            operation
                .selected(
                    "local",
                    ServerCapabilities {
                        wire_version: 27,
                        sessions: true,
                        standalone: false,
                        direct: false,
                    },
                )
                .unwrap();
            operation.checked_out().unwrap();
            operation.send(&mut c, now).unwrap();
        } else {
            c.command(round, cmd.raw(), &[], now).unwrap();
        }
        let req = wire::i32_at(c.transmit(), 4).unwrap();
        // The measured subject must be the compressed encoder in compressed mode.
        assert_eq!(
            wire::i32_at(c.transmit(), 12).unwrap(),
            if compressed {
                wire::OP_COMPRESSED
            } else {
                wire::OP_MSG
            }
        );
        commands += 1;
        let n = c.transmit().len();
        c.consume_transmit(n).unwrap();
        wire::encode(&mut reply, 77, req, 0, &result, &[], 10000).unwrap();
        if compressed {
            compressed_reply[8..12].copy_from_slice(&req.to_le_bytes());
            feed(&mut c, &compressed_reply);
        } else {
            feed(&mut c, &reply);
        }
        assert!(matches!(c.poll_event(),Some(ConnectionEvent::Reply{token})if token==round));
        let body = c.reply().unwrap();
        if coordinator {
            assert!(operation.response(body).unwrap());
        }
        turnloop_mongodb::Error::from_response(body).unwrap();
        let batch = turnloop_mongodb::command::CursorBatch::parse(body).unwrap();
        for row in batch.rows() {
            rows += row.unwrap().get_i32("x").unwrap();
        }
        c.release_reply().unwrap();
    };
    // Both alternating compression buffers and the operation template have seen
    // this fixed-size workload after the original two warm-up commands.
    for round in 0..2 {
        run(round);
    }
    let allocations = allocation::allocations(|| {
        for round in 2..1002 {
            assert!(
                allocation::is_tracking(),
                "command runs on the measuring thread"
            );
            run(round);
        }
    });
    assert_eq!(rows, 2004);
    assert_eq!(
        commands, 1002,
        "two warm-ups plus every measured command ran"
    );
    println!(
        "zlib={compressed} operation={coordinator}: 1000 measured commands, 2000 measured rows, {allocations} allocations"
    );
    assert_eq!(
        allocations, 0,
        "1000 warmed commands/2000 rows allocated {allocations} times (zlib={compressed}, operation={coordinator})"
    );
}
fn feed(c: &mut Connection, b: &[u8]) {
    let mut at = 0;
    while at < b.len() {
        let n = c.receive(&b[at..]).unwrap();
        assert!(n > 0);
        at += n;
    }
}
