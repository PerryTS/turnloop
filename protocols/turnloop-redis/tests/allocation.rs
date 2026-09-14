#![deny(unsafe_op_in_unsafe_fn)]
#[path = "support/alloc.rs"]
mod alloc;
#[global_allocator]
static ALLOC: alloc::Counter = alloc::Counter;
use std::time::Instant;
use turnloop_redis::{
    Config, Connection, Event,
    resp::{Limits, Value, decode},
};
#[test]
fn commands_and_fragmented_decoder_allocate_only_result_storage() {
    let mut c = Connection::new(Config {
        prefer_resp3: false,
        ..Config::default()
    });
    c.connect(Instant::now())
        .expect("fixture operation must succeed");
    c.poll_event();
    c.transport_connected()
        .expect("fixture operation must succeed");
    c.poll_event();
    let operation = |c: &mut Connection| {
        c.command(1, &[b"INCR", b"counter"], None)
            .expect("fixture operation must succeed");
        c.consume_output(c.output().len());
        c.receive(b":42\r\n")
            .expect("fixture operation must succeed");
        assert_eq!(
            c.poll_event(),
            Some(Event::Reply {
                token: 1,
                result: Ok(Value::Integer(42))
            })
        );
    };
    operation(&mut c);
    assert_eq!(
        alloc::measure(|| {
            for _ in 0..1000 {
                operation(&mut c);
            }
        }),
        0
    );
    let frame = b"*3\r\n$4\r\nhello";
    assert_eq!(
        alloc::measure(|| {
            for end in 0..frame.len() {
                assert!(
                    decode(&frame[..end], Limits::default())
                        .expect("fixture operation must succeed")
                        .is_none()
                );
            }
        }),
        0
    );
    assert_eq!(
        alloc::measure(|| {
            let value = decode(b"*2\r\n$3\r\none\r\n$3\r\ntwo\r\n", Limits::default())
                .expect("fixture operation must succeed")
                .expect("fixture operation must succeed")
                .0;
            assert!(matches!(value, Value::Array(v) if v.len() == 2));
        }),
        3,
        "one result array + two owned bulk values"
    );
}

#[test]
fn pubsub_allocates_only_returned_binary_fields() {
    let mut c = Connection::new(Config {
        prefer_resp3: false,
        ..Config::default()
    });
    c.connect(Instant::now())
        .expect("fixture operation must succeed");
    c.poll_event();
    c.transport_connected()
        .expect("fixture operation must succeed");
    c.poll_event();
    c.command(1, &[b"SUBSCRIBE", b"news"], None)
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b"*3\r\n$9\r\nsubscribe\r\n$4\r\nnews\r\n:1\r\n")
        .expect("fixture operation must succeed");
    c.poll_event();
    for _ in 0..10 {
        assert_eq!(
            alloc::measure(|| {
                for byte in b"*3\r\n$7\r\nmessage\r\n$4\r\nnews\r\n$3\r\na\0b\r\n" {
                    c.receive(&[*byte]).expect("fixture operation must succeed");
                }
                assert!(
                    matches!(c.poll_event(), Some(Event::Message { pattern: None, channel, payload }) if channel == b"news" && payload == b"a\0b")
                );
            }),
            2,
            "channel and payload only; no temporary array/tag allocations"
        );
    }
}
