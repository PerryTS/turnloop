//! Retry orchestration for an operation across server selection and pool checkout.
//! retryable-reads/retryable-reads.md § Executing Retryable Read Commands;
//! retryable-writes/retryable-writes.md § Executing Retryable Write Commands.
//! The adapter performs the action returned by `action` and reports its outcome.
//! Input BSON and wire templates are copied into retained buffers once per command.
use crate::Instant;
use crate::{
    Connection, Error, ErrorKind, Result,
    retry::{Retry, RetryKind},
    uri::ReadPreference,
    wire::{self, BsonWriter, Message},
};
use bson::raw::{RawBsonRef, RawDocument};
use std::time::Duration;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Read,
    Write,
    RunCommand,
}
#[derive(Clone, Copy, Debug)]
pub struct RetrySession {
    pub id: [u8; 16],
    pub txn_number: i64,
}
#[derive(Clone, Debug)]
pub struct OperationOptions {
    pub token: u64,
    pub kind: OperationKind,
    pub retry: bool,
    pub timeout: Option<Duration>,
    pub session: Option<RetrySession>,
    pub read_preference: ReadPreference,
}
impl Default for OperationOptions {
    fn default() -> Self {
        Self {
            token: 0,
            kind: OperationKind::RunCommand,
            retry: false,
            timeout: None,
            session: None,
            read_preference: ReadPreference::Primary,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ServerCapabilities {
    pub wire_version: i32,
    pub sessions: bool,
    pub standalone: bool,
    pub direct: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Select,
    Checkout,
    Send,
    Waiting,
    Complete,
    Failed,
}
#[derive(Debug)]
pub enum OperationAction<'a> {
    Idle,
    Select {
        deprioritized: Option<&'a str>,
        read_preference: ReadPreference,
    },
    Checkout {
        address: &'a str,
    },
    Send {
        address: &'a str,
    },
    Waiting,
    Complete {
        token: u64,
    },
    Failed {
        token: u64,
        error: &'a Error,
    },
}
pub struct Operation {
    state: State,
    options: OperationOptions,
    original: Vec<u8>,
    outgoing: Vec<u8>,
    writer: BsonWriter,
    server: String,
    failed_server: String,
    deadline: Option<Instant>,
    policy: Retry,
    first_error: Option<Error>,
    error: Option<Error>,
}
impl Default for Operation {
    fn default() -> Self {
        Self::new()
    }
}
impl Operation {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            options: OperationOptions::default(),
            original: Vec::with_capacity(8192),
            outgoing: Vec::with_capacity(8192),
            writer: BsonWriter::new(),
            server: String::with_capacity(256),
            failed_server: String::with_capacity(256),
            deadline: None,
            policy: Retry {
                kind: RetryKind::Never,
                enabled: false,
                attempts: 0,
                in_transaction: false,
                wire_version: 0,
                sessions_supported: false,
                standalone: false,
                acknowledged: true,
            },
            first_error: None,
            error: None,
        }
    }
    pub fn begin(
        &mut self,
        body: &RawDocument,
        sequences: &[(&str, &[&RawDocument])],
        mut options: OperationOptions,
        now: Instant,
    ) -> Result<()> {
        if !matches!(self.state, State::Idle | State::Complete | State::Failed) {
            return Err(Error::protocol("Operation already active"));
        }
        if body.get_str("$db").is_err() {
            return Err(Error::protocol("Command requires $db"));
        }
        if body
            .get("writeConcern")
            .ok()
            .flatten()
            .and_then(|v| v.as_document())
            .is_some_and(|d| {
                matches!(
                    d.get("w"),
                    Ok(Some(RawBsonRef::Int32(0) | RawBsonRef::Int64(0)))
                )
            })
        {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "Use Connection::command for unacknowledged writes",
            ));
        }
        if let Some(lsid) = body
            .get("lsid")
            .ok()
            .flatten()
            .and_then(|v| v.as_document())
        {
            let id = lsid
                .get_binary("id")
                .map_err(|_| Error::protocol("Invalid session id"))?;
            if id.bytes.len() != 16 || id.subtype != bson::spec::BinarySubtype::Uuid {
                return Err(Error::protocol("Session id must be UUID binary"));
            }
            let id: [u8; 16] = id.bytes.try_into().unwrap();
            if options.session.is_some_and(|s| s.id != id) {
                return Err(Error::protocol("Conflicting session identities"));
            }
            // Explicit sessions preserve the caller's already decorated txnNumber.
            options.session = body
                .get("txnNumber")
                .ok()
                .flatten()
                .and_then(|v| v.as_i64())
                .map(|txn_number| RetrySession { id, txn_number });
        }
        wire::encode(
            &mut self.original,
            1,
            0,
            0,
            body,
            sequences,
            wire::DEFAULT_MAX_MESSAGE,
        )?;
        let m = Message::parse(&self.original, wire::DEFAULT_MAX_MESSAGE)?;
        let name = m
            .body
            .iter_elements()
            .next()
            .transpose()
            .map_err(|_| Error::protocol("Invalid command"))?
            .ok_or_else(|| Error::protocol("Empty command"))?;
        let transaction = m.body.get("autocommit").ok().flatten().is_some();
        let kind = if transaction {
            RetryKind::Never
        } else {
            match options.kind {
                OperationKind::RunCommand => RetryKind::Never,
                OperationKind::Read => {
                    if Retry::read_command(name.key().as_str()) && !writes_pipeline(body) {
                        RetryKind::Read
                    } else {
                        RetryKind::Never
                    }
                }
                OperationKind::Write => {
                    if retryable_write(&m)? {
                        RetryKind::Write
                    } else {
                        RetryKind::Never
                    }
                }
            }
        };
        self.policy = Retry {
            kind,
            enabled: options.retry,
            attempts: 0,
            in_transaction: transaction,
            wire_version: 0,
            sessions_supported: false,
            standalone: false,
            acknowledged: true,
        };
        self.deadline = options.timeout.and_then(|d| now.checked_add(d));
        self.options = options;
        self.state = State::Select;
        self.first_error = None;
        self.error = None;
        self.server.clear();
        self.failed_server.clear();
        Ok(())
    }
    pub fn action(&self) -> OperationAction<'_> {
        match self.state {
            State::Idle => OperationAction::Idle,
            State::Select => OperationAction::Select {
                deprioritized: if self.failed_server.is_empty() {
                    None
                } else {
                    Some(&self.failed_server)
                },
                read_preference: if self.options.kind == OperationKind::Write {
                    ReadPreference::Primary
                } else {
                    self.options.read_preference
                },
            },
            State::Checkout => OperationAction::Checkout {
                address: &self.server,
            },
            State::Send => OperationAction::Send {
                address: &self.server,
            },
            State::Waiting => OperationAction::Waiting,
            State::Complete => OperationAction::Complete {
                token: self.options.token,
            },
            State::Failed => OperationAction::Failed {
                token: self.options.token,
                error: self.error.as_ref().unwrap(),
            },
        }
    }
    pub fn selected(&mut self, address: &str, capabilities: ServerCapabilities) -> Result<()> {
        if self.state != State::Select {
            return Err(Error::protocol("Operation is not selecting a server"));
        }
        self.policy.wire_version = capabilities.wire_version;
        self.policy.sessions_supported = capabilities.sessions;
        self.policy.standalone = capabilities.standalone;
        let can_write_retry = capabilities.sessions
            && !capabilities.standalone
            && capabilities.wire_version >= 6
            && self.options.session.is_some();
        if self.policy.attempts > 0
            && ((self.policy.kind == RetryKind::Write && !can_write_retry)
                || capabilities.wire_version < 6)
        {
            let err = self.first_error.take().unwrap();
            self.finish_error(err);
            return Ok(());
        }
        if self.policy.kind == RetryKind::Write && !can_write_retry {
            self.policy.enabled = false;
        }
        self.server.clear();
        self.server.push_str(address);
        let message = Message::parse(&self.original, wire::DEFAULT_MAX_MESSAGE)?;
        self.writer.clear();
        self.writer.append_fields(message.body, &[])?;
        if let Some(session) = self.options.session
            && capabilities.sessions
            && !self.policy.in_transaction
            && message.body.get("lsid").ok().flatten().is_none()
        {
            let l = self.writer.start_document("lsid", false)?;
            self.writer.binary("id", 4, &session.id)?;
            self.writer.end_document(l)?;
            if self.policy.kind == RetryKind::Write && self.policy.enabled {
                self.writer.int64("txnNumber", session.txn_number)?;
            }
        }
        let pref = if capabilities.direct && self.options.read_preference == ReadPreference::Primary
        {
            ReadPreference::PrimaryPreferred
        } else {
            self.options.read_preference
        };
        if self.options.kind == OperationKind::Read
            && pref != ReadPreference::Primary
            && message.body.get("$readPreference").ok().flatten().is_none()
        {
            let p = self.writer.start_document("$readPreference", false)?;
            self.writer.string("mode", pref.as_str())?;
            self.writer.end_document(p)?;
        }
        let body = self.writer.finish()?;
        message.encode_with_body(&mut self.outgoing, body, wire::DEFAULT_MAX_MESSAGE)?;
        self.state = State::Checkout;
        Ok(())
    }
    pub fn checked_out(&mut self) -> Result<()> {
        if self.state != State::Checkout {
            return Err(Error::protocol(
                "Operation is not checking out a connection",
            ));
        }
        self.state = State::Send;
        Ok(())
    }
    pub fn encoded_command(&self) -> Result<&[u8]> {
        if self.state != State::Send {
            return Err(Error::protocol("Operation is not ready to send"));
        }
        Ok(&self.outgoing)
    }
    pub fn send(&mut self, connection: &mut Connection, now: Instant) -> Result<()> {
        if self.state != State::Send {
            return Err(Error::protocol("Operation is not ready to send"));
        }
        connection.command_encoded(self.options.token, &self.outgoing, now)?;
        self.state = State::Waiting;
        Ok(())
    }
    /// For a host transport that sends encoded_command directly (e.g. a test driver).
    pub fn sent(&mut self) -> Result<()> {
        if self.state != State::Send {
            return Err(Error::protocol("Operation is not ready to send"));
        }
        self.state = State::Waiting;
        Ok(())
    }
    /// Returns true only for the terminal successful reply. Host releases the
    /// connection's reply buffer after handling this result; retries do not escape.
    pub fn response(&mut self, reply: &RawDocument) -> Result<bool> {
        if self.state != State::Waiting {
            return Err(Error::protocol("Unexpected operation reply"));
        }
        match Error::from_response(reply) {
            Ok(()) => {
                self.state = State::Complete;
                self.deadline = None;
                Ok(true)
            }
            Err(e) => {
                self.failed(e);
                Ok(false)
            }
        }
    }
    pub fn failed(&mut self, mut error: Error) {
        if matches!(self.state, State::Complete | State::Failed | State::Idle) {
            return;
        }
        if self.state == State::Select {
            let e = self.first_error.take().unwrap_or(error);
            self.finish_error(e);
            return;
        }
        if self.policy.kind == RetryKind::Write
            && self.policy.enabled
            && !error.has_label("RetryableWriteError")
        {
            let local = matches!(
                error.kind,
                ErrorKind::Network | ErrorKind::Timeout | ErrorKind::PoolCleared
            );
            let old_server = self.policy.wire_version < 9
                && error.kind != ErrorKind::BulkWrite
                && error.code.is_some_and(|c| {
                    matches!(
                        c,
                        6 | 7 | 89 | 91 | 189 | 262 | 9001 | 10107 | 11600 | 11602 | 13435 | 13436
                    )
                });
            if local || old_server {
                error.labels.push("RetryableWriteError".into());
            }
        }
        if self.policy.retry(&error) {
            self.first_error = Some(error);
            self.failed_server.clear();
            self.failed_server.push_str(&self.server);
            self.state = State::Select;
        } else {
            let e = if error.has_label("NoWritesPerformed") {
                self.first_error.take().unwrap_or(error)
            } else {
                error
            };
            self.finish_error(e);
        }
    }
    fn finish_error(&mut self, error: Error) {
        self.error = Some(error);
        self.state = State::Failed;
        self.deadline = None;
    }
    /// The failure which triggered the current retry, or the terminal failure.
    pub fn last_error(&self) -> Option<&Error> {
        self.error.as_ref().or(self.first_error.as_ref())
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.deadline.is_some_and(|d| d <= now) {
            self.finish_error(Error::new(
                ErrorKind::OperationTimeout,
                "MongoDB operation timed out",
            ));
        }
    }
    pub fn cancel(&mut self) {
        if !matches!(self.state, State::Idle | State::Complete | State::Failed) {
            self.finish_error(Error::new(
                ErrorKind::Cancelled,
                "MongoDB operation cancelled",
            ));
        }
    }
}
fn writes_pipeline(body: &RawDocument) -> bool {
    body.get("pipeline")
        .ok()
        .flatten()
        .and_then(|v| v.as_array())
        .is_some_and(|a| {
            a.into_iter().any(|v| {
                v.ok().and_then(|v| v.as_document()).is_some_and(|d| {
                    ["$out", "$merge"]
                        .iter()
                        .any(|k| d.get(k).ok().flatten().is_some())
                })
            })
        })
}
fn retryable_write(m: &Message<'_>) -> Result<bool> {
    let first = m
        .body
        .iter_elements()
        .next()
        .transpose()
        .map_err(|_| Error::protocol("Invalid command"))?
        .ok_or_else(|| Error::protocol("Empty command"))?;
    let name = first.key().as_str();
    match name {
        "insert" | "findAndModify" => Ok(true),
        "update" | "delete" => {
            let key = if name == "update" {
                "updates"
            } else {
                "deletes"
            };
            let eligible = |d: &RawDocument| {
                if name == "update" {
                    !matches!(d.get("multi"), Ok(Some(RawBsonRef::Boolean(true))))
                } else {
                    matches!(
                        d.get("limit"),
                        Ok(Some(RawBsonRef::Int32(1) | RawBsonRef::Int64(1)))
                    )
                }
            };
            let mut count = 0;
            for seq in m.sequences() {
                if seq.identifier == key {
                    for d in seq.documents() {
                        count += 1;
                        if !eligible(d) {
                            return Ok(false);
                        }
                    }
                }
            }
            if let Some(a) = m.body.get(key).ok().flatten().and_then(|v| v.as_array()) {
                for v in a {
                    let v = v.map_err(|_| Error::protocol("Invalid write model"))?;
                    let d = v
                        .as_document()
                        .ok_or_else(|| Error::protocol("Write model must be document"))?;
                    count += 1;
                    if !eligible(d) {
                        return Ok(false);
                    }
                }
            }
            Ok(count > 0)
        }
        _ => Ok(false),
    }
}
