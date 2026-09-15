#![deny(unsafe_op_in_unsafe_fn)]
#[path = "support/allocation.rs"]
mod allocation;
use bytes::BytesMut;
use mysql_common::{
    constants::CapabilityFlags as Caps,
    io::ParseBuf,
    packets::HandshakePacket,
    proto::MyDeserialize,
    proto::{MySerialize, codec::PacketCodec},
};
use turnloop_mysql::*;
#[test]
fn warm_ping_and_compressed_ping_allocate_nothing() {
    // A fixed v10 hello, with caching disabled by the empty native password.
    let caps = Caps::CLIENT_PROTOCOL_41
        | Caps::CLIENT_SECURE_CONNECTION
        | Caps::CLIENT_PLUGIN_AUTH
        | Caps::CLIENT_COMPRESS;
    let mut hello = vec![10];
    hello.extend_from_slice(b"9.6.0\0\x07\0\0\0abcdefgh\0");
    hello.extend_from_slice(&(caps.bits() as u16).to_le_bytes());
    hello.extend_from_slice(&[45, 2, 0]);
    hello.extend_from_slice(&((caps.bits() >> 16) as u16).to_le_bytes());
    hello.extend_from_slice(&[21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    hello.extend_from_slice(b"ijklmnopqrst\0mysql_native_password\0");
    let parsed = HandshakePacket::deserialize((), &mut ParseBuf(&hello))
        .expect("fixture operation must succeed");
    let mut checked = Vec::new();
    parsed.serialize(&mut checked);
    assert_eq!(hello, checked);
    for compression in [false, true] {
        let mut c = Connection::new(Config {
            compression,
            ..Config::default()
        })
        .expect("fixture operation must succeed");
        let mut server = PacketCodec::default();
        let mut wire = BytesMut::new();
        server
            .encode(&mut &hello[..], &mut wire)
            .expect("fixture operation must succeed");
        c.receive(&wire).expect("fixture operation must succeed");
        assert!(matches!(
            c.next_event().expect("fixture operation must succeed"),
            Some(Event::Progress)
        ));
        let mut client_bytes = BytesMut::from(c.output());
        let mut auth = Vec::new();
        assert!(
            server
                .decode(&mut client_bytes, &mut auth)
                .expect("fixture operation must succeed")
        );
        c.consume_output(c.output().len())
            .expect("fixture operation must succeed");
        wire.clear();
        server
            .encode(&mut &[0, 0, 0, 2, 0, 0, 0][..], &mut wire)
            .expect("fixture operation must succeed");
        c.receive(&wire).expect("fixture operation must succeed");
        assert!(matches!(
            c.next_event().expect("fixture operation must succeed"),
            Some(Event::Connected { .. })
        ));
        // Precompute response using upstream's independent compression codec.
        server.reset_seq_id();
        if compression {
            server.compress(flate2::Compression::fast());
        }
        c.ping(1).expect("fixture operation must succeed");
        let mut client_bytes = BytesMut::from(c.output());
        auth.clear();
        assert!(
            server
                .decode(&mut client_bytes, &mut auth)
                .expect("fixture operation must succeed")
        );
        assert_eq!(auth, [14]);
        c.consume_output(c.output().len())
            .expect("fixture operation must succeed");
        wire.clear();
        server
            .encode(&mut &[0, 0, 0, 2, 0, 0, 0][..], &mut wire)
            .expect("fixture operation must succeed");
        let response = wire.to_vec();
        c.receive(&response)
            .expect("fixture operation must succeed");
        while c
            .next_event()
            .expect("fixture operation must succeed")
            .is_some()
        {}
        let mut complete = 0;
        let mut run = || {
            c.ping(2).expect("fixture operation must succeed");
            c.consume_output(c.output().len())
                .expect("fixture operation must succeed");
            c.receive(&response)
                .expect("fixture operation must succeed");
            while let Some(e) = c.next_event().expect("fixture operation must succeed") {
                if let Event::Completed { token, outcome } = e {
                    assert_eq!(token, 2);
                    assert_eq!(outcome, Outcome::Success);
                    complete += 1;
                }
            }
        };
        for _ in 0..10 {
            run();
        }
        let count = allocation::allocations(|| {
            for _ in 0..1000 {
                run();
            }
        });
        println!("compression={compression}: 1000 measured pings, {count} allocations");
        assert_eq!(count, 0, "compression={compression}");
        assert_eq!(complete, 1010);
        // Rows use metadata and a compressed 512-byte value from the independent
        // upstream encoder, exercising retained decompression state as well.
        server.reset_seq_id();
        c.query(3, "SELECT REPEAT('x',512)", None)
            .expect("fixture operation must succeed");
        let mut request = BytesMut::from(c.output());
        auth.clear();
        assert!(
            server
                .decode(&mut request, &mut auth)
                .expect("fixture operation must succeed")
        );
        assert_eq!(auth[0], 3);
        c.consume_output(c.output().len())
            .expect("fixture operation must succeed");
        let mut column = Vec::new();
        mysql_common::packets::Column::new(ColumnType::MYSQL_TYPE_VAR_STRING)
            .with_name(b"value")
            .with_character_set(45)
            .serialize(&mut column);
        let mut row = vec![0xfc, 0, 2];
        row.extend_from_slice(&[b'x'; 512]);
        wire.clear();
        for packet in [
            &[1][..],
            &column,
            &[0xfe, 0, 0, 2, 0],
            &row,
            &[0xfe, 0, 0, 2, 0],
        ] {
            server
                .encode(&mut &packet[..], &mut wire)
                .expect("fixture operation must succeed");
        }
        let response = wire.to_vec();
        c.receive(&response)
            .expect("fixture operation must succeed");
        while c
            .next_event()
            .expect("fixture operation must succeed")
            .is_some()
        {}
        let mut rows = 0;
        let mut complete = 0;
        let mut run = || {
            c.query(3, "SELECT REPEAT('x',512)", None)
                .expect("fixture operation must succeed");
            c.consume_output(c.output().len())
                .expect("fixture operation must succeed");
            c.receive(&response)
                .expect("fixture operation must succeed");
            while let Some(e) = c.next_event().expect("fixture operation must succeed") {
                match e {
                    Event::Row { mut row, .. } => {
                        let RawValue::Bytes(bytes) = row
                            .next()
                            .expect("fixture operation must succeed")
                            .expect("fixture operation must succeed")
                        else {
                            panic!()
                        };
                        assert_eq!(bytes, &[b'x'; 512]);
                        rows += 1;
                    }
                    Event::Completed { outcome, .. } => {
                        assert_eq!(outcome, Outcome::Success);
                        complete += 1;
                    }
                    _ => {}
                }
            }
        };
        for _ in 0..10 {
            run();
        }
        let count = allocation::allocations(|| {
            for _ in 0..1000 {
                run();
            }
        });
        println!(
            "compression={compression}: 1000 measured queries, {rows} rows, {count} allocations"
        );
        assert_eq!(count, 0, "row compression={compression}");
        assert_eq!(rows, 1010);
        assert_eq!(complete, 1010);
    }
}
