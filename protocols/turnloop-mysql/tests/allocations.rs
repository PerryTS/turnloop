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
    let parsed = HandshakePacket::deserialize((), &mut ParseBuf(&hello)).unwrap();
    let mut checked = Vec::new();
    parsed.serialize(&mut checked);
    assert_eq!(hello, checked);
    for compression in [false, true] {
        let mut c = Connection::new(Config {
            compression,
            ..Config::default()
        })
        .unwrap();
        let mut server = PacketCodec::default();
        let mut wire = BytesMut::new();
        server.encode(&mut &hello[..], &mut wire).unwrap();
        c.receive(&wire).unwrap();
        assert!(matches!(c.next_event().unwrap(), Some(Event::Progress)));
        let mut client_bytes = BytesMut::from(c.output());
        let mut auth = Vec::new();
        assert!(server.decode(&mut client_bytes, &mut auth).unwrap());
        c.consume_output(c.output().len()).unwrap();
        wire.clear();
        server
            .encode(&mut &[0, 0, 0, 2, 0, 0, 0][..], &mut wire)
            .unwrap();
        c.receive(&wire).unwrap();
        assert!(matches!(
            c.next_event().unwrap(),
            Some(Event::Connected { .. })
        ));
        // Precompute response using upstream's independent compression codec.
        server.reset_seq_id();
        if compression {
            server.compress(flate2::Compression::fast());
        }
        c.ping(1).unwrap();
        let mut client_bytes = BytesMut::from(c.output());
        auth.clear();
        assert!(server.decode(&mut client_bytes, &mut auth).unwrap());
        assert_eq!(auth, [14]);
        c.consume_output(c.output().len()).unwrap();
        wire.clear();
        server
            .encode(&mut &[0, 0, 0, 2, 0, 0, 0][..], &mut wire)
            .unwrap();
        let response = wire.to_vec();
        c.receive(&response).unwrap();
        while c.next_event().unwrap().is_some() {}
        let mut complete = 0;
        let mut run = || {
            c.ping(2).unwrap();
            c.consume_output(c.output().len()).unwrap();
            c.receive(&response).unwrap();
            while let Some(e) = c.next_event().unwrap() {
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
        assert_eq!(count, 0, "compression={compression}");
        assert_eq!(complete, 1010);
    }
}
