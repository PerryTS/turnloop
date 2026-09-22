#![deny(unsafe_op_in_unsafe_fn)]
//!
//! # Getting started on turnloop
//! Enable the `turnloop` feature for the `asynchronous` module. The embedding
//! host owns `LocalExecutor` and calls `turn`; adapters await its streams and
//! deadline futures. See the crate README and turnloop-io for ownership, streaming
//! and cancellation examples. Default features retain the sans-I/O API.

#![forbid(unsafe_code)]
//! Pull-based, bounded, sans-I/O PostgreSQL client.
//!
//! Feed bytes with `receive`, pull `next_event`, write `output`, then acknowledge
//! only transmitted bytes with `consume_output`. Events borrow reusable storage.
//! The host owns sockets, TLS, entropy, time and result materialization.

#[cfg(all(target_os = "wasi", target_env = "p3"))]
use turnloop_wasi_random as _;

use bytes::BytesMut;
pub use postgres_protocol::authentication::sasl::{ChannelBinding, ScramSha256};
use postgres_protocol::{
    IsNull,
    authentication::md5_hash,
    message::{backend::Header, frontend},
};
use std::{collections::VecDeque, fmt};
pub mod pool;
pub mod types;
mod wire;
use wire::Cursor;
pub use wire::{
    ConnectionFailure, Field, Fields, OwnedFields, OwnedRow, OwnedServerError, Row, ServerError,
};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod host_time;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use host_time::Instant;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant;

pub type Token = u64;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Protocol(&'static str),
    State(&'static str),
    Limit,
    Timeout,
    /// The transport failed. The host may attach its own diagnostic (for
    /// example `ECONNREFUSED`); it reaches every `Outcome::Aborted` and `Closed`.
    Transport(Option<TransportFailure>),
    /// The server terminated the session; preserves SQLSTATE and every field.
    ConnectionAborted(ConnectionFailure),
    Cancelled,
}
impl Error {
    /// A transport failure carrying the host's I/O error kind, OS code and text.
    pub fn transport(error: &std::io::Error) -> Self {
        Self::Transport(Some(error.into()))
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(s) | Self::State(s) => f.write_str(s),
            Self::Limit => f.write_str("protocol buffer limit exceeded"),
            Self::Timeout => f.write_str("Connection terminated due to timeout"),
            Self::Transport(None) => f.write_str("Connection terminated unexpectedly"),
            Self::Transport(Some(failure)) => {
                write!(f, "Connection terminated unexpectedly: {failure}")
            }
            Self::ConnectionAborted(error) => write!(f, "Connection aborted: {error}"),
            Self::Cancelled => f.write_str("Connection terminated"),
        }
    }
}
impl std::error::Error for Error {}
impl From<Error> for std::io::Error {
    fn from(error: Error) -> Self {
        let kind = match &error {
            Error::Transport(Some(failure)) => failure.kind(),
            Error::ConnectionAborted(_) | Error::Transport(None) => {
                std::io::ErrorKind::ConnectionAborted
            }
            Error::Timeout => std::io::ErrorKind::TimedOut,
            _ => std::io::ErrorKind::Other,
        };
        Self::new(kind, error)
    }
}
/// Host-supplied transport diagnostic. Clones share one copy of the message,
/// so aborting a pipeline of tokens allocates nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFailure {
    kind: std::io::ErrorKind,
    code: Option<i32>,
    message: std::sync::Arc<str>,
}
impl TransportFailure {
    /// `code` is whatever error number the host reports (OS errno, libuv code).
    pub fn new(
        kind: std::io::ErrorKind,
        code: Option<i32>,
        message: impl Into<std::sync::Arc<str>>,
    ) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }
    pub fn kind(&self) -> std::io::ErrorKind {
        self.kind
    }
    pub fn code(&self) -> Option<i32> {
        self.code
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}
impl From<&std::io::Error> for TransportFailure {
    fn from(error: &std::io::Error) -> Self {
        Self::new(error.kind(), error.raw_os_error(), error.to_string())
    }
}
impl fmt::Display for TransportFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Protocol("invalid PostgreSQL message")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    Prefer,
    Require,
}
#[derive(Clone)]
pub struct Config {
    pub user: String,
    pub password: Vec<u8>,
    pub database: String,
    pub application_name: String,
    pub ssl: SslMode,
    /// Require SCRAM-SHA-256-PLUS; the host supplies tls-server-end-point data.
    pub channel_binding_required: bool,
    pub max_buffer: usize,
    pub max_pending: usize,
    pub max_scram_iterations: u32,
    pub connect_deadline: Option<Instant>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            user: "postgres".into(),
            password: Vec::new(),
            database: "postgres".into(),
            application_name: "turnloop".into(),
            ssl: SslMode::Disable,
            channel_binding_required: false,
            max_buffer: 64 * 1024 * 1024,
            max_pending: 1024,
            max_scram_iterations: 1_000_000,
            connect_deadline: None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionStatus {
    Idle,
    InTransaction,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// A statement failed and ReadyForQuery confirmed the session can continue.
    ServerError,
    Aborted(Error),
}
#[derive(Debug)]
pub enum Event<'a> {
    UpgradeTls,
    /// Build upstream ScramSha256 outside the core (its constructor reads entropy)
    /// from `Connection::config().password`, then call `start_scram`.
    ScramNeeded {
        plus: bool,
    },
    Connected,
    ParameterStatus {
        name: &'a str,
        value: &'a str,
    },
    Fields {
        token: Token,
        fields: Fields<'a>,
    },
    Row {
        token: Token,
        row: Row<'a>,
    },
    CommandComplete {
        token: Token,
        tag: &'a str,
        row_count: Option<u64>,
    },
    Error {
        token: Option<Token>,
        error: ServerError<'a>,
    },
    Notice(ServerError<'a>),
    Notification {
        process_id: i32,
        channel: &'a str,
        payload: &'a str,
    },
    CopyIn {
        token: Token,
        binary: bool,
        column_formats: &'a [u8],
    },
    CopyOut {
        token: Token,
        binary: bool,
        column_formats: &'a [u8],
    },
    CopyData {
        token: Token,
        data: &'a [u8],
    },
    CopyDone {
        token: Token,
    },
    Completed {
        token: Token,
        outcome: Outcome,
        transaction: TransactionStatus,
    },
    Closed {
        reason: Error,
    },
}
/// [`Event`] with every borrowed payload copied out, for hosts that retain
/// rows, errors or notifications past the next mutable call. `as_event` lends
/// it back in borrowed form, so one host handler can consume either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedEvent {
    UpgradeTls,
    ScramNeeded {
        plus: bool,
    },
    Connected,
    ParameterStatus {
        name: String,
        value: String,
    },
    Fields {
        token: Token,
        fields: OwnedFields,
    },
    Row {
        token: Token,
        row: OwnedRow,
    },
    CommandComplete {
        token: Token,
        tag: String,
        row_count: Option<u64>,
    },
    Error {
        token: Option<Token>,
        error: OwnedServerError,
    },
    Notice(OwnedServerError),
    Notification {
        process_id: i32,
        channel: String,
        payload: String,
    },
    CopyIn {
        token: Token,
        binary: bool,
        column_formats: Vec<u8>,
    },
    CopyOut {
        token: Token,
        binary: bool,
        column_formats: Vec<u8>,
    },
    CopyData {
        token: Token,
        data: Vec<u8>,
    },
    CopyDone {
        token: Token,
    },
    Completed {
        token: Token,
        outcome: Outcome,
        transaction: TransactionStatus,
    },
    Closed {
        reason: Error,
    },
}
impl Event<'_> {
    /// Copy the event out of the connection's buffer. Only borrowed payloads
    /// allocate: each string, byte slice, row, field list or diagnostic once.
    pub fn into_owned(self) -> OwnedEvent {
        match self {
            Self::UpgradeTls => OwnedEvent::UpgradeTls,
            Self::ScramNeeded { plus } => OwnedEvent::ScramNeeded { plus },
            Self::Connected => OwnedEvent::Connected,
            Self::ParameterStatus { name, value } => OwnedEvent::ParameterStatus {
                name: name.into(),
                value: value.into(),
            },
            Self::Fields { token, fields } => OwnedEvent::Fields {
                token,
                fields: fields.into_owned(),
            },
            Self::Row { token, row } => OwnedEvent::Row {
                token,
                row: row.into_owned(),
            },
            Self::CommandComplete {
                token,
                tag,
                row_count,
            } => OwnedEvent::CommandComplete {
                token,
                tag: tag.into(),
                row_count,
            },
            Self::Error { token, error } => OwnedEvent::Error {
                token,
                error: error.into_owned(),
            },
            Self::Notice(error) => OwnedEvent::Notice(error.into_owned()),
            Self::Notification {
                process_id,
                channel,
                payload,
            } => OwnedEvent::Notification {
                process_id,
                channel: channel.into(),
                payload: payload.into(),
            },
            Self::CopyIn {
                token,
                binary,
                column_formats,
            } => OwnedEvent::CopyIn {
                token,
                binary,
                column_formats: column_formats.into(),
            },
            Self::CopyOut {
                token,
                binary,
                column_formats,
            } => OwnedEvent::CopyOut {
                token,
                binary,
                column_formats: column_formats.into(),
            },
            Self::CopyData { token, data } => OwnedEvent::CopyData {
                token,
                data: data.into(),
            },
            Self::CopyDone { token } => OwnedEvent::CopyDone { token },
            Self::Completed {
                token,
                outcome,
                transaction,
            } => OwnedEvent::Completed {
                token,
                outcome,
                transaction,
            },
            Self::Closed { reason } => OwnedEvent::Closed { reason },
        }
    }
}
impl OwnedEvent {
    /// Borrow the retained event in the form `next_event` produced it.
    pub fn as_event(&self) -> Event<'_> {
        match self {
            Self::UpgradeTls => Event::UpgradeTls,
            Self::ScramNeeded { plus } => Event::ScramNeeded { plus: *plus },
            Self::Connected => Event::Connected,
            Self::ParameterStatus { name, value } => Event::ParameterStatus { name, value },
            Self::Fields { token, fields } => Event::Fields {
                token: *token,
                fields: fields.fields(),
            },
            Self::Row { token, row } => Event::Row {
                token: *token,
                row: row.row(),
            },
            Self::CommandComplete {
                token,
                tag,
                row_count,
            } => Event::CommandComplete {
                token: *token,
                tag,
                row_count: *row_count,
            },
            Self::Error { token, error } => Event::Error {
                token: *token,
                error: error.server_error(),
            },
            Self::Notice(error) => Event::Notice(error.server_error()),
            Self::Notification {
                process_id,
                channel,
                payload,
            } => Event::Notification {
                process_id: *process_id,
                channel,
                payload,
            },
            Self::CopyIn {
                token,
                binary,
                column_formats,
            } => Event::CopyIn {
                token: *token,
                binary: *binary,
                column_formats,
            },
            Self::CopyOut {
                token,
                binary,
                column_formats,
            } => Event::CopyOut {
                token: *token,
                binary: *binary,
                column_formats,
            },
            Self::CopyData { token, data } => Event::CopyData {
                token: *token,
                data,
            },
            Self::CopyDone { token } => Event::CopyDone { token: *token },
            Self::Completed {
                token,
                outcome,
                transaction,
            } => Event::Completed {
                token: *token,
                outcome: outcome.clone(),
                transaction: *transaction,
            },
            Self::Closed { reason } => Event::Closed {
                reason: reason.clone(),
            },
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ssl,
    Tls,
    Auth,
    Scram(bool),
    Ready,
    Closing,
    Closed,
}
#[derive(Debug)]
struct Pending {
    token: Token,
    deadline: Option<Instant>,
    failed: bool,
    extended: bool,
    parse: Option<usize>,
}
struct Statement {
    name: String,
    sql: String,
    oids: Vec<u32>,
    parsed: bool,
}
/// A parameter is already encoded by the host or `types`; 0=text, 1=binary.
#[derive(Clone, Copy, Debug)]
pub struct Parameter<'a> {
    pub value: Option<&'a [u8]>,
    pub format: i16,
}

/// Parameters of one extended-protocol operation. An empty name is uncached.
#[derive(Clone, Copy, Debug)]
pub struct ExtendedQuery<'a> {
    pub name: &'a str,
    pub sql: &'a str,
    pub oids: &'a [u32],
    pub params: &'a [Parameter<'a>],
    pub result_formats: &'a [i16],
}

pub struct Connection {
    config: Config,
    state: State,
    input: Vec<u8>,
    input_at: usize,
    output: BytesMut,
    output_at: usize,
    pending: VecDeque<Pending>,
    statements: Vec<Statement>,
    scram: Option<ScramSha256>,
    auth_ok: bool,
    tls: bool,
    binding_available: bool,
    scram_plus: bool,
    copy_in: bool,
    transaction: TransactionStatus,
    key: Option<(i32, i32)>,
    reason: Error,
}
impl Connection {
    pub fn new(config: Config) -> Result<Self> {
        if config.max_buffer < 1024
            || config.max_buffer > i32::MAX as usize
            || config.max_pending == 0
        {
            return Err(Error::Limit);
        }
        if config.user.contains('\0')
            || config.database.contains('\0')
            || config.application_name.contains('\0')
        {
            return Err(Error::State("NUL in startup parameter"));
        }
        if config.channel_binding_required && config.ssl == SslMode::Disable {
            return Err(Error::State("channel binding requires TLS"));
        }
        let mut this = Self {
            config,
            state: State::Auth,
            input: Vec::with_capacity(8192),
            input_at: 0,
            output: BytesMut::with_capacity(8192),
            output_at: 0,
            pending: VecDeque::with_capacity(16),
            statements: Vec::new(),
            scram: None,
            auth_ok: false,
            tls: false,
            binding_available: false,
            scram_plus: false,
            copy_in: false,
            transaction: TransactionStatus::Idle,
            key: None,
            reason: Error::Transport(None),
        };
        if this.config.ssl == SslMode::Disable {
            this.startup()?;
        } else {
            this.state = State::Ssl;
            frontend::ssl_request(&mut this.output);
        }
        Ok(this)
    }
    fn startup(&mut self) -> Result<()> {
        frontend::startup_message(
            [
                ("user", self.config.user.as_str()),
                ("database", self.config.database.as_str()),
                ("application_name", self.config.application_name.as_str()),
                ("client_encoding", "UTF8"),
                ("DateStyle", "ISO"),
            ],
            &mut self.output,
        )?;
        self.state = State::Auth;
        Ok(())
    }
    /// The configuration this connection was built from. On `ScramNeeded`,
    /// build `ScramSha256` from `config().password`; the host keeps no copy.
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn output(&self) -> &[u8] {
        &self.output[self.output_at..]
    }
    pub fn consume_output(&mut self, n: usize) -> Result<()> {
        if n > self.output().len() {
            return Err(Error::State("invalid output acknowledgement"));
        }
        self.output_at += n;
        if self.output_at == self.output.len() {
            self.output.clear();
            self.output_at = 0;
        }
        Ok(())
    }
    pub fn receive(&mut self, bytes: &[u8]) -> Result<()> {
        if matches!(self.state, State::Tls | State::Closing | State::Closed) {
            return Err(Error::State("connection cannot receive plaintext now"));
        }
        let remaining = self.input.len() - self.input_at;
        if bytes.len() > self.config.max_buffer.saturating_sub(remaining) {
            self.abort(Error::Limit);
            return Err(Error::Limit);
        }
        if self.input_at != 0 {
            self.input.copy_within(self.input_at.., 0);
            self.input.truncate(remaining);
            self.input_at = 0;
        }
        self.input.extend_from_slice(bytes);
        Ok(())
    }
    /// Acknowledge TLS without channel-binding data (plain SCRAM fallback).
    pub fn tls_established(&mut self) -> Result<()> {
        self.tls_established_with_channel_binding(false)
    }
    /// Acknowledge verified TLS and whether the host can supply tls-server-end-point
    /// data to `start_scram`. The core prefers PLUS only when data is available.
    pub fn tls_established_with_channel_binding(&mut self, available: bool) -> Result<()> {
        if self.state != State::Tls || !self.output().is_empty() {
            return Err(Error::State("TLS transition not ready"));
        }
        if self.config.channel_binding_required && !available {
            return Err(Error::State(
                "channel binding required but certificate binding is unavailable",
            ));
        }
        self.tls = true;
        self.binding_available = available;
        self.startup()
    }
    pub fn start_scram(&mut self, scram: ScramSha256) -> Result<()> {
        let State::Scram(plus) = self.state else {
            return Err(Error::State("SCRAM was not requested"));
        };
        if plus != scram.message().starts_with(b"p=tls-server-end-point,") {
            return Err(Error::State("SCRAM channel binding mismatch"));
        }
        frontend::sasl_initial_response(
            if plus {
                "SCRAM-SHA-256-PLUS"
            } else {
                "SCRAM-SHA-256"
            },
            scram.message(),
            &mut self.output,
        )?;
        self.scram = Some(scram);
        self.state = State::Auth;
        Ok(())
    }
    fn accept(&self, token: Token) -> Result<()> {
        if self.state != State::Ready || self.copy_in {
            return Err(Error::State("Client is not queryable"));
        }
        if self.pending.len() >= self.config.max_pending {
            return Err(Error::Limit);
        }
        if self.pending.iter().any(|p| p.token == token) {
            return Err(Error::State("duplicate operation token"));
        }
        Ok(())
    }
    fn finish_command(
        &mut self,
        token: Token,
        deadline: Option<Instant>,
        parse: Option<usize>,
        before: usize,
        result: Result<()>,
        extended: bool,
    ) -> Result<()> {
        if let Err(e) = result {
            self.output.truncate(before);
            return Err(e);
        }
        if self.output.len() > self.config.max_buffer {
            self.output.truncate(before);
            return Err(Error::Limit);
        }
        self.pending.push_back(Pending {
            token,
            deadline,
            failed: false,
            extended,
            parse,
        });
        Ok(())
    }
    pub fn query(&mut self, token: Token, sql: &str, deadline: Option<Instant>) -> Result<()> {
        self.accept(token)?;
        if sql.len().saturating_add(6) > self.config.max_buffer.saturating_sub(self.output.len()) {
            return Err(Error::Limit);
        }
        let before = self.output.len();
        let result = frontend::query(sql, &mut self.output).map_err(Error::from);
        self.finish_command(token, deadline, None, before, result, false)
    }
    /// Every operation ends in Sync, so an error cannot discard a later token.
    pub fn execute(
        &mut self,
        token: Token,
        query: ExtendedQuery<'_>,
        deadline: Option<Instant>,
    ) -> Result<()> {
        let ExtendedQuery {
            name,
            sql,
            oids,
            params,
            result_formats,
        } = query;
        self.accept(token)?;
        let size = params.iter().fold(
            64usize
                .saturating_add(name.len().saturating_mul(2))
                .saturating_add(sql.len())
                .saturating_add(oids.len().saturating_mul(4))
                .saturating_add(result_formats.len().saturating_mul(2)),
            |n, p| {
                n.saturating_add(6)
                    .saturating_add(p.value.map_or(0, <[u8]>::len))
            },
        );
        if size > self.config.max_buffer.saturating_sub(self.output.len()) {
            return Err(Error::Limit);
        }

        if params.iter().any(|p| !matches!(p.format, 0 | 1))
            || result_formats.iter().any(|f| !matches!(f, 0 | 1))
        {
            return Err(Error::State("invalid format"));
        }
        let index = if name.is_empty() {
            None
        } else {
            self.statements.iter().position(|s| s.name == name)
        };
        if let Some(i) = index {
            let s = &self.statements[i];
            if s.sql != sql || s.oids != oids {
                return Err(Error::State(
                    "Prepared statements must be unique - a different statement was used for this name",
                ));
            }
        }
        // Refuse reuse until ParseComplete; otherwise a pipelined parse failure
        // could make a later accepted operation refer to a nonexistent statement.
        if index.is_some_and(|i| !self.statements[i].parsed) {
            return Err(Error::State("named statement parse still pending"));
        }
        let before = self.output.len();
        let result = (|| {
            if index.is_none() {
                frontend::parse(name, sql, oids.iter().copied(), &mut self.output)?;
            }
            frontend::bind(
                "",
                name,
                params.iter().map(|p| p.format),
                params.iter(),
                |p, out| {
                    if let Some(value) = p.value {
                        out.extend_from_slice(value);
                        Ok(IsNull::No)
                    } else {
                        Ok(IsNull::Yes)
                    }
                },
                result_formats.iter().copied(),
                &mut self.output,
            )
            .map_err(|_| Error::Protocol("invalid Bind message"))?;
            frontend::describe(b'P', "", &mut self.output)?;
            frontend::execute("", 0, &mut self.output)?;
            frontend::sync(&mut self.output);
            Ok(())
        })();
        let new_index = if result.is_ok()
            && self.output.len() <= self.config.max_buffer
            && index.is_none()
            && !name.is_empty()
        {
            let i = self.statements.len();
            self.statements.push(Statement {
                name: name.into(),
                sql: sql.into(),
                oids: oids.into(),
                parsed: false,
            });
            Some(i)
        } else {
            None
        };
        self.finish_command(token, deadline, new_index, before, result, true)
    }
    pub fn copy_data(&mut self, bytes: &[u8]) -> Result<()> {
        if !self.copy_in {
            return Err(Error::State("COPY IN is not active"));
        }
        if bytes.len().saturating_add(5) > self.config.max_buffer.saturating_sub(self.output.len())
        {
            return Err(Error::Limit);
        }
        frontend::CopyData::new(bytes)?.write(&mut self.output);
        Ok(())
    }
    pub fn copy_finish(&mut self, error: Option<&str>) -> Result<()> {
        if !self.copy_in {
            return Err(Error::State("COPY IN is not active"));
        }
        let before = self.output.len();
        let result = if let Some(message) = error {
            frontend::copy_fail(message, &mut self.output)
        } else {
            frontend::copy_done(&mut self.output);
            Ok(())
        };
        // The earlier Sync is ignored by the server inside COPY IN.
        // Extended protocol needs a fresh Sync after CopyDone/CopyFail.
        if result.is_ok() && self.pending.front().is_some_and(|p| p.extended) {
            frontend::sync(&mut self.output);
        }
        if result.is_err() || self.output.len() > self.config.max_buffer {
            self.output.truncate(before);
            return Err(Error::Limit);
        }
        self.copy_in = false;
        Ok(())
    }
    /// Send on a NEW connection; the main stream remains owned by this state machine.
    pub fn cancel_request(&self) -> Option<[u8; 16]> {
        self.key.map(|(pid, key)| {
            let mut b = [0; 16];
            b[..4].copy_from_slice(&16u32.to_be_bytes());
            b[4..8].copy_from_slice(&80877102u32.to_be_bytes());
            b[8..12].copy_from_slice(&pid.to_be_bytes());
            b[12..].copy_from_slice(&key.to_be_bytes());
            b
        })
    }
    pub fn transaction_status(&self) -> TransactionStatus {
        self.transaction
    }
    pub fn is_ready(&self) -> bool {
        self.state == State::Ready
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        if matches!(self.state, State::Closing | State::Closed) {
            return None;
        }
        self.pending
            .iter()
            .filter_map(|p| p.deadline)
            .chain(if self.state != State::Ready {
                self.config.connect_deadline
            } else {
                None
            })
            .min()
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.next_timeout().is_some_and(|t| t <= now) {
            self.abort(Error::Timeout);
        }
    }
    /// Transport EOF, TLS failure, cancellation or malformed input terminates all
    /// pending tokens once. Pull Completed events followed by Closed.
    pub fn abort(&mut self, reason: Error) {
        if matches!(self.state, State::Closing | State::Closed) {
            return;
        }
        self.reason = reason;
        self.state = State::Closing;
        self.output.clear();
        self.output_at = 0;
        self.copy_in = false;
    }
    pub fn end(&mut self) -> Result<()> {
        if !self.pending.is_empty() || self.state != State::Ready {
            return Err(Error::State("drain queries before end"));
        }
        frontend::terminate(&mut self.output);
        self.state = State::Closing;
        self.reason = Error::Cancelled;
        Ok(())
    }
    fn token(&self) -> Result<Token> {
        self.pending
            .front()
            .map(|p| p.token)
            .ok_or(Error::Protocol("unsolicited query response"))
    }
    /// Pull the next borrowed event. A terminal server ErrorResponse returns
    /// `Error::ConnectionAborted` immediately and makes the session unusable.
    /// Continue pulling to drain one Aborted completion per pending token, then
    /// Closed; a later transport abort cannot replace the server diagnostic.
    pub fn next_event(&mut self) -> Result<Option<Event<'_>>> {
        if self.state == State::Closing {
            if let Some(p) = self.pending.pop_front() {
                return Ok(Some(Event::Completed {
                    token: p.token,
                    outcome: Outcome::Aborted(self.reason.clone()),
                    transaction: self.transaction,
                }));
            }
            if !self.output().is_empty() {
                return Ok(None);
            }
            self.state = State::Closed;
            return Ok(Some(Event::Closed {
                reason: self.reason.clone(),
            }));
        }
        if matches!(self.state, State::Closed | State::Tls | State::Scram(_)) {
            return Ok(None);
        }
        if self.state == State::Ssl {
            let Some(&reply) = self.input.get(self.input_at) else {
                return Ok(None);
            };
            self.input_at += 1;
            match reply {
                b'S' => {
                    if self.input.len() != self.input_at {
                        return Err(Error::Protocol("plaintext after TLS acceptance"));
                    }
                    self.state = State::Tls;
                    return Ok(Some(Event::UpgradeTls));
                }
                b'N' if self.config.ssl == SslMode::Prefer => {
                    self.startup()?;
                }
                b'N' => {
                    return Err(Error::Protocol(
                        "The server does not support SSL connections",
                    ));
                }
                _ => return Err(Error::Protocol("invalid SSLRequest response")),
            }
        }
        loop {
            let available = &self.input[self.input_at..];
            let Some(header) = Header::parse(available)? else {
                return Ok(None);
            };
            if header.len() < 4 {
                return Err(Error::Protocol("invalid message length"));
            }
            let length = header.len() as usize + 1;
            if length > self.config.max_buffer {
                return Err(Error::Limit);
            }
            if available.len() < length {
                return Ok(None);
            }
            let start = self.input_at + 5;
            self.input_at += length;
            let body = &self.input[start..self.input_at];
            let mut c = Cursor(body);
            match header.tag() {
                b'R' if self.state == State::Auth => match c.u32()? {
                    0 => {
                        c.end()?;
                        if self.scram.is_some() {
                            return Err(Error::Protocol("SCRAM final verification missing"));
                        }
                        if self.config.channel_binding_required && !self.scram_plus {
                            return Err(Error::Protocol(
                                "channel binding required but SCRAM-PLUS was not authenticated",
                            ));
                        }
                        self.auth_ok = true;
                    }
                    3 => {
                        c.end()?;
                        if self.config.channel_binding_required {
                            return Err(Error::Protocol("server did not offer channel binding"));
                        }
                        frontend::password_message(&self.config.password, &mut self.output)?;
                    }
                    5 => {
                        let salt = c.take(4)?.try_into().unwrap();
                        c.end()?;
                        if self.config.channel_binding_required {
                            return Err(Error::Protocol("server did not offer channel binding"));
                        }
                        let hash =
                            md5_hash(self.config.user.as_bytes(), &self.config.password, salt);
                        frontend::password_message(hash.as_bytes(), &mut self.output)?;
                    }
                    10 => {
                        let mut plain = false;
                        let mut plus = false;
                        loop {
                            match c.cstr()? {
                                "" => break,
                                "SCRAM-SHA-256" => plain = true,
                                "SCRAM-SHA-256-PLUS" => plus = true,
                                _ => {}
                            }
                        }
                        c.end()?;
                        let plus = self.tls && self.binding_available && plus;
                        if self.config.channel_binding_required && !plus {
                            return Err(Error::Protocol(
                                "channel binding required but server did not offer SCRAM-SHA-256-PLUS",
                            ));
                        }
                        if !plus && !plain {
                            return Err(Error::Protocol("unsupported SASL mechanisms"));
                        }
                        self.scram_plus = plus;
                        self.state = State::Scram(plus);
                        return Ok(Some(Event::ScramNeeded { plus }));
                    }
                    11 => {
                        let iterations = std::str::from_utf8(c.0)
                            .ok()
                            .and_then(|s| s.split(',').find_map(|p| p.strip_prefix("i=")))
                            .and_then(|s| s.parse::<u32>().ok())
                            .ok_or(Error::Protocol("invalid SCRAM iterations"))?;
                        if iterations == 0 || iterations > self.config.max_scram_iterations {
                            return Err(Error::Limit);
                        }
                        let scram = self
                            .scram
                            .as_mut()
                            .ok_or(Error::Protocol("unexpected SASL continuation"))?;
                        scram.update(c.0)?;
                        frontend::sasl_response(scram.message(), &mut self.output)?;
                    }
                    12 => {
                        let mut scram = self
                            .scram
                            .take()
                            .ok_or(Error::Protocol("unexpected SASL final"))?;
                        scram.finish(c.0)?;
                    }
                    _ => return Err(Error::Protocol("unsupported authentication method")),
                },
                b'S' => {
                    let name = c.cstr()?;
                    let value = c.cstr()?;
                    c.end()?;
                    return Ok(Some(Event::ParameterStatus { name, value }));
                }
                b'K' => {
                    self.key = Some((c.i32()?, c.i32()?));
                    c.end()?;
                }
                b'Z' => {
                    self.transaction = match c.u8()? {
                        b'I' => TransactionStatus::Idle,
                        b'T' => TransactionStatus::InTransaction,
                        b'E' => TransactionStatus::Failed,
                        _ => return Err(Error::Protocol("invalid transaction status")),
                    };
                    c.end()?;
                    self.copy_in = false;
                    if self.state == State::Auth {
                        if !self.auth_ok {
                            return Err(Error::Protocol("Ready before authentication"));
                        }
                        self.state = State::Ready;
                        return Ok(Some(Event::Connected));
                    }
                    let p = self
                        .pending
                        .pop_front()
                        .ok_or(Error::Protocol("unsolicited ReadyForQuery"))?;
                    // Keep stable cache indices; failed names can be registered again.
                    if let Some(i) = p.parse
                        && !self.statements[i].parsed
                    {
                        self.statements[i].name.clear();
                    }
                    return Ok(Some(Event::Completed {
                        token: p.token,
                        outcome: if p.failed {
                            Outcome::ServerError
                        } else {
                            Outcome::Success
                        },
                        transaction: self.transaction,
                    }));
                }
                b'T' => {
                    return Ok(Some(Event::Fields {
                        token: self.token()?,
                        fields: Fields::parse(body)?,
                    }));
                }
                b'D' => {
                    return Ok(Some(Event::Row {
                        token: self.token()?,
                        row: Row::parse(body)?,
                    }));
                }
                b'C' => {
                    let tag = c.cstr()?;
                    c.end()?;
                    let row_count = tag.rsplit(' ').next().and_then(|s| s.parse().ok());
                    return Ok(Some(Event::CommandComplete {
                        token: self.token()?,
                        tag,
                        row_count,
                    }));
                }
                b'I' => {
                    c.end()?;
                    return Ok(Some(Event::CommandComplete {
                        token: self.token()?,
                        tag: "",
                        row_count: None,
                    }));
                }
                b'E' => {
                    let error = ServerError::parse(body)?;
                    let token = self.pending.front_mut().map(|p| {
                        p.failed = true;
                        p.token
                    });
                    if token.is_none()
                        || matches!(
                            error.get(b'V').or(error.severity()),
                            Some("FATAL" | "PANIC")
                        )
                    {
                        self.state = State::Closing;
                        self.reason = Error::ConnectionAborted(ConnectionFailure::new(error));
                        self.output.clear();
                        self.output_at = 0;
                        self.copy_in = false;
                        // FATAL/PANIC needs no ReadyForQuery or EOF confirmation.
                        // Subsequent pulls still complete every queued token once.
                        return Err(self.reason.clone());
                    }
                    return Ok(Some(Event::Error { token, error }));
                }
                b'N' => return Ok(Some(Event::Notice(ServerError::parse(body)?))),
                b'A' => {
                    let process_id = c.i32()?;
                    let channel = c.cstr()?;
                    let payload = c.cstr()?;
                    c.end()?;
                    return Ok(Some(Event::Notification {
                        process_id,
                        channel,
                        payload,
                    }));
                }
                b'1' => {
                    c.end()?;
                    let p = self
                        .pending
                        .front_mut()
                        .ok_or(Error::Protocol("unsolicited ParseComplete"))?;
                    if let Some(i) = p.parse {
                        self.statements[i].parsed = true;
                    }
                }
                b'2' | b'3' | b'n' => {
                    c.end()?;
                    self.token()?;
                }
                b'G' | b'H' => {
                    let binary = match c.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err(Error::Protocol("invalid COPY format")),
                    };
                    let count = c.u16()? as usize;
                    let column_formats = c.take(count * 2)?;
                    c.end()?;
                    let token = self.token()?;
                    if header.tag() == b'G' {
                        if self.pending.len() != 1 {
                            return Err(Error::Protocol("COPY IN requires an exclusive query"));
                        }
                        self.copy_in = true;
                        return Ok(Some(Event::CopyIn {
                            token,
                            binary,
                            column_formats,
                        }));
                    }
                    return Ok(Some(Event::CopyOut {
                        token,
                        binary,
                        column_formats,
                    }));
                }
                b'd' => {
                    return Ok(Some(Event::CopyData {
                        token: self.token()?,
                        data: body,
                    }));
                }
                b'c' => {
                    c.end()?;
                    return Ok(Some(Event::CopyDone {
                        token: self.token()?,
                    }));
                }
                _ => return Err(Error::Protocol("unexpected backend message")),
            }
        }
    }
}

#[cfg(feature = "turnloop")]
pub mod asynchronous;
