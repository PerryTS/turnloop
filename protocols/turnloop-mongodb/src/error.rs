use bson::{raw::RawDocument, Document};
use std::{borrow::Cow, fmt};
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Parse,
    Protocol,
    Authentication,
    Server,
    BulkWrite,
    Network,
    Timeout,
    PoolCleared,
    PoolClosed,
    InvalidArgument,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: Cow<'static, str>,
    pub code: Option<i32>,
    pub code_name: Option<String>,
    pub labels: Vec<String>,
    /// Full owned server result only on failure, preserving writeErrors and errInfo.
    pub response: Option<Box<Document>>,
}
impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
            code_name: None,
            labels: Vec::new(),
            response: None,
        }
    }
    pub fn protocol(message: &'static str) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }
    pub fn name(&self) -> &'static str {
        match self.kind {
            ErrorKind::Parse => "MongoParseError",
            ErrorKind::Protocol => "MongoUnexpectedServerResponseError",
            ErrorKind::Authentication | ErrorKind::Server => "MongoServerError",
            ErrorKind::BulkWrite => "MongoBulkWriteError",
            ErrorKind::Network => "MongoNetworkError",
            ErrorKind::Timeout => "MongoNetworkTimeoutError",
            ErrorKind::PoolCleared => "MongoPoolClearedError",
            ErrorKind::PoolClosed => "MongoPoolClosedError",
            ErrorKind::InvalidArgument => "MongoInvalidArgumentError",
            ErrorKind::Cancelled => "MongoOperationTimeoutError",
        }
    }
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|x| x == label)
    }
    pub fn from_response(raw: &RawDocument) -> Result<()> {
        let ok = number(raw, "ok").unwrap_or(0.0) != 0.0;
        let write = raw
            .get("writeErrors")
            .ok()
            .flatten()
            .and_then(|v| v.as_array())
            .is_some_and(|a| a.into_iter().next().is_some());
        let concern = raw
            .get("writeConcernError")
            .ok()
            .flatten()
            .and_then(|v| v.as_document());
        if ok && !write && concern.is_none() {
            return Ok(());
        }
        let doc: Document = raw
            .try_into()
            .map_err(|_| Self::protocol("Invalid BSON error response"))?;
        let detail = if write {
            doc.get_array("writeErrors")
                .ok()
                .and_then(|a| a.first())
                .and_then(|v| v.as_document())
                .unwrap_or(&doc)
        } else {
            doc.get_document("writeConcernError").unwrap_or(&doc)
        };
        let mut err = Self::new(
            if write {
                ErrorKind::BulkWrite
            } else {
                ErrorKind::Server
            },
            detail
                .get_str("errmsg")
                .unwrap_or("MongoDB command failed")
                .to_owned(),
        );
        err.code = detail.get_i32("code").ok();
        err.code_name = detail.get_str("codeName").ok().map(str::to_owned);
        err.labels = doc
            .get_array("errorLabels")
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|b| b.as_str().map(str::to_owned))
            .collect();
        err.response = Some(Box::new(doc));
        Err(err)
    }
}
pub(crate) fn number(d: &RawDocument, k: &str) -> Option<f64> {
    match d.get(k).ok().flatten()? {
        bson::raw::RawBsonRef::Double(v) => Some(v),
        bson::raw::RawBsonRef::Int32(v) => Some(f64::from(v)),
        bson::raw::RawBsonRef::Int64(v) => Some(v as f64),
        _ => None,
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.name(), self.message)
    }
}
impl std::error::Error for Error {}
