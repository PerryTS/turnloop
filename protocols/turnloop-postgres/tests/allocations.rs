#![deny(unsafe_op_in_unsafe_fn)]
#[path = "support/allocation.rs"]
mod allocation;
use turnloop_postgres::*;
#[test]
fn warmed_query_row_and_named_execute_allocate_nothing() {
    let mut c = Connection::new(Config::default()).unwrap();
    c.consume_output(c.output().len()).unwrap();
    c.receive(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0, b'Z', 0, 0, 0, 5, b'I'])
        .unwrap();
    assert!(matches!(c.next_event().unwrap(), Some(Event::Connected)));
    let response = [
        b'D', 0, 0, 0, 12, 0, 1, 0, 0, 0, 2, b'4', b'2', b'C', 0, 0, 0, 13, b'S', b'E', b'L', b'E',
        b'C', b'T', b' ', b'1', 0, b'Z', 0, 0, 0, 5, b'I',
    ];
    let mut rows = 0;
    let mut complete = 0;
    let mut run = || {
        c.query(1, "SELECT 42", None).unwrap();
        c.consume_output(c.output().len()).unwrap();
        c.receive(&response).unwrap();
        while let Some(e) = c.next_event().unwrap() {
            match e {
                Event::Row { mut row, .. } => {
                    assert_eq!(row.next().unwrap().unwrap(), Some(&b"42"[..]));
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
    c.execute(2, q, None).unwrap();
    c.consume_output(c.output().len()).unwrap();
    c.receive(&[b'1', 0, 0, 0, 4, b'2', 0, 0, 0, 4]).unwrap();
    assert!(c.next_event().unwrap().is_none());
    c.receive(&response).unwrap();
    while c.next_event().unwrap().is_some() {}
    let count = allocation::allocations(|| {
        for _ in 0..1000 {
            c.execute(2, q, None).unwrap();
            assert_eq!(c.output()[0], b'B');
            c.consume_output(c.output().len()).unwrap();
            c.receive(&response).unwrap();
            while c.next_event().unwrap().is_some() {}
        }
    });
    assert_eq!(count, 0);
}
