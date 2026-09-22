//! `CursorBatch::rows()` captures only the reply lifetime (issue #69). Under
//! Rust 2024 capture rules an `impl Iterator` return also captures `&self`
//! unless it says `use<'a>`: the iterator then borrows the local batch although
//! its rows borrow only the reply, and `rows` below fails with E0597. The
//! `match` in `first_row` fails the same way in edition-2021 host crates, whose
//! tail-expression temporaries outlive the block's locals.
use turnloop_mongodb::{
    Error, Result,
    bson::{
        doc,
        raw::{RawDocument, RawDocumentBuf},
    },
    command::CursorBatch,
};

/// The tail-position pattern from the issue: the scrutinee borrows `batch`,
/// a local dropped at the end of the block, while the row escapes it.
fn first_row(reply: &RawDocument) -> Option<Result<&RawDocument>> {
    let batch = CursorBatch::parse(reply).ok()?;
    match batch.rows().next() {
        Some(row) => Some(row),
        None => None,
    }
}

/// Returning the iterator itself requires that it not borrow the local batch.
fn rows(reply: &RawDocument) -> Result<impl Iterator<Item = Result<&RawDocument>>> {
    let batch = CursorBatch::parse(reply)?;
    Ok(batch.rows())
}

fn x(row: Result<&RawDocument>) -> Result<i32> {
    row?.get_i32("x").map_err(|_| Error::protocol("missing x"))
}

#[test]
fn rows_outlive_the_batch_that_produced_them() {
    let reply = RawDocumentBuf::try_from(&doc! {
        "ok": 1,
        "cursor": {"id": 0_i64, "ns": "db.items", "firstBatch": [{"x": 1}, {"x": 2}]},
    })
    .expect("reply");
    assert_eq!(x(first_row(&reply).expect("first row")).expect("x"), 1);
    let xs: Vec<i32> = rows(&reply)
        .expect("batch")
        .map(x)
        .collect::<Result<_>>()
        .expect("rows");
    assert_eq!(xs, [1, 2]);

    let empty = RawDocumentBuf::try_from(&doc! {
        "ok": 1,
        "cursor": {"id": 0_i64, "ns": "db.items", "nextBatch": []},
    })
    .expect("reply");
    assert!(first_row(&empty).is_none());
    assert_eq!(rows(&empty).expect("batch").count(), 0);
}
