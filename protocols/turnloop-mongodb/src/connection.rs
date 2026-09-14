//! mongodb-handshake/handshake.md §§ Connection Handshake, Speculative Authentication.
//! Events are pull-based. A token is accepted only on successful `command`, and
//! yields exactly one Reply, Failed, or Unacknowledged event before reuse.
use crate::{
    auth::{Mechanism, Scram},
    uri::Options,
    wire::{self, Decoder, Message},
    Error, ErrorKind, Result,
};
use bson::{doc, raw::RawDocument, Document};
use std::{collections::VecDeque, time::Instant};
#[derive(Debug)]
pub enum ConnectionEvent {
    UpgradeTls,
    Ready,
    Reply { token: u64 },
    Unacknowledged { token: u64 },
    Failed { token: Option<u64>, error: Error },
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    New,
    Tls,
    Handshake,
    Auth,
    Ready,
    Command,
    UnackSending,
    Reply,
    Closed,
}
pub struct Connection {
    options: Options,
    state: State,
    tx: Vec<u8>,
    tx_at: usize,
    decoder: Decoder,
    expanded: Vec<u8>,
    compressed: Vec<u8>,
    compressor: flate2::Compress,
    decompressor: flate2::Decompress,
    zlib: bool,
    events: VecDeque<ConnectionEvent>,
    request_id: i32,
    expected: i32,
    token: Option<u64>,
    deadline: Option<Instant>,
    scram: Option<Scram>,
    nonce: String,
    pub hello: Option<Document>,
    pub max_message_size: usize,
    pub max_bson_size: usize,
    pub max_write_batch_size: usize,
}
impl Connection {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            state: State::New,
            tx: Vec::with_capacity(8192),
            tx_at: 0,
            decoder: Decoder::new(wire::DEFAULT_MAX_MESSAGE),
            expanded: Vec::with_capacity(8192),
            compressed: Vec::with_capacity(8192),
            compressor: flate2::Compress::new(flate2::Compression::default(), true),
            decompressor: flate2::Decompress::new(true),
            zlib: false,
            events: VecDeque::with_capacity(8),
            request_id: 0,
            expected: 0,
            token: None,
            deadline: None,
            scram: None,
            nonce: String::new(),
            hello: None,
            max_message_size: wire::DEFAULT_MAX_MESSAGE,
            max_bson_size: 16 * 1024 * 1024,
            max_write_batch_size: 100_000,
        }
    }
    /// `nonce` must come from a host CSPRNG if credentials are configured.
    pub fn connected(&mut self, now: Instant, nonce: &str) -> Result<()> {
        if self.state != State::New {
            return Err(Error::protocol("Connection already started"));
        }
        self.nonce.push_str(nonce);
        self.deadline = if self.options.connect_timeout.is_zero() {
            None
        } else {
            Some(now + self.options.connect_timeout)
        };
        if self.options.tls {
            self.state = State::Tls;
            self.events.push_back(ConnectionEvent::UpgradeTls);
            Ok(())
        } else {
            self.handshake()
        }
    }
    pub fn tls_established(&mut self) -> Result<()> {
        if self.state != State::Tls {
            return Err(Error::protocol("Unexpected TLS completion"));
        }
        self.handshake()
    }
    fn handshake(&mut self) -> Result<()> {
        let mut d = doc! {"isMaster":1,"helloOk":true,"client":{"driver":{"name":"turnloop-mongodb","version":env!("CARGO_PKG_VERSION")},"os":{"type":std::env::consts::OS}},"compression":self.options.compressors.clone(),"$db":"admin"};
        if let Some(app) = &self.options.app_name {
            d.get_document_mut("client")
                .unwrap()
                .insert("application", doc! {"name":app});
        }
        if let Some(c) = &self.options.credential {
            if c.mechanism.is_none() {
                d.insert("saslSupportedMechs", format!("{}.{}", c.source, c.username));
            }
            let mut scram = Scram::new(c, c.mechanism.unwrap_or(Mechanism::Sha256), &self.nonce)?;
            d.insert("speculativeAuthenticate", scram.start(&c.source, true));
            self.scram = Some(scram);
        }
        self.state = State::Handshake;
        self.send_document(&d)?;
        Ok(())
    }
    fn send_document(&mut self, d: &Document) -> Result<()> {
        let raw = bson::raw::RawDocumentBuf::try_from(d)
            .map_err(|_| Error::protocol("Cannot encode BSON command"))?;
        self.send(&raw, &[], 0)
    }
    fn send(
        &mut self,
        body: &RawDocument,
        seq: &[(&str, &[&RawDocument])],
        flags: u32,
    ) -> Result<()> {
        let id = self.request_id.wrapping_add(1).max(1);
        wire::encode(&mut self.tx, id, 0, flags, body, seq, self.max_message_size)?;
        self.request_id = id;
        self.expected = id;
        self.tx_at = 0;
        let command = body.iter_elements().next().transpose().map_err(|_|Error::protocol("Invalid command"))?.ok_or_else(||Error::protocol("Empty command"))?;
        self.compress(command.key().as_str())
    }
    fn compress(&mut self, command:&str)->Result<()> {
        // Compression spec § Commands Not to Compress: includes authentication secrets.
        if self.zlib
            && !matches!(
                command,
                "hello"
                    | "isMaster"
                    | "ismaster"
                    | "saslStart"
                    | "saslContinue"
                    | "authenticate"
                    | "getnonce"
                    | "createUser"
                    | "updateUser"
                    | "copydbSaslStart"
                    | "copydbgetnonce"
                    | "copydb"
            )
        {
            self.compressed.clear();
            self.compressed.extend_from_slice(&self.tx[..16]);
            self.compressed[12..16].copy_from_slice(&wire::OP_COMPRESSED.to_le_bytes());
            self.compressed
                .extend_from_slice(&wire::OP_MSG.to_le_bytes());
            self.compressed
                .extend_from_slice(&((self.tx.len() - 16) as i32).to_le_bytes());
            self.compressed.push(2);
            self.compressor.reset();
            self.compressed
                .reserve(self.tx.len() + self.tx.len() / 100 + 256);
            let status = self
                .compressor
                .compress_vec(
                    &self.tx[16..],
                    &mut self.compressed,
                    flate2::FlushCompress::Finish,
                )
                .map_err(|_| Error::protocol("Compression failed"))?;
            if status != flate2::Status::StreamEnd {
                return Err(Error::protocol("Compression buffer exhausted"));
            }
            let n = self.compressed.len() as i32;
            self.compressed[..4].copy_from_slice(&n.to_le_bytes());
            std::mem::swap(&mut self.tx, &mut self.compressed);
        }
        Ok(())
    }
    pub fn is_ready(&self) -> bool {
        self.state == State::Ready
    }
    pub fn transmit(&self) -> &[u8] {
        &self.tx[self.tx_at..]
    }
    pub fn consume_transmit(&mut self, n: usize) -> Result<()> {
        if n > self.tx.len() - self.tx_at {
            return Err(Error::protocol("Transmit consumption exceeds bytes"));
        }
        self.tx_at += n;
        if self.tx_at == self.tx.len() {
            self.tx.clear();
            self.tx_at = 0;
            if self.state==State::UnackSending {self.state=State::Ready;self.deadline=None;self.events.push_back(ConnectionEvent::Unacknowledged{token:self.token.take().unwrap()});}
        }
        Ok(())
    }
    /// Feed may consume only a frame prefix; retain and feed the remaining bytes.
    pub fn receive(&mut self, input: &[u8]) -> Result<usize> {
        if matches!(
            self.state,
            State::New | State::Tls | State::Ready | State::UnackSending | State::Reply | State::Closed
        ) {
            return Err(Error::protocol("Connection is not expecting a reply"));
        }
        let result = self.receive_inner(input);
        if let Err(e) = &result {
            self.fail(e.clone());
        }
        result
    }
    fn receive_inner(&mut self, input: &[u8]) -> Result<usize> {
        let n = self.decoder.feed(input)?;
        if !self.decoder.complete() {
            return Ok(n);
        }
        self.expanded.clear();
        if wire::i32_at(self.decoder.bytes(), 12)? == wire::OP_COMPRESSED {
            if !self.zlib {
                return Err(Error::protocol("Unnegotiated compressed reply"));
            }
            let b = self.decoder.bytes();
            if b.len() < 25 || wire::i32_at(b, 16)? != wire::OP_MSG || b[24] != 2 {
                return Err(Error::protocol("Invalid compressed envelope"));
            }
            let size = wire::i32_at(b, 20)?;
            if size < 10 || size as usize > self.max_message_size - 16 {
                return Err(Error::protocol("Decompressed size out of range"));
            }
            self.expanded.extend_from_slice(&b[..16]);
            self.expanded[..4].copy_from_slice(&(size + 16).to_le_bytes());
            self.expanded[12..16].copy_from_slice(&wire::OP_MSG.to_le_bytes());
            self.expanded.resize(16 + size as usize, 0);
            self.decompressor.reset(true);
            let status = self
                .decompressor
                .decompress(
                    &b[25..],
                    &mut self.expanded[16..],
                    flate2::FlushDecompress::Finish,
                )
                .map_err(|_| Error::protocol("Invalid zlib reply"))?;
            if status != flate2::Status::StreamEnd
                || self.decompressor.total_out() != size as u64
                || self.decompressor.total_in() != b.len() as u64 - 25
            {
                return Err(Error::protocol("Compressed reply size mismatch"));
            }
        }
        let m = Message::parse(self.frame(), self.max_message_size)?;
        if m.response_to != self.expected {
            return Err(Error::protocol("MongoDB responseTo does not match request"));
        }
        if m.flags & wire::MORE_TO_COME != 0 {
            return Err(Error::protocol("Unexpected exhaust reply"));
        }
        if self.state == State::Command {
            self.state = State::Reply;
            self.deadline = None;
            self.events.push_back(ConnectionEvent::Reply {
                token: self.token.unwrap(),
            });
            return Ok(n);
        }
        Error::from_response(m.body)?;
        let d: Document = m
            .body
            .try_into()
            .map_err(|_| Error::protocol("Malformed handshake BSON"))?;
        self.decoder.clear();
        self.expanded.clear();
        if self.state == State::Handshake {
            let wire_version = d.get_i32("maxWireVersion").unwrap_or(0);
            if wire_version < 6 {
                return Err(Error::protocol(
                    "MongoDB server must support OP_MSG (MongoDB 3.6+)",
                ));
            }
            for (key, target, min) in [
                ("maxMessageSizeBytes", &mut self.max_message_size, 26),
                ("maxBsonObjectSize", &mut self.max_bson_size, 5),
                ("maxWriteBatchSize", &mut self.max_write_batch_size, 1),
            ] {
                if let Ok(v) = d.get_i32(key) {
                    if v < min {
                        return Err(Error::protocol("Invalid handshake size limit"));
                    }
                    *target = v as usize;
                }
            }
            self.max_message_size = self.max_message_size.min(wire::DEFAULT_MAX_MESSAGE);
            self.decoder.set_max(self.max_message_size);
            self.zlib = self.options.compressors.iter().any(|c| c == "zlib")
                && d.get_array("compression")
                    .ok()
                    .is_some_and(|a| a.iter().any(|b| b.as_str() == Some("zlib")));
            self.hello = Some(d.clone());
            if let Some(c) = &self.options.credential {
                if !d.get_bool("arbiterOnly").unwrap_or(false) {
                    self.state = State::Auth;
                    let next = if let Ok(spec) = d.get_document("speculativeAuthenticate") {
                        self.scram.as_mut().unwrap().receive(spec, &c.source)?
                    } else {
                        let mechanism = c.mechanism.unwrap_or_else(|| {
                            if d.get_array("saslSupportedMechs").ok().is_some_and(|a| {
                                a.iter().any(|b| b.as_str() == Some("SCRAM-SHA-256"))
                            }) {
                                Mechanism::Sha256
                            } else {
                                Mechanism::Sha1
                            }
                        });
                        let mut scram = Scram::new(c, mechanism, &self.nonce)?;
                        let start = scram.start(&c.source, false);
                        self.scram = Some(scram);
                        Some(start)
                    };
                    if let Some(next) = next {
                        self.send_document(&next)?;
                        return Ok(n);
                    }
                }
            }
        } else if self.state == State::Auth {
            let source = &self.options.credential.as_ref().unwrap().source;
            let next = self.scram.as_mut().unwrap().receive(&d, source)?;
            if let Some(next) = next {
                self.send_document(&next)?;
                return Ok(n);
            }
        }
        self.scram = None;
        self.nonce.clear();
        self.state = State::Ready;
        self.deadline = None;
        self.events.push_back(ConnectionEvent::Ready);
        Ok(n)
    }
    fn frame(&self) -> &[u8] {
        if self.expanded.is_empty() {
            self.decoder.bytes()
        } else {
            &self.expanded
        }
    }
    pub fn reply(&self) -> Result<&RawDocument> {
        if self.state != State::Reply && !(self.state==State::Closed&&self.token.is_some()&&self.decoder.complete()) {
            return Err(Error::protocol("No reply available"));
        }
        Ok(Message::parse(self.frame(), self.max_message_size)?.body)
    }
    pub fn release_reply(&mut self) -> Result<()> {
        if self.state != State::Reply && !(self.state==State::Closed&&self.token.is_some()&&self.decoder.complete()) {
            return Err(Error::protocol("No reply to release"));
        }
        self.token = None;
        self.decoder.clear();
        self.expanded.clear();
        if self.state!=State::Closed{self.state = State::Ready;}
        Ok(())
    }
    pub fn poll_event(&mut self) -> Option<ConnectionEvent> {
        self.events.pop_front()
    }
    pub fn command(
        &mut self,
        token: u64,
        body: &RawDocument,
        sequences: &[(&str, &[&RawDocument])],
        now: Instant,
    ) -> Result<()> {
        if self.state != State::Ready || !self.tx.is_empty() {
            return Err(Error::protocol("Connection busy"));
        }
        if body.get_str("$db").is_err() {
            return Err(Error::protocol("Command requires $db"));
        }
        if body.as_bytes().len() > self.max_bson_size
            || sequences.iter().any(|(_, docs)| {
                docs.len() > self.max_write_batch_size
                    || docs.iter().any(|d| d.as_bytes().len() > self.max_bson_size)
            })
        {
            return Err(Error::protocol(
                "Command exceeds negotiated BSON or batch size",
            ));
        }
        let unack = body
            .get("writeConcern")
            .ok().flatten().and_then(|v|v.as_document())
            .is_some_and(|d| d.get_i32("w").ok() == Some(0));
        self.send(body, sequences, if unack { wire::MORE_TO_COME } else { 0 })?;
        if unack {
            self.token=Some(token);self.state=State::UnackSending;
            self.deadline=if self.options.socket_timeout.is_zero(){None}else{Some(now+self.options.socket_timeout)};
        } else {
            self.token = Some(token);
            self.state = State::Command;
            self.deadline = if self.options.socket_timeout.is_zero() {
                None
            } else {
                Some(now + self.options.socket_timeout)
            };
        }
        Ok(())
    }
    /// Sends an already encoded OP_MSG template with a fresh wire request id.
    /// Used by Operation to retain sequence storage and session identity on retries.
    pub fn command_encoded(&mut self,token:u64,frame:&[u8],now:Instant)->Result<()> {
        if self.state!=State::Ready||!self.tx.is_empty(){return Err(Error::protocol("Connection busy"));}
        let message=Message::parse(frame,self.max_message_size)?;
        if message.flags!=0||message.body.get_str("$db").is_err(){return Err(Error::protocol("Operation template requires $db and no wire flags"));}
        if message.body.as_bytes().len()>self.max_bson_size{return Err(Error::protocol("Command exceeds maxBsonObjectSize"));}
        for seq in message.sequences(){let mut n=0;for d in seq.documents(){n+=1;if n>self.max_write_batch_size||d.as_bytes().len()>self.max_bson_size{return Err(Error::protocol("Write sequence exceeds server limits"));}}}
        let command=message.body.iter_elements().next().transpose().map_err(|_|Error::protocol("Invalid command"))?.ok_or_else(||Error::protocol("Empty command"))?;
        self.tx.clear();self.tx.extend_from_slice(frame);self.request_id=self.request_id.wrapping_add(1).max(1);self.expected=self.request_id;self.tx[4..8].copy_from_slice(&self.request_id.to_le_bytes());self.tx_at=0;
        if let Err(e)=self.compress(command.key().as_str()){self.tx.clear();return Err(e);}
        self.token=Some(token);self.state=State::Command;self.deadline=if self.options.socket_timeout.is_zero(){None}else{Some(now+self.options.socket_timeout)};Ok(())
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.deadline.is_some_and(|d| now >= d) {
            self.fail(Error::new(
                ErrorKind::Timeout,
                "MongoDB connection timed out",
            ));
        }
    }
    pub fn fail(&mut self, error: Error) {
        if self.state == State::Closed {
            return;
        }
        if self.state != State::Reply {
            self.events.push_back(ConnectionEvent::Failed {
                token: self.token.take(),
                error,
            });
        }
        self.state = State::Closed;
        self.deadline = None;
        self.tx.clear();
        self.tx_at = 0;
        self.events.push_back(ConnectionEvent::Closed);
    }
    pub fn close(&mut self) {
        if self.state == State::Closed {
            return;
        }
        if self.token.is_some() && self.state != State::Reply {
            self.events.push_back(ConnectionEvent::Failed {
                token: self.token.take(),
                error: Error::new(
                    ErrorKind::Cancelled,
                    "Operation cancelled by connection close",
                ),
            });
        }
        self.state = State::Closed;
        self.deadline = None;
        self.tx.clear();
        self.tx_at = 0;
        self.events.push_back(ConnectionEvent::Closed);
    }
}
