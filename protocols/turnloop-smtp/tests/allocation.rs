#![deny(unsafe_op_in_unsafe_fn)]
#[path = "support/alloc.rs"]
mod alloc;
#[global_allocator]
static ALLOC: alloc::Counter = alloc::Counter;
use std::time::Instant;
use turnloop_smtp::{Config, Connection, Envelope, Event, Tls};
#[test]
fn mail_transport_allocates_only_returned_info() {
    let now = Instant::now();
    let mut c = Connection::new(Config {
        tls: Tls::None,
        ..Config::default()
    })
    .expect("fixture operation must succeed");
    c.connected(now).expect("fixture operation must succeed");
    c.receive(b"220 smtp\r\n", now).expect("fixture operation must succeed");
    c.consume_output(c.output().len());
    c.receive(b"250-smtp\r\n250 PIPELINING\r\n", now).expect("fixture operation must succeed");
    c.poll_event();
    for _ in 0..10 {
        let envelope = Envelope {
            from: "a@example.test".into(),
            to: vec!["b@example.test".into()],
        };
        let id = "<id@example.test>".into();
        let count = alloc::measure(|| {
            c.send(1, envelope, id, b"Subject: test\r\n\r\nhello\r\n", now)
                .expect("fixture operation must succeed");
            c.consume_output(c.output().len());
            c.receive(b"250 mail\r\n250 rcpt\r\n", now).expect("fixture operation must succeed");
            c.consume_output(c.output().len());
            c.receive(b"354 data\r\n", now).expect("fixture operation must succeed");
            c.consume_output(c.output().len());
            c.receive(b"250 queued\r\n", now).expect("fixture operation must succeed");
            assert!(
                matches!(c.poll_event(), Some(Event::Sent { token: 1, info }) if info.accepted == ["b@example.test"])
            );
        });
        assert_eq!(
            count, 3,
            "accepted vector + recipient string + response string are the owned result"
        );
    }
}
