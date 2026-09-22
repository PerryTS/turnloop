#![deny(unsafe_op_in_unsafe_fn)]
//!
//! # Getting started on turnloop
//! Enable the `turnloop` feature for the `asynchronous` module. The embedding
//! host owns `LocalExecutor` and calls `turn`; adapters await its streams and
//! deadline futures. See the crate README and turnloop-io for ownership, streaming
//! and cancellation examples. Default features retain the sans-I/O API.

#![forbid(unsafe_code)]
//! Pull-driven MySQL protocol. The host owns transport, entropy, time and JS conversion.
#[cfg(all(target_os = "wasi", target_env = "p3"))]
use turnloop_wasi_random as _;

use bytes::BytesMut;
use mysql_common::{
    auth::plugins::{
        caching_sha2_password::scramble_sha256, mysql_native_password::scramble_native,
    },
    constants::CapabilityFlags as Caps,
    io::ParseBuf,
    packets::{
        AuthPlugin, AuthSwitchRequest, CommonOkPacket, HandshakePacket, HandshakeResponse,
        OkPacket, OkPacketDeserializer, OldEofPacket, SslRequest, StmtPacket,
    },
    proto::{MyDeserialize, MySerialize},
};
pub use mysql_common::{
    constants::{ColumnFlags, ColumnType, StatusFlags},
    value::Value,
};
use std::fmt;
mod codec;
mod wire;
mod zlib;
use codec::PacketCodec;
pub mod pool;
pub mod types;
pub use wire::{Column, ColumnTypeInfo, RawValue, Row, ServerError, error_code};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod host_time;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use host_time::Instant;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant;

pub type Token = u64;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Protocol(&'static str),
    State(&'static str),
    Limit,
    Timeout,
    Transport,
    Cancelled,
    LocalInfileDisabled,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(s) | Self::State(s) => f.write_str(s),
            Self::Limit => f.write_str("protocol buffer limit exceeded"),
            Self::Timeout => f.write_str("connect ETIMEDOUT"),
            Self::Transport => f.write_str("Connection lost: The server closed the connection."),
            Self::Cancelled => f.write_str("Connection closed"),
            Self::LocalInfileDisabled => f.write_str("LOAD DATA LOCAL INFILE is disabled"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Protocol("invalid MySQL packet")
    }
}
#[derive(Clone)]
pub struct Config {
    pub user: String,
    pub password: Vec<u8>,
    pub database: Option<String>,
    pub tls: bool,
    pub compression: bool,
    pub multiple_statements: bool,
    pub local_infile: bool,
    pub max_buffer: usize,
    pub max_columns: usize,
    pub connect_deadline: Option<Instant>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            user: "root".into(),
            password: Vec::new(),
            database: None,
            tls: false,
            compression: false,
            multiple_statements: false,
            local_infile: false,
            max_buffer: 64 * 1024 * 1024,
            max_columns: 4096,
            connect_deadline: None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    ServerError,
    Aborted(Error),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Statement {
    pub id: u32,
    pub parameters: u16,
    pub columns: u16,
    pub warnings: u16,
}
#[derive(Debug)]
pub enum Event<'a> {
    Progress,
    UpgradeTls,
    Connected {
        connection_id: u32,
    },
    /// Supply 20 fresh random bytes to rsa_seed; no entropy is read by the core.
    RsaSeedNeeded,
    AuthFastSuccess,
    AuthFull,
    ColumnCount {
        token: Token,
        count: usize,
    },
    Column {
        token: Token,
        column: Column<'a>,
        parameter: bool,
    },
    Row {
        token: Token,
        row: Row<'a>,
    },
    Ok {
        token: Token,
        packet: OkPacket<'a>,
    },
    Prepared {
        token: Token,
        statement: Statement,
    },
    Error {
        token: Option<Token>,
        error: ServerError<'a>,
    },
    LocalInfile {
        token: Token,
        file_name: &'a [u8],
    },
    Completed {
        token: Token,
        outcome: Outcome,
    },
    Closed {
        reason: Error,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Handshake,
    Tls,
    Auth,
    Key,
    Seed,
    Ready,
    Header,
    Columns(usize),
    ColumnsEnd,
    Rows,
    Prepare,
    Parameters(usize),
    ParametersEnd,
    PrepareColumns(usize),
    PrepareEnd,
    Infile,
    NoResponse,
    Closing,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Query,
    Prepare,
    Execute,
    Reset,
    ChangeUser,
    Other,
}
struct Pending {
    token: Token,
    deadline: Option<Instant>,
    kind: CommandKind,
}
pub struct Connection {
    packet_ready: bool,
    config: Config,
    state: State,
    codec: PacketCodec,
    input: BytesMut,
    packet: Vec<u8>,
    scratch: Vec<u8>,
    output: BytesMut,
    output_at: usize,
    caps: Caps,
    tls: bool,
    plugin: AuthPlugin<'static>,
    nonce: [u8; 20],
    connection_id: u32,
    server_version: (u16, u16, u16),
    pending: Option<Pending>,
    completion: Option<Outcome>,
    columns: Vec<ColumnTypeInfo>,
    binary: bool,
    statement: Option<Statement>,
    statements: Vec<Statement>,
    status: StatusFlags,
    reason: Error,
}
impl Connection {
    pub fn new(config: Config) -> Result<Self> {
        if config.max_buffer < 1024
            || config.max_buffer > u32::MAX as usize
            || config.max_columns == 0
        {
            return Err(Error::Limit);
        }
        if config.user.contains('\0')
            || config.database.as_ref().is_some_and(|s| s.contains('\0'))
            || config.password.contains(&0)
        {
            return Err(Error::State("NUL in credentials"));
        }
        let mut codec = PacketCodec::default();
        codec.max_allowed_packet = config.max_buffer;
        Ok(Self {
            packet_ready: false,
            config,
            state: State::Handshake,
            codec,
            input: BytesMut::with_capacity(8192),
            packet: Vec::with_capacity(8192),
            scratch: Vec::with_capacity(8192),
            output: BytesMut::with_capacity(8192),
            output_at: 0,
            caps: Caps::empty(),
            tls: false,
            plugin: AuthPlugin::CachingSha2Password,
            nonce: [0; 20],
            connection_id: 0,
            server_version: (8, 0, 0),
            pending: None,
            completion: None,
            columns: Vec::with_capacity(16),
            binary: false,
            statement: None,
            statements: Vec::new(),
            status: StatusFlags::empty(),
            reason: Error::Transport,
        })
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
    pub fn receive(&mut self, b: &[u8]) -> Result<()> {
        if matches!(
            self.state,
            State::Tls | State::Seed | State::Closing | State::Closed
        ) {
            return Err(Error::State("cannot receive plaintext now"));
        }
        if b.len()
            > self.config.max_buffer.saturating_sub(
                self.input.len()
                    + if self.packet_ready {
                        0
                    } else {
                        self.packet.len()
                    },
            )
        {
            self.abort(Error::Limit);
            return Err(Error::Limit);
        }
        self.input.extend_from_slice(b);
        Ok(())
    }
    fn send(&mut self) -> Result<()> {
        if self.scratch.len() > self.config.max_buffer
            || self.output.len().saturating_add(self.scratch.len() + 32) > self.config.max_buffer
        {
            return Err(Error::Limit);
        }
        self.codec
            .encode(&mut &self.scratch[..], &mut self.output)
            .map_err(|_| Error::Protocol("packet encoding failed"))?;
        self.scratch.clear();
        Ok(())
    }
    fn scramble(&mut self) -> Result<()> {
        match self.plugin {
            AuthPlugin::CachingSha2Password => {
                if let Some(b) = scramble_sha256(&self.nonce, &self.config.password) {
                    self.scratch.extend_from_slice(&b);
                }
            }
            AuthPlugin::MysqlNativePassword => {
                if let Some(b) = scramble_native(&self.nonce, &self.config.password) {
                    self.scratch.extend_from_slice(&b);
                }
            }
            _ => return Err(Error::Protocol("unsupported authentication plugin")),
        }
        Ok(())
    }
    fn handshake_response(&mut self) -> Result<()> {
        self.scratch.clear();
        self.scramble()?;
        // Connection-time allocation only; the upstream packet borrows the auth bytes.
        let response = HandshakeResponse::new(
            Some(&self.scratch[..]),
            self.server_version,
            Some(self.config.user.as_bytes()),
            self.config.database.as_deref().map(str::as_bytes),
            Some(self.plugin.borrow()),
            self.caps,
            None,
            self.config.max_buffer as u32,
        );
        let mut bytes = Vec::new();
        response.serialize(&mut bytes);
        self.scratch.clear();
        self.scratch.extend_from_slice(&bytes);
        self.send()?;
        self.state = State::Auth;
        Ok(())
    }
    pub fn tls_established(&mut self) -> Result<()> {
        if self.state != State::Tls || !self.output().is_empty() {
            return Err(Error::State("TLS transition not ready"));
        }
        self.tls = true;
        self.handshake_response()
    }
    /// RSA OAEP SHA1 consumes this one-use, host-generated cryptographic seed.
    pub fn rsa_seed(&mut self, seed: [u8; 20]) -> Result<()> {
        use mysql_common::crypto::rsa::{Pkcs1OaepPadding, PublicKey, Rng};
        struct Seed(Option<[u8; 20]>);
        impl Rng for Seed {
            type Error = Error;
            fn fill(&mut self, b: &mut [u8]) -> Result<()> {
                if b.len() != 20 {
                    return Err(Error::Protocol("unexpected OAEP entropy request"));
                }
                b.copy_from_slice(&self.0.take().ok_or(Error::State("seed consumed"))?);
                Ok(())
            }
        }
        if self.state != State::Seed {
            return Err(Error::State("RSA entropy not requested"));
        }
        let key = PublicKey::from_pem(&self.packet[1..])
            .map_err(|_| Error::Protocol("invalid server RSA public key"))?;
        if !(128..=1024).contains(&key.num_octets()) {
            return Err(Error::Protocol("invalid RSA modulus size"));
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.config.password);
        self.scratch.push(0);
        for (i, b) in self.scratch.iter_mut().enumerate() {
            *b ^= self.nonce[i % 20];
        }
        // Guard upstream OAEP's length arithmetic before calling it.
        if self.scratch.len() > key.num_octets() - 42 {
            return Err(Error::Protocol("password too long for RSA key"));
        }
        let encrypted = key
            .encrypt_block(&self.scratch, Pkcs1OaepPadding::new(Seed(Some(seed))))
            .map_err(|_| Error::Protocol("RSA encryption failed"))?;
        self.scratch.clear();
        self.scratch.extend_from_slice(&encrypted);
        self.send()?;
        self.packet.clear();
        self.state = State::Auth;
        Ok(())
    }
    /// Whether a command submitted now would be admitted. This is the exact rule
    /// every command method applies: the session is authenticated and idle, the
    /// previous command's `Completed` has been delivered and all output has been
    /// acknowledged. A host queue can gate submission on it instead of copying
    /// the preconditions. Unlike `is_ready`, it also requires flushed output.
    pub fn can_accept(&self) -> bool {
        self.state == State::Ready && self.pending.is_none() && self.output().is_empty()
    }
    fn accept(&self) -> Result<()> {
        if !self.can_accept() {
            Err(Error::State("connection busy or closed"))
        } else {
            Ok(())
        }
    }
    fn command(
        &mut self,
        token: Token,
        kind: CommandKind,
        state: State,
        deadline: Option<Instant>,
    ) -> Result<()> {
        self.codec.reset_seq_id();
        self.send()?;
        self.pending = Some(Pending {
            token,
            deadline,
            kind,
        });
        self.state = state;
        Ok(())
    }
    pub fn query(&mut self, token: Token, sql: &str, deadline: Option<Instant>) -> Result<()> {
        self.accept()?;
        if sql.len().saturating_add(33) > self.config.max_buffer {
            return Err(Error::Limit);
        }

        self.scratch.clear();
        self.scratch.push(3);
        self.scratch.extend_from_slice(sql.as_bytes());
        self.binary = false;
        self.command(token, CommandKind::Query, State::Header, deadline)
    }
    pub fn prepare(&mut self, token: Token, sql: &str, deadline: Option<Instant>) -> Result<()> {
        self.accept()?;
        if sql.len().saturating_add(33) > self.config.max_buffer {
            return Err(Error::Limit);
        }

        self.scratch.clear();
        self.scratch.push(0x16);
        self.scratch.extend_from_slice(sql.as_bytes());
        self.command(token, CommandKind::Prepare, State::Prepare, deadline)
    }
    pub fn execute(
        &mut self,
        token: Token,
        id: u32,
        params: &[Value],
        deadline: Option<Instant>,
    ) -> Result<()> {
        self.accept()?;
        let stmt = self
            .statements
            .iter()
            .find(|s| s.id == id)
            .ok_or(Error::State("unknown prepared statement"))?;
        if params.len() != stmt.parameters as usize {
            return Err(Error::State("Incorrect arguments to mysqld_stmt_execute"));
        }
        let size = params
            .iter()
            .fold(16u64.saturating_add(params.len() as u64 * 3), |n, p| {
                n.saturating_add(p.bin_len())
            });
        if size > self.config.max_buffer.saturating_sub(32) as u64 {
            return Err(Error::Limit);
        }
        self.scratch.clear();
        self.scratch.push(0x17);
        self.scratch.extend_from_slice(&id.to_le_bytes());
        self.scratch.push(0);
        self.scratch.extend_from_slice(&1u32.to_le_bytes());
        if !params.is_empty() {
            let start = self.scratch.len();
            self.scratch.resize(start + params.len().div_ceil(8), 0);
            for (i, p) in params.iter().enumerate() {
                if matches!(p, Value::NULL) {
                    self.scratch[start + i / 8] |= 1 << (i % 8);
                }
            }
            self.scratch.push(1);
            for p in params {
                let (t, flags) = match p {
                    Value::NULL => (6, 0),
                    Value::Bytes(_) => (253, 0),
                    Value::Int(_) => (8, 0),
                    Value::UInt(_) => (8, 128),
                    Value::Float(_) => (4, 0),
                    Value::Double(_) => (5, 0),
                    Value::Date(..) => (12, 0),
                    Value::Time(..) => (11, 0),
                };
                self.scratch.extend_from_slice(&[t, flags]);
            }
            for p in params {
                p.serialize(&mut self.scratch);
            }
        }
        self.binary = true;
        self.command(token, CommandKind::Execute, State::Header, deadline)
    }
    fn simple(
        &mut self,
        token: Token,
        code: u8,
        id: Option<u32>,
        kind: CommandKind,
        state: State,
    ) -> Result<()> {
        self.accept()?;
        self.scratch.clear();
        self.scratch.push(code);
        if let Some(id) = id {
            if !self.statements.iter().any(|s| s.id == id) {
                return Err(Error::State("unknown prepared statement"));
            }
            self.scratch.extend_from_slice(&id.to_le_bytes());
        }
        self.command(token, kind, state, None)
    }
    pub fn ping(&mut self, token: Token) -> Result<()> {
        self.simple(token, 0x0e, None, CommandKind::Other, State::Header)
    }
    pub fn reset_connection(&mut self, token: Token) -> Result<()> {
        self.simple(token, 0x1f, None, CommandKind::Reset, State::Header)
    }
    pub fn reset_statement(&mut self, token: Token, id: u32) -> Result<()> {
        self.simple(token, 0x1a, Some(id), CommandKind::Other, State::Header)
    }
    pub fn close_statement(&mut self, token: Token, id: u32) -> Result<()> {
        self.simple(token, 0x19, Some(id), CommandKind::Other, State::NoResponse)?;
        self.statements.retain(|s| s.id != id);
        Ok(())
    }
    pub fn change_user(
        &mut self,
        token: Token,
        user: String,
        password: Vec<u8>,
        database: Option<String>,
    ) -> Result<()> {
        use mysql_common::packets::{ComChangeUser, ComChangeUserMoreData};
        self.accept()?;
        if user.contains('\0') || database.as_ref().is_some_and(|s| s.contains('\0')) {
            return Err(Error::State("NUL in credentials"));
        }
        self.config.user = user;
        self.config.password = password;
        self.config.database = database;
        self.scratch.clear();
        self.scramble()?;
        let more = ComChangeUserMoreData::new(45).with_auth_plugin(Some(self.plugin.borrow()));
        let command = ComChangeUser::new()
            .with_user(Some(self.config.user.as_bytes()))
            .with_database(self.config.database.as_deref().map(str::as_bytes))
            .with_auth_plugin_data(Some(&self.scratch[..]))
            .with_more_data(Some(more));
        let mut bytes = Vec::new();
        command.serialize(&mut bytes);
        self.scratch.clear();
        self.scratch.extend_from_slice(&bytes);
        self.command(token, CommandKind::ChangeUser, State::Auth, None)
    }
    pub fn local_infile_data(&mut self, data: &[u8]) -> Result<()> {
        if self.state != State::Infile || data.is_empty() {
            return Err(Error::State("LOCAL INFILE not active or empty data"));
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(data);
        self.send()
    }
    pub fn local_infile_finish(&mut self) -> Result<()> {
        if self.state != State::Infile {
            return Err(Error::State("LOCAL INFILE not active"));
        }
        self.scratch.clear();
        self.send()?;
        self.state = State::Header;
        Ok(())
    }
    pub fn is_ready(&self) -> bool {
        self.state == State::Ready && self.pending.is_none()
    }
    pub fn status(&self) -> StatusFlags {
        self.status
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        if matches!(self.state, State::Closing | State::Closed) {
            None
        } else if let Some(p) = &self.pending {
            p.deadline
        } else if !self.is_ready() {
            self.config.connect_deadline
        } else {
            None
        }
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.next_timeout().is_some_and(|t| t <= now) {
            self.abort(Error::Timeout);
        }
    }
    pub fn abort(&mut self, reason: Error) {
        if matches!(self.state, State::Closed | State::Closing) {
            return;
        }
        self.reason = reason;
        self.state = State::Closing;
        self.output.clear();
        self.output_at = 0;
        if self.pending.is_some() {
            self.completion = Some(Outcome::Aborted(reason));
        }
    }
    pub fn quit(&mut self) -> Result<()> {
        self.accept()?;
        self.scratch.clear();
        self.scratch.push(1);
        self.codec.reset_seq_id();
        self.send()?;
        self.state = State::Closing;
        self.reason = Error::Cancelled;
        Ok(())
    }
    fn token(&self) -> Result<Token> {
        self.pending
            .as_ref()
            .map(|p| p.token)
            .ok_or(Error::Protocol("unsolicited result"))
    }
    fn complete(&mut self, outcome: Outcome) {
        self.completion = Some(outcome);
        self.state = State::Ready;
    }
    pub fn next_event(&mut self) -> Result<Option<Event<'_>>> {
        if self.state == State::NoResponse && self.output().is_empty() {
            self.complete(Outcome::Success);
        }
        if let Some(outcome) = self.completion.take() {
            let p = self
                .pending
                .take()
                .ok_or(Error::Protocol("completion without command"))?;
            return Ok(Some(Event::Completed {
                token: p.token,
                outcome,
            }));
        }
        if self.state == State::Closing {
            if !self.output().is_empty() {
                return Ok(None);
            }
            self.state = State::Closed;
            return Ok(Some(Event::Closed {
                reason: self.reason,
            }));
        }
        if matches!(
            self.state,
            State::Closed | State::Tls | State::Seed | State::Infile | State::NoResponse
        ) {
            return Ok(None);
        }
        if self.packet_ready {
            self.packet.clear();
            self.packet_ready = false;
        }
        if !self
            .codec
            .decode(&mut self.input, &mut self.packet)
            .map_err(|_| Error::Protocol("invalid packet framing or sequence"))?
        {
            return Ok(None);
        }
        self.packet_ready = true;
        if self.packet.is_empty() {
            return Err(Error::Protocol("empty server packet"));
        }
        if self.packet[0] == 0xff {
            let token = self.pending.as_ref().map(|p| p.token);
            if token.is_some() {
                self.complete(Outcome::ServerError);
            } else {
                self.state = State::Closing;
            }
            return Ok(Some(Event::Error {
                token,
                error: ServerError::parse(&self.packet)?,
            }));
        }
        match self.state {
            State::Handshake => {
                let h = HandshakePacket::deserialize((), &mut ParseBuf(&self.packet))?;
                if h.protocol_version() != 10 {
                    return Err(Error::Protocol("unsupported handshake version"));
                }
                self.connection_id = h.connection_id();
                self.server_version = h.server_version_parsed().unwrap_or((8, 0, 0));
                self.nonce = h
                    .nonce()
                    .try_into()
                    .map_err(|_| Error::Protocol("invalid auth nonce"))?;
                self.plugin = h
                    .auth_plugin()
                    .unwrap_or(AuthPlugin::MysqlNativePassword)
                    .into_owned();
                let required = Caps::CLIENT_PROTOCOL_41
                    | Caps::CLIENT_SECURE_CONNECTION
                    | Caps::CLIENT_PLUGIN_AUTH;
                if !h.capabilities().contains(required) {
                    return Err(Error::Protocol("server lacks required capabilities"));
                }
                let mut caps = required
                    | Caps::CLIENT_LONG_PASSWORD
                    | Caps::CLIENT_LONG_FLAG
                    | Caps::CLIENT_TRANSACTIONS
                    | Caps::CLIENT_MULTI_RESULTS
                    | Caps::CLIENT_PS_MULTI_RESULTS
                    | Caps::CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA;
                if self.config.database.is_some() {
                    caps |= Caps::CLIENT_CONNECT_WITH_DB;
                }
                if self.config.multiple_statements {
                    caps |= Caps::CLIENT_MULTI_STATEMENTS;
                }
                if self.config.local_infile {
                    caps |= Caps::CLIENT_LOCAL_FILES;
                }
                if self.config.compression {
                    caps |= Caps::CLIENT_COMPRESS;
                }
                if self.config.tls {
                    if !h.capabilities().contains(Caps::CLIENT_SSL) {
                        return Err(Error::Protocol("Server does not support secure connection"));
                    }
                    caps |= Caps::CLIENT_SSL;
                }
                self.caps = caps & h.capabilities();
                if self.config.tls {
                    if !self.input.is_empty() {
                        return Err(Error::Protocol("plaintext after TLS handshake offer"));
                    }
                    self.scratch.clear();
                    SslRequest::new(self.caps, self.config.max_buffer as u32, 45)
                        .serialize(&mut self.scratch);
                    self.send()?;
                    self.state = State::Tls;
                    return Ok(Some(Event::UpgradeTls));
                }
                self.handshake_response()?;
            }
            State::Auth | State::Key => match self.packet[0] {
                0 => {
                    let ok = parse_ok(&self.packet, self.caps)?;
                    self.status = ok.status_flags();
                    if self
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.kind == CommandKind::ChangeUser)
                    {
                        self.statements.clear();
                        self.complete(Outcome::Success);
                    } else {
                        if self.caps.contains(Caps::CLIENT_COMPRESS) {
                            self.codec.compress(flate2::Compression::default());
                        }
                        self.state = State::Ready;
                        return Ok(Some(Event::Connected {
                            connection_id: self.connection_id,
                        }));
                    }
                }
                0xfe => {
                    let switch = AuthSwitchRequest::deserialize((), &mut ParseBuf(&self.packet))?;
                    self.plugin = switch.auth_plugin().into_owned();
                    self.nonce = switch
                        .plugin_data()
                        .try_into()
                        .map_err(|_| Error::Protocol("invalid auth-switch nonce"))?;
                    self.scratch.clear();
                    self.scramble()?;
                    self.send()?;
                    self.state = State::Auth;
                }
                1 if self.state == State::Key => {
                    self.state = State::Seed;
                    return Ok(Some(Event::RsaSeedNeeded));
                }
                1 if matches!(self.plugin, AuthPlugin::CachingSha2Password) => {
                    match self.packet.as_slice() {
                        [1, 3] => return Ok(Some(Event::AuthFastSuccess)),
                        [1, 4] => {
                            self.scratch.clear();
                            if self.tls {
                                self.scratch.extend_from_slice(&self.config.password);
                                self.scratch.push(0);
                            } else {
                                self.scratch.push(2);
                                self.state = State::Key;
                            }
                            self.send()?;
                            return Ok(Some(Event::AuthFull));
                        }
                        _ => return Err(Error::Protocol("invalid caching_sha2 challenge")),
                    }
                }
                _ => return Err(Error::Protocol("unexpected authentication packet")),
            },
            State::Header => {
                let token = self.token()?;
                match self.packet[0] {
                    0 => {
                        let ok = parse_ok(&self.packet, self.caps)?;
                        self.status = ok.status_flags();
                        if !self
                            .status
                            .contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS)
                        {
                            if self
                                .pending
                                .as_ref()
                                .is_some_and(|p| p.kind == CommandKind::Reset)
                            {
                                self.statements.clear();
                            }
                            self.complete(Outcome::Success);
                        }
                        return Ok(Some(Event::Ok {
                            token,
                            packet: parse_ok(&self.packet, self.caps)?,
                        }));
                    }
                    0xfb => {
                        if !self.config.local_infile {
                            self.abort(Error::LocalInfileDisabled);
                            return Err(Error::LocalInfileDisabled);
                        }
                        self.state = State::Infile;
                        return Ok(Some(Event::LocalInfile {
                            token,
                            file_name: &self.packet[1..],
                        }));
                    }
                    _ => {
                        let mut c = wire::Cursor(&self.packet);
                        let count = usize::try_from(c.lenenc()?).map_err(|_| Error::Limit)?;
                        c.end()?;
                        if count == 0 || count > self.config.max_columns {
                            return Err(Error::Limit);
                        }
                        self.columns.clear();
                        self.state = State::Columns(count);
                        return Ok(Some(Event::ColumnCount { token, count }));
                    }
                }
            }
            State::Columns(left) | State::Parameters(left) | State::PrepareColumns(left) => {
                let column = Column::parse(&self.packet)?;
                let parameter = matches!(self.state, State::Parameters(_));
                match self.state {
                    State::Columns(_) => {
                        self.columns.push(column.type_info);
                        self.state = if left == 1 {
                            State::ColumnsEnd
                        } else {
                            State::Columns(left - 1)
                        };
                    }
                    State::Parameters(_) => {
                        self.state = if left == 1 {
                            State::ParametersEnd
                        } else {
                            State::Parameters(left - 1)
                        }
                    }
                    _ => {
                        self.state = if left == 1 {
                            State::PrepareEnd
                        } else {
                            State::PrepareColumns(left - 1)
                        }
                    }
                }
                return Ok(Some(Event::Column {
                    token: self.token()?,
                    column,
                    parameter,
                }));
            }
            State::ColumnsEnd => {
                self.status = parse_eof(&self.packet, self.caps)?.status_flags();
                self.state = State::Rows;
            }
            State::Rows => {
                if self.packet[0] == 0xfe && self.packet.len() < 9 {
                    let ok = parse_eof(&self.packet, self.caps)?;
                    self.status = ok.status_flags();
                    if self
                        .status
                        .contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS)
                    {
                        self.state = State::Header;
                    } else {
                        self.complete(Outcome::Success);
                    }
                    return Ok(Some(Event::Ok {
                        token: self.token()?,
                        packet: parse_eof(&self.packet, self.caps)?,
                    }));
                }
                return Ok(Some(Event::Row {
                    token: self.token()?,
                    row: Row::parse(&self.packet, &self.columns, self.binary)?,
                }));
            }
            State::Prepare => {
                let s = StmtPacket::deserialize((), &mut ParseBuf(&self.packet))?;
                let stmt = Statement {
                    id: s.statement_id(),
                    parameters: s.num_params(),
                    columns: s.num_columns(),
                    warnings: s.warning_count(),
                };
                if stmt.parameters as usize > self.config.max_columns
                    || stmt.columns as usize > self.config.max_columns
                {
                    return Err(Error::Limit);
                }
                self.statement = Some(stmt);
                if stmt.parameters > 0 {
                    self.state = State::Parameters(stmt.parameters as usize);
                } else if stmt.columns > 0 {
                    self.state = State::PrepareColumns(stmt.columns as usize);
                } else {
                    return self.prepared();
                }
            }
            State::ParametersEnd => {
                parse_eof(&self.packet, self.caps)?;
                let stmt = self.statement.unwrap();
                if stmt.columns > 0 {
                    self.state = State::PrepareColumns(stmt.columns as usize);
                } else {
                    return self.prepared();
                }
            }
            State::PrepareEnd => {
                parse_eof(&self.packet, self.caps)?;
                return self.prepared();
            }
            _ => return Err(Error::Protocol("unexpected packet in current state")),
        }
        // A consumed control packet may produce output without an event. The host
        // must flush output and continue polling while buffered input remains.
        Ok(Some(Event::Progress))
    }
    fn prepared(&mut self) -> Result<Option<Event<'_>>> {
        let statement = self
            .statement
            .take()
            .ok_or(Error::Protocol("missing prepare state"))?;
        self.statements.push(statement);
        self.complete(Outcome::Success);
        Ok(Some(Event::Prepared {
            token: self.token()?,
            statement,
        }))
    }
}
fn parse_ok(b: &[u8], caps: Caps) -> Result<OkPacket<'_>> {
    Ok(OkPacketDeserializer::<CommonOkPacket>::deserialize(caps, &mut ParseBuf(b))?.into_inner())
}
fn parse_eof(b: &[u8], caps: Caps) -> Result<OkPacket<'_>> {
    if b.len() != 5 || b[0] != 0xfe {
        return Err(Error::Protocol("expected legacy EOF"));
    }
    Ok(OkPacketDeserializer::<OldEofPacket>::deserialize(caps, &mut ParseBuf(b))?.into_inner())
}

#[cfg(feature = "turnloop")]
pub mod asynchronous;
