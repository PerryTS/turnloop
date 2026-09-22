#![deny(unsafe_op_in_unsafe_fn)]
#[path = "support/allocation.rs"]
mod allocation;
use turnloop_postgres::*;
#[test]
fn warmed_query_row_and_named_execute_allocate_nothing() {
    let mut c = Connection::new(Config::default()).expect("fixture operation must succeed");
    c.consume_output(c.output().len())
        .expect("fixture operation must succeed");
    c.receive(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'])
        .expect("fixture operation must succeed");
    assert!(matches!(
        c.next_event().expect("fixture operation must succeed"),
        Some(Event::Connected)
    ));
    let response = [
        b'D', 0, 0, 0, 12, 0, 1, 0, 0, 0, 2, b'4', b'2', b'C', 0, 0, 0, 13, b'S', b'E', b'L', b'E',
        b'C', b'T', b' ', b'1', 0, b'Z', 0, 0, 0, 5, b'I',
    ];
    let mut rows = 0;
    let mut complete = 0;
    let mut run = || {
        c.query(1, "SELECT 42", None)
            .expect("fixture operation must succeed");
        c.consume_output(c.output().len())
            .expect("fixture operation must succeed");
        c.receive(&response)
            .expect("fixture operation must succeed");
        while let Some(e) = c.next_event().expect("fixture operation must succeed") {
            match e {
                Event::Row { mut row, .. } => {
                    assert_eq!(
                        row.next()
                            .expect("fixture operation must succeed")
                            .expect("fixture operation must succeed"),
                        Some(&b"42"[..])
                    );
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
    assert_eq!(count, 0);
    assert_eq!(rows, 1010);
    assert_eq!(complete, 1010);
    let q = ExtendedQuery {
        name: "cached",
        sql: "SELECT $1::int4",
        oids: &[23],
        params: &[Parameter {
            value: Some(b"42"),
            format: 0,
        }],
        result_formats: &[0],
    };
    c.execute(2, q, None)
        .expect("fixture operation must succeed");
    c.consume_output(c.output().len())
        .expect("fixture operation must succeed");
    c.receive(&[b'1', 0, 0, 0, 4, b'2', 0, 0, 0, 4])
        .expect("fixture operation must succeed");
    assert!(
        c.next_event()
            .expect("fixture operation must succeed")
            .is_none()
    );
    c.receive(&response)
        .expect("fixture operation must succeed");
    while c
        .next_event()
        .expect("fixture operation must succeed")
        .is_some()
    {}
    let count = allocation::allocations(|| {
        for _ in 0..1000 {
            c.execute(2, q, None)
                .expect("fixture operation must succeed");
            assert_eq!(c.output()[0], b'B');
            c.consume_output(c.output().len())
                .expect("fixture operation must succeed");
            c.receive(&response)
                .expect("fixture operation must succeed");
            while c
                .next_event()
                .expect("fixture operation must succeed")
                .is_some()
            {}
        }
    });
    assert_eq!(count, 0);
}

#[test]
fn terminal_diagnostic_allocates_once_and_pipeline_abort_clones_allocate_zero() {
    assert_eq!(
        allocation::allocations(|| {
            std::hint::black_box(Box::new(42));
        }),
        1
    );
    let mut c = Connection::new(Config::default()).expect("core");
    c.consume_output(c.output().len()).expect("startup sent");
    c.receive(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I").expect("auth");
    assert!(matches!(
        c.next_event().expect("connected"),
        Some(Event::Connected)
    ));
    for token in 0..64 {
        c.query(token, "SELECT 1", None).expect("pipeline");
    }
    let body = b"SFATAL\0C57P01\0Mterminated by administrator\0\0";
    let packet = [
        b"E".as_slice(),
        &((body.len() + 4) as u32).to_be_bytes(),
        body,
    ]
    .concat();
    c.receive(&packet).expect("fatal response");
    let mut diagnostic = None;
    assert_eq!(
        allocation::allocations(|| {
            diagnostic = Some(c.next_event().expect_err("fatal"));
        }),
        1,
        "one shared copy of the terminal fields"
    );
    let diagnostic = diagnostic.expect("owned error");
    assert_eq!(
        allocation::allocations(|| {
            c.abort(Error::Transport(None));
            for token in 0..64 {
                match c.next_event().expect("completion") {
                    Some(Event::Completed {
                        token: actual,
                        outcome: Outcome::Aborted(error),
                        ..
                    }) => {
                        assert_eq!(actual, token);
                        assert_eq!(error, diagnostic);
                    }
                    event => panic!("missing pipeline abort: {event:?}"),
                }
            }
            assert!(
                matches!(c.next_event().expect("close"), Some(Event::Closed { reason }) if reason == diagnostic)
            );
            assert!(c.next_event().expect("no duplicate").is_none());
        }),
        0
    );
    assert_eq!(c.pending_count(), 0);

    // A host transport diagnostic is built once; fanning it out to every
    // pending token and the close shares that copy.
    let mut c = Connection::new(Config::default()).expect("core");
    c.consume_output(c.output().len()).expect("startup sent");
    c.receive(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I").expect("auth");
    assert!(matches!(
        c.next_event().expect("connected"),
        Some(Event::Connected)
    ));
    for token in 0..64 {
        c.query(token, "SELECT 1", None).expect("pipeline");
    }
    let reason = Error::Transport(Some(TransportFailure::new(
        std::io::ErrorKind::ConnectionReset,
        Some(-104),
        "read ECONNRESET",
    )));
    assert_eq!(
        allocation::allocations(|| {
            c.abort(reason.clone());
            for token in 0..64 {
                match c.next_event().expect("completion") {
                    Some(Event::Completed {
                        token: actual,
                        outcome: Outcome::Aborted(error),
                        ..
                    }) => {
                        assert_eq!(actual, token);
                        assert_eq!(error, reason);
                    }
                    event => panic!("missing pipeline abort: {event:?}"),
                }
            }
            assert!(
                matches!(c.next_event().expect("close"), Some(Event::Closed { reason: r }) if r == reason)
            );
        }),
        0
    );
}
