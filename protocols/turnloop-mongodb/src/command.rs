//! crud/crud.md §§ Read/Write Operations, Write Models and Results.
//! Builders retain capacity. Inputs and options are borrowed raw BSON; IDs and
//! wall-clock ObjectId timestamps are supplied by the host (no ObjectId::new()).
use crate::{Error, ErrorKind, Result, wire::BsonWriter};
use bson::{
    Document,
    raw::{RawArray, RawBsonRef, RawDocument},
};
#[derive(Debug, Default)]
pub struct Command {
    writer: BsonWriter,
    scratch: BsonWriter,
}
impl Command {
    pub fn new() -> Self {
        Self {
            writer: BsonWriter::new(),
            scratch: BsonWriter::new(),
        }
    }
    pub fn raw(&self) -> &RawDocument {
        self.writer
            .as_raw()
            .expect("Command builder must finish successfully")
    }
    /// Applies URI-level defaults only when the command has no explicit override.
    /// Call before sizing BulkBatcher; no owned BSON documents are constructed.
    pub fn apply_client_options(
        &mut self,
        options: &crate::uri::Options,
        write: bool,
    ) -> Result<()> {
        self.scratch.clear();
        self.scratch.append_fields(self.writer.as_raw()?, &[])?;
        if write
            && self
                .writer
                .as_raw()?
                .get("writeConcern")
                .ok()
                .flatten()
                .is_none()
            && ["w", "journal", "wtimeoutms"]
                .iter()
                .any(|k| options.raw.contains_key(*k))
        {
            let wc = self.scratch.start_document("writeConcern", false)?;
            if let Some(w) = options.raw.get("w") {
                if let Ok(n) = w.parse::<i32>() {
                    self.scratch.int32("w", n)?;
                } else {
                    self.scratch.string("w", w)?;
                }
            }
            if let Some(j) = options.raw.get("journal") {
                self.scratch.boolean("j", j == "true")?;
            }
            if let Some(n) = options.raw.get("wtimeoutms") {
                self.scratch.int64(
                    "wtimeout",
                    n.parse::<i64>().map_err(|_| {
                        Error::new(ErrorKind::InvalidArgument, "wtimeoutMS out of range")
                    })?,
                )?;
            }
            self.scratch.end_document(wc)?;
        }
        if !write {
            if let Some(level) = options.raw.get("readconcernlevel")
                && self
                    .writer
                    .as_raw()?
                    .get("readConcern")
                    .ok()
                    .flatten()
                    .is_none()
            {
                let rc = self.scratch.start_document("readConcern", false)?;
                self.scratch.string("level", level)?;
                self.scratch.end_document(rc)?;
            }
            if options.read_preference != crate::uri::ReadPreference::Primary
                && self
                    .writer
                    .as_raw()?
                    .get("$readPreference")
                    .ok()
                    .flatten()
                    .is_none()
            {
                let rp = self.scratch.start_document("$readPreference", false)?;
                self.scratch
                    .string("mode", options.read_preference.as_str())?;
                if !options.read_preference_tags.is_empty() {
                    let tags = self.scratch.start_document("tags", true)?;
                    for (i, set) in options.read_preference_tags.iter().enumerate() {
                        let mut b = [0; 20];
                        let tag = self.scratch.start_document(index(i, &mut b), false)?;
                        for (k, v) in set {
                            self.scratch.string(k, v)?;
                        }
                        self.scratch.end_document(tag)?;
                    }
                    self.scratch.end_document(tags)?;
                }
                if let Some(max) = options.max_staleness {
                    self.scratch
                        .int64("maxStalenessSeconds", max.as_secs() as i64)?;
                }
                self.scratch.end_document(rp)?;
            }
        }
        self.scratch.finish()?;
        std::mem::swap(&mut self.writer, &mut self.scratch);
        Ok(())
    }
    fn begin(&mut self, name: &str, collection: Option<&str>) -> Result<()> {
        self.writer.clear();
        if let Some(c) = collection {
            if c.is_empty() || c.contains('\0') {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "Invalid collection name",
                ));
            }
            self.writer.string(name, c)
        } else {
            self.writer.int32(name, 1)
        }
    }
    fn end(&mut self, db: &str, options: Option<&RawDocument>, omit: &[&str]) -> Result<()> {
        if let Some(o) = options {
            self.writer.append_fields(o, omit)?;
        }
        self.writer.string("$db", db)?;
        self.writer.finish()?;
        Ok(())
    }
    /// User command fields remain in original order; only the database is appended.
    pub fn run(&mut self, db: &str, body: &RawDocument) -> Result<()> {
        self.writer.clear();
        self.writer.append_fields(body, &["$db"])?;
        self.end(db, None, &[])
    }
    pub fn insert(
        &mut self,
        db: &str,
        coll: &str,
        ordered: bool,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("insert", Some(coll))?;
        self.writer.boolean("ordered", ordered)?;
        self.end(db, options, &["insert", "documents", "ordered", "$db"])
    }
    pub fn update(
        &mut self,
        db: &str,
        coll: &str,
        ordered: bool,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("update", Some(coll))?;
        self.writer.boolean("ordered", ordered)?;
        self.end(db, options, &["update", "updates", "ordered", "$db"])
    }
    pub fn delete(
        &mut self,
        db: &str,
        coll: &str,
        ordered: bool,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("delete", Some(coll))?;
        self.writer.boolean("ordered", ordered)?;
        self.end(db, options, &["delete", "deletes", "ordered", "$db"])
    }
    pub fn find(
        &mut self,
        db: &str,
        coll: &str,
        filter: &RawDocument,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("find", Some(coll))?;
        self.writer.document("filter", filter)?;
        self.end(db, options, &["find", "filter", "$db"])
    }
    pub fn find_one(&mut self, db: &str, coll: &str, filter: &RawDocument) -> Result<()> {
        self.begin("find", Some(coll))?;
        self.writer.document("filter", filter)?;
        self.writer.int64("limit", 1)?;
        self.writer.boolean("singleBatch", true)?;
        self.end(db, None, &[])
    }
    pub fn aggregate(
        &mut self,
        db: &str,
        coll: &str,
        pipeline: &[&RawDocument],
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("aggregate", Some(coll))?;
        let at = self.writer.start_document("pipeline", true)?;
        for (i, d) in pipeline.iter().enumerate() {
            let mut b = [0u8; 20];
            self.writer.document(index(i, &mut b), d)?;
        }
        self.writer.end_document(at)?;
        if options.is_none_or(|o| o.get_document("cursor").is_err()) {
            let c = self.writer.start_document("cursor", false)?;
            self.writer.end_document(c)?;
        }
        self.end(db, options, &["aggregate", "pipeline", "$db"])
    }
    /// countDocuments is an aggregation; it never uses the metadata-based count command.
    pub fn count_documents(
        &mut self,
        db: &str,
        coll: &str,
        filter: &RawDocument,
        skip: i64,
        limit: i64,
    ) -> Result<()> {
        if skip < 0 || limit < 0 {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "count skip/limit must be nonnegative",
            ));
        }
        self.begin("aggregate", Some(coll))?;
        let pipeline = self.writer.start_document("pipeline", true)?;
        let stage = self.writer.start_document("0", false)?;
        self.writer.document("$match", filter)?;
        self.writer.end_document(stage)?;
        let mut i = 1;
        for (key, n) in [("$skip", skip), ("$limit", limit)] {
            if n > 0 {
                let mut b = [0; 20];
                let s = self.writer.start_document(index(i, &mut b), false)?;
                self.writer.int64(key, n)?;
                self.writer.end_document(s)?;
                i += 1;
            }
        }
        let mut b = [0; 20];
        let stage = self.writer.start_document(index(i, &mut b), false)?;
        let group = self.writer.start_document("$group", false)?;
        self.writer.int32("_id", 1)?;
        let n = self.writer.start_document("n", false)?;
        self.writer.int32("$sum", 1)?;
        self.writer.end_document(n)?;
        self.writer.end_document(group)?;
        self.writer.end_document(stage)?;
        self.writer.end_document(pipeline)?;
        let c = self.writer.start_document("cursor", false)?;
        self.writer.end_document(c)?;
        self.end(db, None, &[])
    }
    pub fn estimated_document_count(
        &mut self,
        db: &str,
        coll: &str,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("count", Some(coll))?;
        self.end(db, options, &["count", "$db"])
    }
    pub fn distinct(
        &mut self,
        db: &str,
        coll: &str,
        key: &str,
        filter: &RawDocument,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("distinct", Some(coll))?;
        self.writer.string("key", key)?;
        self.writer.document("query", filter)?;
        self.end(db, options, &["distinct", "key", "query", "$db"])
    }
    /// `update=None` means findOneAndDelete. Options include new, upsert, projection
    /// (wire name fields), sort, arrayFilters and writeConcern.
    pub fn find_one_and_modify(
        &mut self,
        db: &str,
        coll: &str,
        filter: &RawDocument,
        update: Option<&RawDocument>,
        options: Option<&RawDocument>,
    ) -> Result<()> {
        self.begin("findAndModify", Some(coll))?;
        self.writer.document("query", filter)?;
        if let Some(u) = update {
            self.writer.document("update", u)?;
        } else {
            self.writer.boolean("remove", true)?;
        }
        self.end(
            db,
            options,
            &["findAndModify", "query", "update", "remove", "$db"],
        )
    }
    pub fn create_indexes(
        &mut self,
        db: &str,
        coll: &str,
        indexes: &[&RawDocument],
        options: Option<&RawDocument>,
    ) -> Result<()> {
        if indexes.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "Index list must not be empty",
            ));
        }
        self.begin("createIndexes", Some(coll))?;
        let a = self.writer.start_document("indexes", true)?;
        for (i, d) in indexes.iter().enumerate() {
            let mut b = [0; 20];
            self.writer.document(index(i, &mut b), d)?;
        }
        self.writer.end_document(a)?;
        self.end(db, options, &["createIndexes", "indexes", "$db"])
    }
    pub fn list_indexes(&mut self, db: &str, coll: &str) -> Result<()> {
        self.begin("listIndexes", Some(coll))?;
        self.end(db, None, &[])
    }
    pub fn drop_index(&mut self, db: &str, coll: &str, name: &str) -> Result<()> {
        self.begin("dropIndexes", Some(coll))?;
        self.writer.string("index", name)?;
        self.end(db, None, &[])
    }
    pub fn list_databases(&mut self, name_only: bool) -> Result<()> {
        self.begin("listDatabases", None)?;
        self.writer.boolean("nameOnly", name_only)?;
        self.end("admin", None, &[])
    }
    pub fn list_collections(
        &mut self,
        db: &str,
        name_only: bool,
        filter: &RawDocument,
    ) -> Result<()> {
        self.begin("listCollections", None)?;
        self.writer.boolean("nameOnly", name_only)?;
        self.writer.document("filter", filter)?;
        self.end(db, None, &[])
    }
    pub fn get_more(
        &mut self,
        db: &str,
        coll: &str,
        id: i64,
        batch_size: Option<i32>,
        max_time_ms: Option<i64>,
    ) -> Result<()> {
        self.writer.clear();
        self.writer.int64("getMore", id)?;
        self.writer.string("collection", coll)?;
        if let Some(n) = batch_size {
            if n <= 0 {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "getMore batchSize must be positive",
                ));
            }
            self.writer.int32("batchSize", n)?;
        }
        if let Some(n) = max_time_ms {
            self.writer.int64("maxTimeMS", n)?;
        }
        self.end(db, None, &[])
    }
    pub fn kill_cursor(&mut self, db: &str, coll: &str, id: i64) -> Result<()> {
        self.begin("killCursors", Some(coll))?;
        let a = self.writer.start_document("cursors", true)?;
        self.writer.int64("0", id)?;
        self.writer.end_document(a)?;
        self.end(db, None, &[])
    }
}
fn index(mut n: usize, b: &mut [u8; 20]) -> &str {
    let mut at = b.len();
    loop {
        at -= 1;
        b[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    std::str::from_utf8(&b[at..]).unwrap()
}
/// A write sequence entry (updateOne/updateMany/replaceOne, deleteOne/deleteMany).
#[derive(Default)]
pub struct WriteModel {
    writer: BsonWriter,
}
impl WriteModel {
    pub fn update(
        &mut self,
        filter: &RawDocument,
        update: &RawDocument,
        multi: bool,
        upsert: bool,
        options: Option<&RawDocument>,
    ) -> Result<&RawDocument> {
        self.writer.clear();
        self.writer.document("q", filter)?;
        self.writer.document("u", update)?;
        self.writer.boolean("multi", multi)?;
        self.writer.boolean("upsert", upsert)?;
        if let Some(o) = options {
            self.writer
                .append_fields(o, &["q", "u", "multi", "upsert"])?;
        }
        self.writer.finish()
    }
    pub fn delete(
        &mut self,
        filter: &RawDocument,
        many: bool,
        options: Option<&RawDocument>,
    ) -> Result<&RawDocument> {
        self.writer.clear();
        self.writer.document("q", filter)?;
        self.writer.int32("limit", if many { 0 } else { 1 })?;
        if let Some(o) = options {
            self.writer.append_fields(o, &["q", "limit"])?;
        }
        self.writer.finish()
    }
}
/// No per-row allocation: documents borrow the raw response buffer.
pub struct CursorBatch<'a> {
    pub id: i64,
    pub namespace: &'a str,
    pub documents: &'a RawArray,
    pub post_batch_resume_token: Option<&'a RawDocument>,
}
impl<'a> CursorBatch<'a> {
    pub fn parse(reply: &'a RawDocument) -> Result<Self> {
        crate::Error::from_response(reply)?;
        let c = reply
            .get_document("cursor")
            .map_err(|_| Error::protocol("Missing cursor"))?;
        Ok(Self {
            id: c
                .get_i64("id")
                .map_err(|_| Error::protocol("Missing cursor id"))?,
            namespace: c
                .get_str("ns")
                .map_err(|_| Error::protocol("Missing cursor namespace"))?,
            documents: c
                .get("firstBatch")
                .ok()
                .flatten()
                .or_else(|| c.get("nextBatch").ok().flatten())
                .and_then(|v| v.as_array())
                .ok_or_else(|| Error::protocol("Missing cursor batch"))?,
            post_batch_resume_token: c
                .get("postBatchResumeToken")
                .ok()
                .flatten()
                .and_then(|v| v.as_document()),
        })
    }
    /// The iterator borrows the reply, not this batch, so it may outlive `self`.
    pub fn rows(&self) -> impl Iterator<Item = Result<&'a RawDocument>> + use<'a> {
        self.documents.into_iter().map(|v| match v {
            Ok(RawBsonRef::Document(d)) => Ok(d),
            _ => Err(Error::protocol("Cursor batch contains non-document")),
        })
    }
}
#[derive(Debug)]
pub struct Cursor {
    pub id: i64,
    pub database: String,
    pub collection: String,
    pub server: String,
    pub batch_size: Option<i32>,
    remaining: Option<u64>,
}
impl Cursor {
    pub fn new(
        namespace: &str,
        server: String,
        limit: Option<u64>,
        batch_size: Option<i32>,
    ) -> Result<Self> {
        let (db, coll) = namespace
            .split_once('.')
            .ok_or_else(|| Error::protocol("Invalid cursor namespace"))?;
        Ok(Self {
            id: 0,
            database: db.into(),
            collection: coll.into(),
            server,
            batch_size,
            remaining: limit.filter(|v| *v != 0),
        })
    }
    /// Returns number of rows to expose; when limit reached, host must kill any live id.
    pub fn accept(&mut self, batch: &CursorBatch<'_>) -> Result<usize> {
        if batch.namespace.split_once('.')
            != Some((self.database.as_str(), self.collection.as_str()))
        {
            return Err(Error::protocol("Cursor namespace changed"));
        }
        self.id = batch.id;
        let count = batch.rows().try_fold(0usize, |n, row| row.map(|_| n + 1))?;
        let take = self.remaining.map_or(count, |n| count.min(n as usize));
        if let Some(n) = &mut self.remaining {
            *n -= take as u64;
        }
        Ok(take)
    }
    pub fn needs_kill(&self) -> bool {
        self.id != 0 && self.remaining == Some(0)
    }
    pub fn get_more(&self, cmd: &mut Command) -> Result<bool> {
        if self.id == 0 || self.needs_kill() {
            return Ok(false);
        }
        let size = match (self.batch_size, self.remaining) {
            (Some(n), Some(r)) => Some(n.min(r.min(i32::MAX as u64) as i32)),
            (n, _) => n,
        };
        cmd.get_more(&self.database, &self.collection, self.id, size, None)?;
        Ok(true)
    }
}
/// Counts and error detail of an insert/update/delete reply.
///
/// MongoDB reports a failed write *inside* a successful command: a duplicate
/// key answers `ok: 1` with a `writeErrors` array, and an unsatisfied write
/// concern answers `ok: 1` with `writeConcernError`. `ok` alone is therefore no
/// verdict. [`WriteResult::parse`] is the verdict: it runs
/// [`Error::from_response`] first and fails on either field, so a caller never
/// needs to call `from_response` beforehand. [`WriteResult::decode`] keeps them
/// as data for aggregation (as [`BulkResult`] does); check
/// [`WriteResult::succeeded`] on what it returns.
#[derive(Debug)]
pub struct WriteResult<'a> {
    pub count: i64,
    pub modified_count: i64,
    pub upserted: Option<&'a RawArray>,
    /// Never nonempty in a [`WriteResult::parse`] result.
    pub write_errors: Option<&'a RawArray>,
    /// Never present in a [`WriteResult::parse`] result.
    pub write_concern_error: Option<&'a RawDocument>,
}
impl<'a> WriteResult<'a> {
    /// Succeeds only for a write that fully succeeded. `ok: 0`, a nonempty
    /// `writeErrors` (a [`ErrorKind::BulkWrite`] error) and a
    /// `writeConcernError` (a [`ErrorKind::Server`] error) all fail with the
    /// error [`Error::from_response`] builds, full server response included.
    pub fn parse(r: &'a RawDocument) -> Result<Self> {
        Error::from_response(r)?;
        Self::decode(r)
    }
    /// Fails only when the command itself failed (`ok: 0`) or the reply is
    /// malformed; per-document and write-concern errors are returned in the
    /// fields, so callers must consult [`WriteResult::succeeded`].
    pub fn decode(r: &'a RawDocument) -> Result<Self> {
        if crate::error::number(r, "ok").unwrap_or(0.0) == 0.0 {
            Error::from_response(r)?;
        }
        Ok(Self {
            count: integer(r, "n")?,
            modified_count: integer(r, "nModified").unwrap_or(0),
            upserted: r.get("upserted").ok().flatten().and_then(|v| v.as_array()),
            write_errors: r
                .get("writeErrors")
                .ok()
                .flatten()
                .and_then(|v| v.as_array()),
            write_concern_error: r
                .get("writeConcernError")
                .ok()
                .flatten()
                .and_then(|v| v.as_document()),
        })
    }
    /// False when any document failed or the write concern was not satisfied.
    pub fn succeeded(&self) -> bool {
        self.write_errors
            .is_none_or(|a| a.into_iter().next().is_none())
            && self.write_concern_error.is_none()
    }
}
fn integer(d: &RawDocument, k: &str) -> Result<i64> {
    match d.get(k).ok().flatten() {
        Some(RawBsonRef::Int32(v)) => Ok(i64::from(v)),
        Some(RawBsonRef::Int64(v)) => Ok(v),
        _ => Err(Error::protocol("Missing integer result field")),
    }
}

/// Aggregates split batches with original indices preserved. Allocation is solely
/// owned result/error representation; success counters do not allocate.
#[derive(Debug, Default)]
pub struct BulkResult {
    pub count: i64,
    pub modified_count: i64,
    pub write_errors: Vec<Document>,
    pub write_concern_errors: Vec<Document>,
}
impl BulkResult {
    pub fn accept(&mut self, reply: &RawDocument, offset: i32, ordered: bool) -> Result<bool> {
        let result = WriteResult::decode(reply)?;
        self.count += result.count;
        self.modified_count += result.modified_count;
        let mut errors = false;
        if let Some(a) = result.write_errors {
            for e in a {
                let e = e.map_err(|_| Error::protocol("Invalid write error"))?;
                let d = e
                    .as_document()
                    .ok_or_else(|| Error::protocol("Invalid write error"))?;
                let mut d: Document = d
                    .try_into()
                    .map_err(|_| Error::protocol("Invalid write error BSON"))?;
                d.insert(
                    "index",
                    d.get_i32("index")
                        .map_err(|_| Error::protocol("Missing write error index"))?
                        + offset,
                );
                self.write_errors.push(d);
                errors = true;
            }
        }
        if let Some(c) = result.write_concern_error {
            self.write_concern_errors.push(
                c.try_into()
                    .map_err(|_| Error::protocol("Invalid write concern error"))?,
            );
        }
        Ok(!(ordered && errors))
    }
}

/// Splits a write sequence against all three negotiated size limits without
/// copying documents. Accumulate results with BulkResult and this batch's offset.
pub struct BulkBatcher<'a> {
    documents: &'a [&'a RawDocument],
    at: usize,
    overhead: usize,
    max_message: usize,
    max_bson: usize,
    max_count: usize,
}
pub struct BulkBatch<'a> {
    pub offset: usize,
    pub documents: &'a [&'a RawDocument],
}
impl<'a> BulkBatcher<'a> {
    pub fn new(
        documents: &'a [&'a RawDocument],
        body: &RawDocument,
        identifier: &str,
        max_message: usize,
        max_bson: usize,
        max_count: usize,
    ) -> Result<Self> {
        let overhead = 16 + 4 + 1 + body.as_bytes().len() + 1 + 4 + identifier.len() + 1;
        if documents.is_empty()
            || max_count == 0
            || overhead >= max_message
            || body.as_bytes().len() > max_bson
        {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "Invalid bulk write size or empty batch",
            ));
        }
        for d in documents {
            if d.as_bytes().len() > max_bson || d.as_bytes().len() > max_message - overhead {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "Document exceeds MongoDB size limit",
                ));
            }
        }
        Ok(Self {
            documents,
            at: 0,
            overhead,
            max_message,
            max_bson,
            max_count,
        })
    }
    pub fn next_batch(&mut self) -> Option<BulkBatch<'a>> {
        if self.at == self.documents.len() {
            return None;
        }
        let start = self.at;
        let mut bytes = self.overhead;
        while self.at < self.documents.len() && self.at - start < self.max_count {
            let n = self.documents[self.at].as_bytes().len();
            if n > self.max_bson || n > self.max_message - bytes {
                break;
            }
            bytes += n;
            self.at += 1;
        }
        Some(BulkBatch {
            offset: start,
            documents: &self.documents[start..self.at],
        })
    }
}
/// ObjectId generator with host-supplied process entropy and wall-clock seconds.
/// BSON ObjectId specification § Generation: 4 timestamp, 5 random, 3 counter bytes.
pub struct ObjectIdGenerator {
    random: [u8; 5],
    counter: u32,
}
impl ObjectIdGenerator {
    pub fn new(random: [u8; 5], counter: u32) -> Self {
        Self {
            random,
            counter: counter & 0x00ff_ffff,
        }
    }
    pub fn generate(&mut self, unix_seconds: u32) -> bson::oid::ObjectId {
        let mut bytes = [0; 12];
        bytes[..4].copy_from_slice(&unix_seconds.to_be_bytes());
        bytes[4..9].copy_from_slice(&self.random);
        bytes[9..].copy_from_slice(&self.counter.to_be_bytes()[1..]);
        self.counter = self.counter.wrapping_add(1) & 0x00ff_ffff;
        bson::oid::ObjectId::from_bytes(bytes)
    }
}
