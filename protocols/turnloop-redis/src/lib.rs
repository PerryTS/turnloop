#![deny(unsafe_op_in_unsafe_fn)]
//! Sans-I/O Redis connection. The host drains `output`, acknowledges writes with
//! `consume_output`, supplies bytes/time, and polls events. Never reads a clock.
//! Tokens are unique among outstanding commands. A timeout leaves a wire-order
//! tombstone until its reply arrives, preventing later replies being misassigned.
pub mod resp;
pub mod routing;
use resp::{Limits, Value, encode_command};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct Config {
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: u32,
    pub client_name: Option<String>,
    pub tls: bool,
    pub prefer_resp3: bool,
    pub offline_queue: bool,
    pub auto_resubscribe: bool,
    pub auto_resend_unfulfilled: bool,
    pub max_retries_per_request: Option<u32>,
    pub connect_timeout: Duration,
    pub limits: Limits,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            username: None,
            password: None,
            database: 0,
            client_name: None,
            tls: false,
            prefer_resp3: true,
            offline_queue: true,
            auto_resubscribe: true,
            auto_resend_unfulfilled: true,
            max_retries_per_request: Some(20),
            connect_timeout: Duration::from_secs(10),
            limits: Limits::default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub name: &'static str,
    pub message: String,
}
impl Error {
    fn new(message: &str) -> Self {
        Self {
            name: "Error",
            message: message.into(),
        }
    }
    fn reply(bytes: Vec<u8>) -> Self {
        Self {
            name: "ReplyError",
            message: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}
impl std::error::Error for Error {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Tls,
    Handshake,
    Ready,
    RetryDecision,
    WaitingRetry,
    Closed,
}
#[derive(Debug, PartialEq)]
pub enum Event {
    Connect,
    UpgradeTls,
    Ready {
        resp3: bool,
    },
    Reply {
        token: u64,
        result: Result<Value, Error>,
    },
    Message {
        pattern: Option<Vec<u8>>,
        channel: Vec<u8>,
        payload: Vec<u8>,
    },
    Push(Value),
    /// Evaluate retryStrategy(attempt) in the host, then call `retry`.
    Retry {
        attempt: u32,
    },
    Error(Error),
    CloseTransport,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Normal,
    Subscribe,
    Psubscribe,
    Unsubscribe,
    Punsubscribe,
    Quit,
}
impl Kind {
    fn of(name: &[u8]) -> Self {
        if name.eq_ignore_ascii_case(b"SUBSCRIBE") {
            Self::Subscribe
        } else if name.eq_ignore_ascii_case(b"PSUBSCRIBE") {
            Self::Psubscribe
        } else if name.eq_ignore_ascii_case(b"UNSUBSCRIBE") {
            Self::Unsubscribe
        } else if name.eq_ignore_ascii_case(b"PUNSUBSCRIBE") {
            Self::Punsubscribe
        } else if name.eq_ignore_ascii_case(b"QUIT") {
            Self::Quit
        } else {
            Self::Normal
        }
    }
    fn subscription(self) -> bool {
        matches!(
            self,
            Self::Subscribe | Self::Psubscribe | Self::Unsubscribe | Self::Punsubscribe
        )
    }
}
struct Pending {
    token: Option<u64>,
    wire: Vec<u8>,
    deadline: Option<Instant>,
    kind: Kind,
    acks: usize,
}
#[derive(Clone, Copy)]
enum Handshake {
    Hello,
    Auth,
    Select,
    Name,
    Restore,
}
pub struct Connection {
    config: Config,
    state: State,
    resp3: bool,
    handshake: Handshake,
    tx: Vec<u8>,
    tx_pos: usize,
    rx: Vec<u8>,
    events: VecDeque<Event>,
    pending: VecDeque<Pending>,
    offline: VecDeque<Pending>,
    pool: Vec<Vec<u8>>,
    channels: Vec<Vec<u8>>,
    patterns: Vec<Vec<u8>>,
    subscriber: bool,
    deadline: Option<Instant>,
    attempt: u32,
}
impl Connection {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: State::Disconnected,
            resp3: false,
            handshake: Handshake::Hello,
            tx: Vec::with_capacity(4096),
            tx_pos: 0,
            rx: Vec::with_capacity(4096),
            events: VecDeque::with_capacity(32),
            pending: VecDeque::with_capacity(32),
            offline: VecDeque::with_capacity(32),
            pool: Vec::with_capacity(32),
            channels: Vec::new(),
            patterns: Vec::new(),
            subscriber: false,
            deadline: None,
            attempt: 0,
        }
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
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
    pub fn connect(&mut self, now: Instant) -> Result<(), Error> {
        if !matches!(
            self.state,
            State::Disconnected | State::Closed | State::WaitingRetry
        ) {
            return Err(Error::new("Redis is already connecting/connected"));
        }
        self.rx.clear();
        self.state = State::Connecting;
        self.deadline = now.checked_add(self.config.connect_timeout);
        self.events.push_back(Event::Connect);
        Ok(())
    }
    pub fn transport_connected(&mut self) -> Result<(), Error> {
        if self.state != State::Connecting {
            return Err(Error::new("Unexpected connection"));
        }
        if self.config.tls {
            self.state = State::Tls;
            self.events.push_back(Event::UpgradeTls);
        } else {
            self.begin_handshake();
        }
        Ok(())
    }
    pub fn tls_established(&mut self) -> Result<(), Error> {
        if self.state != State::Tls {
            return Err(Error::new("Unexpected TLS completion"));
        }
        self.begin_handshake();
        Ok(())
    }
    fn begin_handshake(&mut self) {
        self.state = State::Handshake;
        self.resp3 = false;
        self.subscriber = false;
        if self.config.prefer_resp3 {
            self.handshake = Handshake::Hello;
            if let Some(password) = &self.config.password {
                encode_command(
                    &[
                        b"HELLO",
                        b"3",
                        b"AUTH",
                        self.config
                            .username
                            .as_deref()
                            .unwrap_or("default")
                            .as_bytes(),
                        password.as_bytes(),
                    ],
                    &mut self.tx,
                );
            } else {
                encode_command(&[b"HELLO", b"3"], &mut self.tx);
            }
        } else {
            self.auth();
        }
    }
    fn auth(&mut self) {
        self.handshake = Handshake::Auth;
        if let Some(password) = &self.config.password {
            if let Some(user) = &self.config.username {
                encode_command(
                    &[b"AUTH", user.as_bytes(), password.as_bytes()],
                    &mut self.tx,
                );
            } else {
                encode_command(&[b"AUTH", password.as_bytes()], &mut self.tx);
            }
        } else {
            self.select();
        }
    }
    fn select(&mut self) {
        self.handshake = Handshake::Select;
        if self.config.database != 0 {
            encode_command(
                &[b"SELECT", self.config.database.to_string().as_bytes()],
                &mut self.tx,
            );
        } else {
            self.name();
        }
    }
    fn name(&mut self) {
        self.handshake = Handshake::Name;
        if let Some(name) = &self.config.client_name {
            encode_command(&[b"CLIENT", b"SETNAME", name.as_bytes()], &mut self.tx);
        } else {
            self.restore();
        }
    }
    fn restore(&mut self) {
        self.handshake = Handshake::Restore;
        if self.config.auto_resubscribe {
            for (kind, items) in [
                (Kind::Subscribe, &self.channels),
                (Kind::Psubscribe, &self.patterns),
            ] {
                for item in items {
                    let mut wire = self.pool.pop().unwrap_or_default();
                    wire.clear();
                    encode_command(
                        &[
                            if kind == Kind::Subscribe {
                                b"SUBSCRIBE"
                            } else {
                                b"PSUBSCRIBE"
                            },
                            item,
                        ],
                        &mut wire,
                    );
                    self.tx.extend_from_slice(&wire);
                    self.pending.push_back(Pending {
                        token: None,
                        wire,
                        deadline: None,
                        kind,
                        acks: 1,
                    });
                }
            }
            self.subscriber = !self.pending.is_empty();
        } else {
            self.channels.clear();
            self.patterns.clear();
        }
        if self.pending.is_empty() {
            self.ready();
        }
    }
    fn ready(&mut self) {
        self.state = State::Ready;
        self.deadline = None;
        self.attempt = 0;
        self.refresh_subscriber();
        self.events.push_back(Event::Ready { resp3: self.resp3 });
        while let Some(p) = self.offline.pop_front() {
            self.tx.extend_from_slice(&p.wire);
            self.pending.push_back(p);
        }
    }
    /// `deadline` is host policy, including commandTimeout or blocking-command
    /// protection. Redis's own BLPOP timeout remains in the command arguments.
    pub fn command(
        &mut self,
        token: u64,
        args: &[&[u8]],
        deadline: Option<Instant>,
    ) -> Result<(), Error> {
        let name = args.first().ok_or_else(|| Error::new("Empty command"))?;
        if self
            .pending
            .iter()
            .chain(&self.offline)
            .any(|p| p.token == Some(token))
        {
            return Err(Error::new("Duplicate command token"));
        }
        if matches!(self.state, State::Closed) {
            return Err(Error::new("Connection is closed."));
        }
        if self.state != State::Ready && !self.config.offline_queue {
            return Err(Error::new(
                "Stream isn't writeable and enableOfflineQueue options is false",
            ));
        }
        let kind = Kind::of(name);
        if self.subscriber
            && !(kind.subscription() || kind == Kind::Quit || name.eq_ignore_ascii_case(b"PING"))
        {
            return Err(Error::new(
                "Connection in subscriber mode, only subscriber commands may be used",
            ));
        }
        if matches!(kind, Kind::Subscribe | Kind::Psubscribe) {
            self.subscriber = true;
        }
        let acks = if args.len() > 1 {
            args.len() - 1
        } else {
            match kind {
                Kind::Unsubscribe | Kind::Punsubscribe => usize::MAX,
                _ => 1,
            }
        };
        let mut wire = self.pool.pop().unwrap_or_default();
        wire.clear();
        encode_command(args, &mut wire);
        let p = Pending {
            token: Some(token),
            wire,
            deadline,
            kind,
            acks,
        };
        if self.state == State::Ready {
            self.tx.extend_from_slice(&p.wire);
            self.pending.push_back(p);
        } else {
            self.offline.push_back(p);
        }
        Ok(())
    }
    pub fn receive(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if !matches!(self.state, State::Ready | State::Handshake) {
            return Err(Error::new("Bytes outside Redis protocol state"));
        }
        // Limit buffering even when the host supplies many frames at once.
        for chunk in bytes.chunks(4096) {
            self.rx.extend_from_slice(chunk);
            let mut consumed = 0;
            loop {
                if self.state == State::Ready
                    && (self.subscriber || self.rx.get(consumed) == Some(&b'>'))
                {
                    match resp::message(&self.rx[consumed..], self.config.limits) {
                        Ok(Some((message, len))) => {
                            self.events.push_back(Event::Message {
                                pattern: message.pattern.map(<[u8]>::to_vec),
                                channel: message.channel.to_vec(),
                                payload: message.payload.to_vec(),
                            });
                            consumed += len;
                            continue;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            let error = Error::new(&e.to_string());
                            self.close();
                            return Err(error);
                        }
                    }
                }
                match resp::decode(&self.rx[consumed..], self.config.limits) {
                    Ok(Some((value, n))) => {
                        consumed += n;
                        if let Err(error) = self.frame(value) {
                            self.close();
                            return Err(error);
                        }
                        if self.state == State::Closed {
                            return Ok(());
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let error = Error::new(&e.to_string());
                        self.close();
                        return Err(error);
                    }
                }
            }
            self.rx.drain(..consumed);
        }
        Ok(())
    }
    fn frame(&mut self, value: Value) -> Result<(), Error> {
        if let Value::Attribute(_, value) = value {
            return self.frame(*value);
        }
        if self.state == State::Handshake && !matches!(self.handshake, Handshake::Restore) {
            if let Value::Error(bytes) = value {
                if matches!(self.handshake, Handshake::Hello)
                    && (bytes.starts_with(b"ERR unknown command") || bytes.starts_with(b"NOPROTO"))
                {
                    self.auth();
                    return Ok(());
                }
                self.events.push_back(Event::Error(Error::reply(bytes)));
                self.close();
                return Ok(());
            }
            match self.handshake {
                Handshake::Hello => {
                    self.resp3 = true;
                    self.select();
                }
                Handshake::Auth => self.select(),
                Handshake::Select => self.name(),
                Handshake::Name => self.restore(),
                Handshake::Restore => unreachable!(),
            }
            return Ok(());
        }
        let pubsub = matches!(value, Value::Push(_))
            || (self.subscriber && matches!(value, Value::Array(_)));
        if pubsub {
            if let Some(items) = value.items() {
                let tag = items.first().and_then(Value::bytes).unwrap_or_default();
                if (tag == b"message" && items.len() == 3)
                    || (tag == b"pmessage" && items.len() == 4)
                {
                    let mut items = match value {
                        Value::Array(v) | Value::Push(v) => v,
                        _ => unreachable!(),
                    };
                    let payload = take_bytes(items.pop().unwrap())?;
                    let channel = take_bytes(items.pop().unwrap())?;
                    let pattern = if items.len() == 2 {
                        Some(take_bytes(items.pop().unwrap())?)
                    } else {
                        None
                    };
                    self.events.push_back(Event::Message {
                        pattern,
                        channel,
                        payload,
                    });
                    return Ok(());
                }
                let kind = Kind::of(tag);
                if kind.subscription() && items.len() == 3 {
                    let target = if matches!(kind, Kind::Psubscribe | Kind::Punsubscribe) {
                        &mut self.patterns
                    } else {
                        &mut self.channels
                    };
                    if let Some(channel) = items[1].bytes() {
                        if matches!(kind, Kind::Subscribe | Kind::Psubscribe) {
                            if !target.iter().any(|v| v == channel) {
                                target.push(channel.to_vec());
                            }
                        } else {
                            target.retain(|v| v != channel);
                        }
                    }
                    let empty = target.is_empty();
                    let count = items[2]
                        .integer()
                        .ok_or_else(|| Error::new("Invalid subscription count"))?;
                    self.subscriber = count != 0;
                    if let Some(p) = self.pending.front_mut().filter(|p| p.kind == kind) {
                        let done = if p.acks == usize::MAX {
                            empty
                        } else {
                            p.acks = p.acks.saturating_sub(1);
                            p.acks == 0
                        };
                        if done {
                            self.complete(Value::Integer(count))?;
                        }
                        if self.state == State::Handshake && self.pending.is_empty() {
                            self.ready();
                        }
                    } else {
                        return Err(Error::new("Unexpected subscription acknowledgement"));
                    }
                    return Ok(());
                }
                if tag == b"pong" && !self.resp3 {
                    // RESP2 subscriber PING returns [pong, payload].
                    let pong = items
                        .get(1)
                        .and_then(Value::bytes)
                        .unwrap_or_default()
                        .to_vec();
                    self.complete(Value::Bulk(pong))?;
                    return Ok(());
                }
            }
            if matches!(value, Value::Push(_)) {
                self.events.push_back(Event::Push(value));
                return Ok(());
            }
        }
        self.complete(value)
    }
    fn complete(&mut self, value: Value) -> Result<(), Error> {
        let p = self
            .pending
            .pop_front()
            .ok_or_else(|| Error::new("Unexpected Redis reply"))?;
        if let Some(token) = p.token {
            let result = if let Value::Error(bytes) = value {
                Err(Error::reply(bytes))
            } else {
                Ok(value)
            };
            self.events.push_back(Event::Reply { token, result });
        }
        self.pool.push(p.wire);
        self.refresh_subscriber();
        if p.kind == Kind::Quit {
            self.close();
        }
        Ok(())
    }
    fn refresh_subscriber(&mut self) {
        self.subscriber = !self.channels.is_empty()
            || !self.patterns.is_empty()
            || self
                .pending
                .iter()
                .chain(&self.offline)
                .any(|p| matches!(p.kind, Kind::Subscribe | Kind::Psubscribe));
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.deadline
            .into_iter()
            .chain(
                self.pending
                    .iter()
                    .chain(&self.offline)
                    .filter_map(|p| p.deadline),
            )
            .min()
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        for p in &mut self.pending {
            if p.deadline.is_some_and(|d| d <= now) {
                p.deadline = None;
                if let Some(token) = p.token.take() {
                    self.events.push_back(Event::Reply {
                        token,
                        result: Err(Error::new("Command timed out")),
                    });
                }
            }
        }
        let mut i = 0;
        while i < self.offline.len() {
            if self.offline[i].deadline.is_some_and(|d| d <= now) {
                let p = self.offline.remove(i).unwrap();
                if let Some(token) = p.token {
                    self.events.push_back(Event::Reply {
                        token,
                        result: Err(Error::new("Command timed out")),
                    });
                }
                self.pool.push(p.wire);
            } else {
                i += 1;
            }
        }
        if self.deadline.is_some_and(|d| d <= now) {
            self.deadline = None;
            if self.state == State::WaitingRetry {
                let _ = self.connect(now);
            } else {
                self.events
                    .push_back(Event::Error(Error::new("connect ETIMEDOUT")));
                self.transport_lost();
            }
        }
    }
    /// Call once per failed transport, including DNS/connect/TLS failures. The
    /// host closes the socket on CloseTransport before accepting new Connect.
    pub fn transport_lost(&mut self) {
        if matches!(
            self.state,
            State::Closed | State::RetryDecision | State::WaitingRetry
        ) {
            return;
        }
        self.tx.clear();
        self.tx_pos = 0;
        self.rx.clear();
        self.deadline = None;
        while let Some(p) = self.pending.pop_back() {
            if self.config.auto_resend_unfulfilled && p.token.is_some() {
                self.offline.push_front(p);
            } else {
                self.fail_pending(p, "Connection is closed.");
            }
        }
        self.attempt = self.attempt.saturating_add(1);
        if self
            .config
            .max_retries_per_request
            .is_some_and(|n| self.attempt > n)
        {
            while let Some(p) = self.offline.pop_front() {
                self.fail_pending(p, "Reached the max retries per request limit");
            }
        }
        self.state = State::RetryDecision;
        self.events.push_back(Event::CloseTransport);
        self.events.push_back(Event::Retry {
            attempt: self.attempt,
        });
    }
    pub fn retry(&mut self, now: Instant, delay: Option<Duration>) -> Result<(), Error> {
        if self.state != State::RetryDecision {
            return Err(Error::new("No retry decision requested"));
        }
        if let Some(delay) = delay {
            self.state = State::WaitingRetry;
            self.deadline = now.checked_add(delay);
        } else {
            self.close();
        }
        Ok(())
    }
    fn fail_pending(&mut self, p: Pending, message: &str) {
        if let Some(token) = p.token {
            self.events.push_back(Event::Reply {
                token,
                result: Err(Error::new(message)),
            });
        }
        self.pool.push(p.wire);
    }
    pub fn close(&mut self) {
        if self.state == State::Closed {
            return;
        }
        while let Some(p) = self.pending.pop_front() {
            self.fail_pending(p, "Connection is closed.");
        }
        while let Some(p) = self.offline.pop_front() {
            self.fail_pending(p, "Connection is closed.");
        }
        self.state = State::Closed;
        self.deadline = None;
        self.tx.clear();
        self.tx_pos = 0;
        self.channels.clear();
        self.patterns.clear();
        self.subscriber = false;
        self.events.push_back(Event::CloseTransport);
        self.events.push_back(Event::Closed);
    }
}
fn take_bytes(value: Value) -> Result<Vec<u8>, Error> {
    match value {
        Value::Bulk(v) | Value::Simple(v) => Ok(v),
        _ => Err(Error::new("Invalid pub/sub message")),
    }
}
