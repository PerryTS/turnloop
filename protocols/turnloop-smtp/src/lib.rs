#![deny(unsafe_op_in_unsafe_fn)]
//! Pull-driven SMTP with explicit TLS transitions. Socket, DNS, certificate
//! policy and clock ownership remain in the host. `Ready` also completes verify.
pub mod message;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    io::Write,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tls {
    None,
    Opportunistic,
    Required,
    Implicit,
}
#[derive(Clone)]
pub enum Auth {
    Plain { user: String, password: String },
    Login { user: String, password: String },
    Xoauth2 { user: String, access_token: String },
}
#[derive(Clone)]
pub struct Config {
    pub name: String,
    pub tls: Tls,
    pub auth: Option<Auth>,
    pub greeting_timeout: Duration,
    pub socket_timeout: Duration,
    pub max_response_bytes: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            name: "[127.0.0.1]".into(),
            tls: Tls::Opportunistic,
            auth: None,
            greeting_timeout: Duration::from_secs(30),
            socket_timeout: Duration::from_secs(600),
            max_response_bytes: 64 * 1024,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub starttls: bool,
    pub pipelining: bool,
    pub size: Option<u64>,
    pub size_supported: bool,
    pub eight_bit_mime: bool,
    pub smtp_utf8: bool,
    pub auth_plain: bool,
    pub auth_login: bool,
    pub auth_xoauth2: bool,
}
impl Capabilities {
    fn parse(&mut self, response: &str) {
        *self = Self::default();
        // First EHLO line is the server greeting, not an extension.
        for line in response.lines().skip(1) {
            let Some(text) = line.get(4..) else {
                continue;
            };
            let mut words = text.split_ascii_whitespace();
            let Some(name) = words.next() else {
                continue;
            };
            if name.eq_ignore_ascii_case("STARTTLS") {
                self.starttls = true;
            } else if name.eq_ignore_ascii_case("PIPELINING") {
                self.pipelining = true;
            } else if name.eq_ignore_ascii_case("8BITMIME") {
                self.eight_bit_mime = true;
            } else if name.eq_ignore_ascii_case("SMTPUTF8") {
                self.smtp_utf8 = true;
            } else if name.eq_ignore_ascii_case("SIZE") {
                self.size_supported = true;
                self.size = words.next().and_then(|v| v.parse().ok());
            } else if name.eq_ignore_ascii_case("AUTH")
                || name.to_ascii_uppercase().starts_with("AUTH=")
            {
                for word in name.get(5..).into_iter().chain(words) {
                    if word.eq_ignore_ascii_case("PLAIN") {
                        self.auth_plain = true;
                    }
                    if word.eq_ignore_ascii_case("LOGIN") {
                        self.auth_login = true;
                    }
                    if word.eq_ignore_ascii_case("XOAUTH2") {
                        self.auth_xoauth2 = true;
                    }
                }
            }
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub from: String,
    pub to: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub response: String,
    pub response_code: Option<u16>,
    pub command: &'static str,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
fn error(
    code: &'static str,
    command: &'static str,
    message: &str,
    response_code: Option<u16>,
    response: &str,
) -> Error {
    Error {
        code,
        command,
        message: if response.is_empty() {
            message.into()
        } else {
            format!("{message}: {response}")
        },
        response_code,
        response: response.into(),
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub recipient: String,
    pub error: Error,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendInfo {
    pub response: String,
    pub response_code: u16,
    pub accepted: Vec<String>,
    pub rejected: Vec<Rejection>,
    pub envelope: Envelope,
    pub message_id: String,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    UpgradeTls,
    Ready,
    Sent {
        token: u64,
        info: SendInfo,
    },
    Failed {
        token: Option<u64>,
        error: Error,
        envelope: Option<Envelope>,
        accepted: Vec<String>,
        rejected: Vec<Rejection>,
    },
    Reset,
    CloseTransport,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Tls,
    Greeting,
    Ehlo,
    Helo,
    StartTls,
    Auth,
    LoginUser,
    LoginPassword,
    OAuthFailure,
    Ready,
    Mail,
    Recipients,
    Data,
    Body,
    Reset,
    Quit,
    Closed,
}
pub struct Connection {
    config: Config,
    state: State,
    secure: bool,
    implicit_pending: bool,
    capabilities: Capabilities,
    tx: Vec<u8>,
    tx_pos: usize,
    rx: Vec<u8>,
    response: String,
    response_code: Option<u16>,
    auth_raw: Vec<u8>,
    auth_encoded: String,
    body: Vec<u8>,
    events: VecDeque<Event>,
    deadline: Option<Instant>,
    token: Option<u64>,
    envelope: Option<Envelope>,
    message_id: String,
    accepted: Vec<String>,
    rejected: Vec<Rejection>,
    recipient_index: usize,
    mail_error: Option<Error>,
    automatic_reset: bool,
}
impl Connection {
    pub fn new(config: Config) -> Result<Self, Error> {
        if config.name.is_empty() || config.name.bytes().any(|b| b <= b' ' || b == 127) {
            return Err(error("EINVAL", "EHLO", "Invalid client hostname", None, ""));
        }
        Ok(Self {
            config,
            state: State::Disconnected,
            secure: false,
            implicit_pending: false,
            capabilities: Capabilities::default(),
            tx: Vec::with_capacity(4096),
            tx_pos: 0,
            rx: Vec::with_capacity(4096),
            response: String::with_capacity(1024),
            response_code: None,
            auth_raw: Vec::with_capacity(256),
            auth_encoded: String::with_capacity(512),
            body: Vec::with_capacity(8192),
            events: VecDeque::with_capacity(16),
            deadline: None,
            token: None,
            envelope: None,
            message_id: String::new(),
            accepted: Vec::new(),
            rejected: Vec::new(),
            recipient_index: 0,
            mail_error: None,
            automatic_reset: false,
        })
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    pub fn output(&self) -> &[u8] {
        &self.tx[self.tx_pos..]
    }
    pub fn consume_output(&mut self, count: usize) {
        assert!(count <= self.output().len());
        self.tx_pos += count;
        if self.tx_pos == self.tx.len() {
            self.tx.clear();
            self.tx_pos = 0;
        }
    }
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
    pub fn connected(&mut self, now: Instant) -> Result<(), Error> {
        if self.state != State::Disconnected {
            return Err(error("ESTATE", "CONN", "Already connected", None, ""));
        }
        self.deadline = now.checked_add(self.config.greeting_timeout);
        if self.config.tls == Tls::Implicit {
            self.state = State::Tls;
            self.implicit_pending = true;
            self.events.push_back(Event::UpgradeTls);
        } else {
            self.state = State::Greeting;
        }
        Ok(())
    }
    pub fn tls_established(&mut self, now: Instant) -> Result<(), Error> {
        if self.state != State::Tls {
            return Err(error(
                "ESTATE",
                "STARTTLS",
                "Unexpected TLS completion",
                None,
                "",
            ));
        }
        self.secure = true;
        self.capabilities = Capabilities::default();
        if self.implicit_pending {
            self.implicit_pending = false;
            self.state = State::Greeting;
            self.deadline = now.checked_add(self.config.greeting_timeout);
        } else {
            self.ehlo();
            self.arm(now);
        }
        Ok(())
    }
    fn arm(&mut self, now: Instant) {
        self.deadline = now.checked_add(self.config.socket_timeout);
    }
    fn ehlo(&mut self) {
        self.state = State::Ehlo;
        write!(self.tx, "EHLO {}\r\n", self.config.name).unwrap();
    }
    pub fn receive(&mut self, bytes: &[u8], now: Instant) -> Result<(), Error> {
        if matches!(self.state, State::Disconnected | State::Tls | State::Closed) {
            return Err(error(
                "ESTATE",
                "CONN",
                "Bytes outside SMTP protocol state",
                None,
                "",
            ));
        }
        if self.state != State::Greeting {
            self.arm(now);
        }
        for chunk in bytes.chunks(4096) {
            self.rx.extend_from_slice(chunk);
            while let Some(end) = self.rx.iter().position(|b| *b == b'\n') {
                if end < 4
                    || self.rx[end - 1] != b'\r'
                    || !self.rx[..3].iter().all(u8::is_ascii_digit)
                    || !matches!(self.rx[3], b' ' | b'-')
                {
                    return self.protocol_error("Invalid SMTP response");
                }
                let code = u16::from(self.rx[0] - b'0') * 100
                    + u16::from(self.rx[1] - b'0') * 10
                    + u16::from(self.rx[2] - b'0');
                if !(200..600).contains(&code) || self.response_code.is_some_and(|c| c != code) {
                    return self.protocol_error("Inconsistent SMTP response code");
                }
                self.response_code = Some(code);
                if !self.response.is_empty() {
                    self.response.push('\n');
                }
                self.response
                    .push_str(&String::from_utf8_lossy(&self.rx[..end - 1]));
                let complete = self.rx[3] == b' ';
                self.rx.drain(..=end);
                if self.response.len() > self.config.max_response_bytes {
                    return self.protocol_error("SMTP response too large");
                }
                if complete {
                    self.response_code = None;
                    let mut response = std::mem::take(&mut self.response);
                    self.reply(code, &response);
                    if !matches!(self.state, State::Closed | State::Greeting) {
                        self.arm(now);
                    }
                    response.clear();
                    self.response = response;
                    if self.state == State::Tls && !self.rx.is_empty() {
                        return self.protocol_error("Unexpected plaintext after STARTTLS");
                    }
                    if self.state == State::Closed {
                        break;
                    }
                }
            }
            if self.rx.len() + self.response.len() > self.config.max_response_bytes {
                return self.protocol_error("SMTP response too large");
            }
        }
        Ok(())
    }
    fn protocol_error(&mut self, message: &str) -> Result<(), Error> {
        let e = error("EPROTOCOL", self.command_name(), message, None, "");
        self.fail(e.clone());
        self.close_transport();
        Err(e)
    }
    fn reply(&mut self, code: u16, response: &str) {
        if code == 421 {
            let e = error(
                "ECONNECTION",
                self.command_name(),
                "Server closed connection",
                Some(code),
                response,
            );
            self.fail(e);
            self.close_transport();
            return;
        }
        match self.state {
            State::Greeting if code == 220 => self.ehlo(),
            State::Ehlo if code == 250 => {
                self.capabilities.parse(response);
                self.after_ehlo();
            }
            State::Ehlo if matches!(code, 500 | 502 | 504) => {
                self.state = State::Helo;
                write!(self.tx, "HELO {}\r\n", self.config.name).unwrap();
            }
            State::Helo if code == 250 => {
                self.capabilities = Capabilities::default();
                self.after_ehlo();
            }
            State::StartTls if code == 220 => {
                self.state = State::Tls;
                self.events.push_back(Event::UpgradeTls);
            }
            State::Auth if code == 235 => self.ready(),
            State::Auth if code == 334 => match &self.config.auth {
                Some(Auth::Login { user, .. }) => {
                    self.auth_raw.clear();
                    self.auth_raw.extend_from_slice(user.as_bytes());
                    self.auth_line();
                    self.state = State::LoginUser;
                }
                Some(Auth::Plain { .. }) => {
                    self.auth_line();
                    self.state = State::LoginPassword;
                }
                Some(Auth::Xoauth2 { .. }) => {
                    self.tx.extend_from_slice(b"\r\n");
                    self.state = State::OAuthFailure;
                }
                None => self.unexpected(code, response),
            },
            State::LoginUser if code == 334 => {
                if let Some(Auth::Login { password, .. }) = &self.config.auth {
                    self.auth_raw.clear();
                    self.auth_raw.extend_from_slice(password.as_bytes());
                    self.auth_line();
                    self.state = State::LoginPassword;
                }
            }
            State::LoginPassword if code == 235 => self.ready(),
            State::Mail => {
                if code != 250 {
                    self.mail_error = Some(error(
                        "EENVELOPE",
                        "MAIL FROM",
                        "Mail command failed",
                        Some(code),
                        response,
                    ));
                }
                self.state = State::Recipients;
                if !self.capabilities.pipelining {
                    if let Some(e) = self.mail_error.take() {
                        self.fail_and_reset(e);
                    } else {
                        self.rcpt(0);
                    }
                }
            }
            State::Recipients => {
                let recipient = self.envelope.as_ref().unwrap().to[self.recipient_index].clone();
                if matches!(code, 250..=252) && self.mail_error.is_none() {
                    self.accepted.push(recipient);
                } else {
                    self.rejected.push(Rejection {
                        recipient,
                        error: error(
                            "EENVELOPE",
                            "RCPT TO",
                            "Recipient command failed",
                            Some(code),
                            response,
                        ),
                    });
                }
                self.recipient_index += 1;
                if self.recipient_index == self.envelope.as_ref().unwrap().to.len() {
                    if let Some(e) = self.mail_error.take() {
                        self.fail_and_reset(e);
                    } else if self.accepted.is_empty() {
                        self.fail_and_reset(error(
                            "EENVELOPE",
                            "RCPT TO",
                            "Can't send mail - all recipients were rejected",
                            Some(code),
                            response,
                        ));
                    } else {
                        self.tx.extend_from_slice(b"DATA\r\n");
                        self.state = State::Data;
                    }
                } else if !self.capabilities.pipelining {
                    self.rcpt(self.recipient_index);
                }
            }
            State::Data if code == 354 => {
                self.tx.extend_from_slice(&self.body);
                self.state = State::Body;
            }
            State::Body if code == 250 => {
                let token = self.token.take().unwrap();
                let info = SendInfo {
                    response: response.into(),
                    response_code: code,
                    accepted: std::mem::take(&mut self.accepted),
                    rejected: std::mem::take(&mut self.rejected),
                    envelope: self.envelope.take().unwrap(),
                    message_id: std::mem::take(&mut self.message_id),
                };
                self.events.push_back(Event::Sent { token, info });
                self.state = State::Ready;
            }
            State::Reset if code == 250 => {
                self.state = State::Ready;
                if self.automatic_reset {
                    self.automatic_reset = false;
                } else {
                    self.events.push_back(Event::Reset);
                }
            }
            State::Quit if code == 221 => self.close_transport(),
            _ => self.unexpected(code, response),
        }
    }
    fn after_ehlo(&mut self) {
        if !self.secure && self.config.tls != Tls::None && self.capabilities.starttls {
            self.tx.extend_from_slice(b"STARTTLS\r\n");
            self.state = State::StartTls;
        } else if !self.secure && self.config.tls == Tls::Required {
            self.fail(error(
                "ETLS",
                "STARTTLS",
                "Error upgrading connection with STARTTLS: server does not support STARTTLS",
                None,
                "",
            ));
            self.close_transport();
        } else {
            self.authenticate();
        }
    }
    fn authenticate(&mut self) {
        self.auth_raw.clear();
        match &self.config.auth {
            None => {
                self.ready();
                return;
            }
            Some(Auth::Plain { user, password }) if self.capabilities.auth_plain => {
                self.auth_raw.push(0);
                self.auth_raw.extend_from_slice(user.as_bytes());
                self.auth_raw.push(0);
                self.auth_raw.extend_from_slice(password.as_bytes());
                self.tx.extend_from_slice(b"AUTH PLAIN ");
                self.auth_line();
            }
            Some(Auth::Login { .. }) if self.capabilities.auth_login => {
                self.tx.extend_from_slice(b"AUTH LOGIN\r\n")
            }
            Some(Auth::Xoauth2 { user, access_token }) if self.capabilities.auth_xoauth2 => {
                write!(
                    self.auth_raw,
                    "user={user}\x01auth=Bearer {access_token}\x01\x01"
                )
                .unwrap();
                self.tx.extend_from_slice(b"AUTH XOAUTH2 ");
                self.auth_line();
            }
            _ => {
                self.fail(error(
                    "EAUTH",
                    "AUTH",
                    "No supported authentication method",
                    None,
                    "",
                ));
                self.close_transport();
                return;
            }
        }
        self.state = State::Auth;
    }
    fn auth_line(&mut self) {
        self.auth_encoded.clear();
        STANDARD.encode_string(&self.auth_raw, &mut self.auth_encoded);
        self.tx.extend_from_slice(self.auth_encoded.as_bytes());
        self.tx.extend_from_slice(b"\r\n");
    }
    fn ready(&mut self) {
        self.state = State::Ready;
        self.events.push_back(Event::Ready);
    }
    /// Move the result's inherent envelope/ID ownership into the core. `content`
    /// may be lettre's formatted MIME bytes or another already-built message.
    pub fn send(
        &mut self,
        token: u64,
        envelope: Envelope,
        message_id: String,
        content: &[u8],
        now: Instant,
    ) -> Result<(), Error> {
        if self.state != State::Ready {
            return Err(error(
                "ESTATE",
                "MAIL FROM",
                "SMTP connection is not ready",
                None,
                "",
            ));
        }
        let valid_address = |s: &str| {
            !s.bytes()
                .any(|b| b <= b' ' || matches!(b, b'<' | b'>' | 127))
        };
        if !valid_address(&envelope.from)
            || envelope.to.is_empty()
            || envelope
                .to
                .iter()
                .any(|s| s.is_empty() || !valid_address(s))
        {
            return Err(error(
                "EENVELOPE",
                "MAIL FROM",
                "Invalid envelope",
                None,
                "",
            ));
        }
        let utf8 = !envelope.from.is_ascii() || envelope.to.iter().any(|s| !s.is_ascii());
        let eight_bit = !content.is_ascii();
        if utf8 && !self.capabilities.smtp_utf8 {
            return Err(error(
                "EENVELOPE",
                "MAIL FROM",
                "SMTPUTF8 is required but not supported",
                None,
                "",
            ));
        }
        if eight_bit && !self.capabilities.eight_bit_mime {
            return Err(error(
                "EMESSAGE",
                "MAIL FROM",
                "8BITMIME is required but not supported",
                None,
                "",
            ));
        }
        self.body.clear();
        let size = encode_data(content, &mut self.body);
        if self
            .capabilities
            .size
            .is_some_and(|limit| limit != 0 && size as u64 > limit)
        {
            return Err(error(
                "EMESSAGE",
                "MAIL FROM",
                "Message exceeds server SIZE limit",
                None,
                "",
            ));
        }
        self.accepted.clear();
        self.rejected.clear();
        self.mail_error = None;
        self.recipient_index = 0;
        write!(self.tx, "MAIL FROM:<{}>", envelope.from).unwrap();
        if self.capabilities.size_supported {
            write!(self.tx, " SIZE={size}").unwrap();
        }
        if eight_bit {
            self.tx.extend_from_slice(b" BODY=8BITMIME");
        }
        if utf8 {
            self.tx.extend_from_slice(b" SMTPUTF8");
        }
        self.tx.extend_from_slice(b"\r\n");
        self.envelope = Some(envelope);
        self.message_id = message_id;
        self.token = Some(token);
        if self.capabilities.pipelining {
            for i in 0..self.envelope.as_ref().unwrap().to.len() {
                self.rcpt(i);
            }
        }
        self.state = State::Mail;
        self.arm(now);
        Ok(())
    }
    fn rcpt(&mut self, index: usize) {
        write!(
            self.tx,
            "RCPT TO:<{}>\r\n",
            self.envelope.as_ref().unwrap().to[index]
        )
        .unwrap();
    }
    fn unexpected(&mut self, code: u16, response: &str) {
        let auth = matches!(
            self.state,
            State::Auth | State::LoginUser | State::LoginPassword | State::OAuthFailure
        );
        let tls = self.state == State::StartTls;
        let body = self.state == State::Body;
        let data = self.state == State::Data;
        let e = error(
            if body {
                "EMESSAGE"
            } else if auth {
                "EAUTH"
            } else if tls {
                "ETLS"
            } else {
                "EENVELOPE"
            },
            self.command_name(),
            if body {
                "Message failed"
            } else if data {
                "Data command failed"
            } else if auth {
                "Invalid login"
            } else if tls {
                "Error upgrading connection with STARTTLS"
            } else {
                "Unexpected SMTP response"
            },
            Some(code),
            response,
        );
        if matches!(self.state, State::Data | State::Body) {
            self.fail_and_reset(e);
        } else {
            self.fail(e);
            self.close_transport();
        }
    }
    fn fail_and_reset(&mut self, e: Error) {
        self.fail(e);
        self.tx.extend_from_slice(b"RSET\r\n");
        self.state = State::Reset;
        self.automatic_reset = true;
    }
    fn fail(&mut self, e: Error) {
        self.events.push_back(Event::Failed {
            token: self.token.take(),
            error: e,
            envelope: self.envelope.take(),
            accepted: std::mem::take(&mut self.accepted),
            rejected: std::mem::take(&mut self.rejected),
        });
    }
    pub fn reset(&mut self, now: Instant) -> Result<(), Error> {
        if self.state != State::Ready {
            return Err(error(
                "ESTATE",
                "RSET",
                "SMTP connection is not ready",
                None,
                "",
            ));
        }
        self.tx.extend_from_slice(b"RSET\r\n");
        self.state = State::Reset;
        self.automatic_reset = false;
        self.arm(now);
        Ok(())
    }
    pub fn quit(&mut self, now: Instant) -> Result<(), Error> {
        if self.state != State::Ready {
            return Err(error(
                "ESTATE",
                "QUIT",
                "SMTP connection is not ready",
                None,
                "",
            ));
        }
        self.tx.extend_from_slice(b"QUIT\r\n");
        self.state = State::Quit;
        self.arm(now);
        Ok(())
    }
    fn command_name(&self) -> &'static str {
        match self.state {
            State::Ehlo => "EHLO",
            State::Helo => "HELO",
            State::StartTls | State::Tls => "STARTTLS",
            State::Auth | State::LoginUser | State::LoginPassword | State::OAuthFailure => {
                match self.config.auth {
                    Some(Auth::Plain { .. }) => "AUTH PLAIN",
                    Some(Auth::Login { .. }) => "AUTH LOGIN",
                    Some(Auth::Xoauth2 { .. }) => "AUTH XOAUTH2",
                    None => "AUTH",
                }
            }
            State::Mail => "MAIL FROM",
            State::Recipients => "RCPT TO",
            State::Data | State::Body => "DATA",
            State::Reset => "RSET",
            State::Quit => "QUIT",
            _ => "CONN",
        }
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.deadline.is_some_and(|d| d <= now) {
            self.fail(error(
                "ETIMEDOUT",
                self.command_name(),
                if self.state == State::Greeting {
                    "Greeting never received"
                } else {
                    "Timeout"
                },
                None,
                "",
            ));
            self.close_transport();
        }
    }
    pub fn transport_lost(&mut self) {
        if self.state != State::Closed {
            self.fail(error(
                "ECONNECTION",
                self.command_name(),
                "Connection closed unexpectedly",
                None,
                "",
            ));
            self.close_transport();
        }
    }
    pub fn close(&mut self) {
        if self.token.is_some() {
            self.fail(error(
                "ECANCELLED",
                self.command_name(),
                "Cancelled",
                None,
                "",
            ));
        }
        self.close_transport();
    }
    fn close_transport(&mut self) {
        if self.state == State::Closed {
            return;
        }
        self.state = State::Closed;
        self.deadline = None;
        self.tx.clear();
        self.tx_pos = 0;
        self.events.push_back(Event::CloseTransport);
        self.events.push_back(Event::Closed);
    }
}
/// Normalize LF, CRLF and bare CR, dot-stuff each line, append the SMTP DATA
/// terminator. Returns SIZE octets (normalized content, excluding transparency).
pub fn encode_data(content: &[u8], out: &mut Vec<u8>) -> usize {
    let mut start = true;
    let mut i = 0;
    let mut size = 0;
    while i < content.len() {
        match content[i] {
            b'\r' | b'\n' => {
                if content[i] == b'\r' && content.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                out.extend_from_slice(b"\r\n");
                size += 2;
                start = true;
            }
            byte => {
                if start && byte == b'.' {
                    out.push(b'.');
                }
                out.push(byte);
                size += 1;
                start = false;
            }
        }
        i += 1;
    }
    if !start {
        out.extend_from_slice(b"\r\n");
        size += 2;
    }
    out.extend_from_slice(b".\r\n");
    size
}
