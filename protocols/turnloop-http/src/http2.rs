//! RFC 9113 connection/stream framing. Caller retains partial frames and acknowledges
//! writes. DATA is borrowed, flow-control credit is returned explicitly by the host.
use crate::{Error, Result, hpack, http1::Header};
use std::time::Instant;
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub frame_size: usize,
    pub header_block: usize,
    pub header_list: usize,
    pub continuations: usize,
    pub streams: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            frame_size: 16384,
            header_block: 65536,
            header_list: 32768,
            continuations: 16,
            streams: 100,
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    pub kind: u8,
    pub flags: u8,
    pub stream: u32,
    pub payload: &'a [u8],
}
fn error(code: &'static str, message: &'static str) -> Error {
    Error::new(code, message)
}
fn protocol(message: &'static str) -> Error {
    error("PROTOCOL_ERROR", message)
}
fn frame_error() -> Error {
    error("FRAME_SIZE_ERROR", "invalid frame size")
}
fn u32be(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
pub fn decode_frame(input: &[u8], max: usize) -> Result<Option<Frame<'_>>> {
    if input.len() < 9 {
        return Ok(None);
    }
    let len = ((input[0] as usize) << 16) | ((input[1] as usize) << 8) | input[2] as usize;
    if len > max {
        return Err(frame_error());
    }
    if input.len() < 9 + len {
        return Ok(None);
    }
    Ok(Some(Frame {
        kind: input[3],
        flags: input[4],
        stream: u32be(&input[5..9]) & 0x7fffffff,
        payload: &input[9..9 + len],
    }))
}
pub fn encode_frame(
    kind: u8,
    flags: u8,
    stream: u32,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> Result<()> {
    if payload.len() > 0xffffff || stream > 0x7fffffff {
        return Err(frame_error());
    }
    let len = payload.len() as u32;
    out.extend_from_slice(&len.to_be_bytes()[1..]);
    out.push(kind);
    out.push(flags);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(())
}
#[derive(Debug)]
pub enum Event<'a> {
    Settings,
    Headers {
        stream: u32,
        headers: Vec<Header>,
        end_stream: bool,
    },
    Data {
        stream: u32,
        bytes: &'a [u8],
        end_stream: bool,
    },
    Reset {
        stream: u32,
        code: u32,
    },
    Goaway {
        last_stream: u32,
        code: u32,
    },
    Ping {
        ack: bool,
        data: [u8; 8],
    },
    WindowUpdate {
        stream: u32,
    },
}
pub struct Step<'a> {
    pub consumed: usize,
    pub event: Option<Event<'a>>,
}
#[derive(Debug)]
struct Stream {
    id: u32,
    send_window: i64,
    recv_window: i64,
    unreleased: u32,
    local_end: bool,
    remote_end: bool,
    received_head: bool,
    sent_head: bool,
    recv_length: Option<u64>,
    received: u64,
    head_request: bool,
    recv_no_body: bool,
    send_no_body: bool,
    send_length: Option<u64>,
    sent: u64,
}
impl Stream {
    fn closed(&self) -> bool {
        self.local_end && self.remote_end
    }
}
pub struct Connection {
    role: Role,
    limits: Limits,
    streams: Vec<Stream>,
    next_id: u32,
    last_remote: u32,
    send_window: i64,
    recv_window: i64,
    initial_send: i64,
    peer_frame: usize,
    peer_streams: usize,
    preface: bool,
    settings_received: bool,
    settings_awaiting_ack: bool,
    decoder: hpack::Decoder,
    encoder: hpack::Encoder,
    block: Vec<u8>,
    scratch: Vec<u8>,
    continuation: Option<(u32, bool, usize)>,
    output: Vec<u8>,
    output_pos: usize,
    draining: bool,
    failed: bool,
    settings_deadline: Option<Instant>,
}
impl Connection {
    pub fn new(role: Role, limits: Limits) -> Result<Self> {
        if !(16384..=0xffffff).contains(&limits.frame_size)
            || limits.streams == 0
            || limits.streams > u32::MAX as usize
        {
            return Err(protocol("invalid limits"));
        }
        let mut result = Self {
            role,
            limits,
            streams: Vec::with_capacity(limits.streams),
            next_id: if role == Role::Client { 1 } else { 2 },
            last_remote: 0,
            send_window: 65535,
            recv_window: 65535,
            initial_send: 65535,
            peer_frame: 16384,
            peer_streams: usize::MAX,
            preface: role == Role::Client,
            settings_received: false,
            settings_awaiting_ack: true,
            decoder: hpack::Decoder::new(4096, limits.header_list),
            encoder: hpack::Encoder::new(4096),
            block: Vec::new(),
            scratch: Vec::new(),
            continuation: None,
            output: Vec::new(),
            output_pos: 0,
            draining: false,
            failed: false,
            settings_deadline: None,
        };
        if role == Role::Client {
            result.output.extend_from_slice(PREFACE);
        }
        let mut settings = Vec::new();
        for (id, value) in [
            (3, limits.streams as u32),
            (5, limits.frame_size as u32),
            (6, limits.header_list as u32),
        ] {
            settings.extend_from_slice(&(id as u16).to_be_bytes());
            settings.extend_from_slice(&value.to_be_bytes());
        }
        if role == Role::Client {
            settings.extend_from_slice(&[0, 2, 0, 0, 0, 0]);
        }
        result.frame(4, 0, 0, &settings)?;
        Ok(result)
    }
    /// Host-supplied SETTINGS acknowledgement deadline; no clock is sampled.
    pub fn set_settings_deadline(&mut self, deadline: Option<Instant>) {
        self.settings_deadline = deadline;
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        if self.settings_awaiting_ack && !self.failed {
            self.settings_deadline
        } else {
            None
        }
    }
    pub fn handle_timeout(&mut self, now: Instant) -> Option<Error> {
        if self.next_timeout().is_some_and(|d| d <= now) {
            self.failed = true;
            let mut payload = [0; 8];
            payload[..4].copy_from_slice(&self.last_remote.to_be_bytes());
            payload[7] = 4;
            self.frame(7, 0, 0, &payload).expect("fixed-size GOAWAY");
            Some(error(
                "SETTINGS_TIMEOUT",
                "SETTINGS acknowledgement timeout",
            ))
        } else {
            None
        }
    }
    /// Notify transport loss, then call `poll_failed_stream` until exhausted.
    pub fn eof(&mut self) {
        self.failed = true;
    }
    /// Exactly one terminal result for each still-open stream after connection failure.
    /// Fully ended/reset streams have already produced their terminal event.
    pub fn poll_failed_stream(&mut self) -> Option<u32> {
        if !self.failed {
            return None;
        }
        let stream = self.streams.iter_mut().find(|s| !s.remote_end)?;
        stream.local_end = true;
        stream.remote_end = true;
        Some(stream.id)
    }
    pub fn output(&self) -> &[u8] {
        &self.output[self.output_pos..]
    }
    pub fn consume_output(&mut self, n: usize) -> Result<()> {
        if n > self.output().len() {
            return Err(protocol("write acknowledgement exceeds output"));
        }
        self.output_pos += n;
        if self.output_pos == self.output.len() {
            self.output.clear();
            self.output_pos = 0;
        }
        Ok(())
    }
    fn frame(&mut self, kind: u8, flags: u8, id: u32, payload: &[u8]) -> Result<()> {
        encode_frame(kind, flags, id, payload, &mut self.output)
    }
    fn index(&self, id: u32) -> Result<usize> {
        self.streams
            .iter()
            .position(|s| s.id == id)
            .ok_or(protocol("unknown or idle stream"))
    }
    fn active(&self) -> usize {
        self.streams.iter().filter(|s| !s.closed()).count()
    }
    fn add_stream(&mut self, id: u32) -> Result<usize> {
        if self.active() >= self.limits.streams {
            return Err(error("REFUSED_STREAM", "stream limit"));
        }
        let stream = Stream {
            id,
            send_window: self.initial_send,
            recv_window: 65535,
            unreleased: 0,
            local_end: false,
            remote_end: false,
            received_head: false,
            sent_head: false,
            recv_length: None,
            received: 0,
            head_request: false,
            recv_no_body: false,
            send_no_body: false,
            send_length: None,
            sent: 0,
        };
        if let Some(i) = self
            .streams
            .iter()
            .position(|s| s.closed() && s.unreleased == 0)
        {
            self.streams[i] = stream;
            Ok(i)
        } else {
            if self.streams.len() >= self.limits.streams {
                return Err(error(
                    "REFUSED_STREAM",
                    "release consumed capacity before opening streams",
                ));
            }
            self.streams.push(stream);
            Ok(self.streams.len() - 1)
        }
    }
    pub fn open(&mut self, headers: &[Header], end_stream: bool) -> Result<u32> {
        if self.role != Role::Client || self.draining || self.failed {
            return Err(protocol("cannot open stream"));
        }
        if self.active() >= self.peer_streams {
            return Err(error("REFUSED_STREAM", "peer stream limit"));
        }
        validate_headers(headers, true, false)?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(2)
            .filter(|n| *n <= 0x7fffffff)
            .ok_or(protocol("stream IDs exhausted"))?;
        self.add_stream(id)?;
        self.send_headers(id, headers, end_stream)?;
        Ok(id)
    }
    pub fn send_headers(&mut self, id: u32, headers: &[Header], end_stream: bool) -> Result<()> {
        if self.failed {
            return Err(protocol("failed connection"));
        }
        let i = self.index(id)?;
        let s = &self.streams[i];
        if s.local_end {
            return Err(error("STREAM_CLOSED", "local stream closed"));
        }
        validate_headers(headers, self.role == Role::Client, s.sent_head)?;
        let informational = headers
            .iter()
            .find(|h| h.name == ":status")
            .is_some_and(|h| h.value.starts_with(b"1"));
        if informational && end_stream {
            return Err(protocol("informational END_STREAM"));
        }
        if s.sent_head && !end_stream {
            return Err(protocol("trailers must end stream"));
        }
        let no_body = self.role == Role::Server
            && (s.head_request
                || headers
                    .iter()
                    .any(|h| h.name == ":status" && matches!(h.value.as_slice(), b"204" | b"304")));
        let length = if s.sent_head {
            s.send_length
        } else if no_body {
            None
        } else {
            content_length(headers)?
        };
        if end_stream && length.is_some_and(|n| n != s.sent) {
            return Err(protocol("outgoing content-length mismatch"));
        }
        self.scratch.clear();
        self.encoder.encode(headers, &mut self.scratch);
        let count = self.scratch.len().max(1).div_ceil(self.peer_frame);
        for n in 0..count {
            let start = n * self.peer_frame;
            let end = (start + self.peer_frame).min(self.scratch.len());
            let flags =
                if n + 1 == count { 4 } else { 0 } | if n == 0 && end_stream { 1 } else { 0 };
            encode_frame(
                if n == 0 { 1 } else { 9 },
                flags,
                id,
                &self.scratch[start..end],
                &mut self.output,
            )?;
        }
        let s = &mut self.streams[i];
        s.sent_head |= !informational;
        s.send_no_body = no_body;
        s.send_length = length;
        s.local_end = end_stream;
        if self.role == Role::Client {
            s.head_request = headers
                .iter()
                .any(|h| h.name == ":method" && h.value == b"HEAD");
        }
        Ok(())
    }
    /// Returns accepted bytes (zero on flow-control stall). Host retains remainder.
    pub fn send_data(&mut self, id: u32, bytes: &[u8], end_stream: bool) -> Result<usize> {
        let i = self.index(id)?;
        let s = &self.streams[i];
        if self.failed || s.local_end || !s.sent_head {
            return Err(error("STREAM_CLOSED", "cannot send DATA"));
        }
        let n = bytes
            .len()
            .min(self.peer_frame)
            .min(self.send_window.max(0) as usize)
            .min(s.send_window.max(0) as usize);
        if n == 0 && !bytes.is_empty() {
            return Ok(0);
        }
        let end = end_stream && n == bytes.len();
        if s.send_no_body && !bytes.is_empty() {
            return Err(protocol("DATA on bodyless response"));
        }
        if s.send_length.is_some_and(|length| {
            s.sent + n as u64 > length || (end && s.sent + n as u64 != length)
        }) {
            return Err(protocol("outgoing content-length mismatch"));
        }
        self.frame(0, u8::from(end), id, &bytes[..n])?;
        self.send_window -= n as i64;
        let s = &mut self.streams[i];
        s.send_window -= n as i64;
        s.sent += n as u64;
        s.local_end = end;
        Ok(n)
    }
    pub fn release_capacity(&mut self, id: u32, n: u32) -> Result<()> {
        let i = self.index(id)?;
        if n == 0 {
            return Ok(());
        }
        let s = &mut self.streams[i];
        if n > s.unreleased {
            return Err(protocol("capacity exceeds received DATA"));
        }
        s.unreleased -= n;
        s.recv_window += n as i64;
        self.recv_window += n as i64;
        self.frame(8, 0, 0, &n.to_be_bytes())?;
        if !self.streams[i].remote_end {
            self.frame(8, 0, id, &n.to_be_bytes())?;
        }
        Ok(())
    }
    pub fn reset(&mut self, id: u32, code: u32) -> Result<()> {
        let i = self.index(id)?;
        self.streams[i].local_end = true;
        self.streams[i].remote_end = true;
        self.frame(3, 0, id, &code.to_be_bytes())
    }
    pub fn ping(&mut self, data: [u8; 8]) -> Result<()> {
        self.frame(6, 0, 0, &data)
    }
    pub fn shutdown(&mut self) -> Result<()> {
        self.draining = true;
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&self.last_remote.to_be_bytes());
        self.frame(7, 0, 0, &bytes)
    }
    pub fn is_drained(&self) -> bool {
        self.draining && self.active() == 0 && self.output().is_empty()
    }
    pub fn receive<'a>(&mut self, input: &'a [u8]) -> Result<Step<'a>> {
        if self.failed {
            return Err(protocol("failed connection"));
        }
        let result = self.receive_inner(input);
        if let Err(e) = &result {
            self.failed = true;
            let code = match e.code {
                "FLOW_CONTROL_ERROR" => 3,
                "FRAME_SIZE_ERROR" => 6,
                "COMPRESSION_ERROR" => 9,
                "ENHANCE_YOUR_CALM" => 11,
                _ => 1,
            };
            let mut payload = [0; 8];
            payload[..4].copy_from_slice(&self.last_remote.to_be_bytes());
            payload[4..].copy_from_slice(&(code as u32).to_be_bytes());
            self.frame(7, 0, 0, &payload)?;
        }
        result
    }
    fn receive_inner<'a>(&mut self, input: &'a [u8]) -> Result<Step<'a>> {
        if !self.preface {
            let n = input.len().min(PREFACE.len());
            if input[..n] != PREFACE[..n] {
                return Err(protocol("invalid client preface"));
            }
            if n < PREFACE.len() {
                return Ok(Step {
                    consumed: 0,
                    event: None,
                });
            }
            self.preface = true;
            return Ok(Step {
                consumed: PREFACE.len(),
                event: None,
            });
        }
        let Some(f) = decode_frame(input, self.limits.frame_size)? else {
            return Ok(Step {
                consumed: 0,
                event: None,
            });
        };
        if !self.settings_received && (f.kind != 4 || f.flags & 1 != 0) {
            return Err(protocol("first frame must be SETTINGS"));
        }
        if let Some((id, _, _)) = self.continuation {
            if f.kind != 9 || f.stream != id {
                return Err(protocol("interleaved header block"));
            }
        } else if f.kind == 9 {
            return Err(protocol("unexpected CONTINUATION"));
        }
        let mut step = Step {
            consumed: 9 + f.payload.len(),
            event: None,
        };
        match f.kind {
            0 => {
                if f.stream == 0 {
                    return Err(protocol("DATA on connection"));
                }
                let i = self.index(f.stream)?;
                let s = &mut self.streams[i];
                if s.remote_end || !s.received_head {
                    return Err(protocol("DATA in invalid stream state"));
                }
                let payload = unpadded(f)?;
                let flow = f.payload.len() as i64;
                if flow > self.recv_window || flow > s.recv_window {
                    return Err(error("FLOW_CONTROL_ERROR", "receive window exceeded"));
                }
                self.recv_window -= flow;
                s.recv_window -= flow;
                s.unreleased += flow as u32;
                s.received += payload.len() as u64;
                let end = f.flags & 1 != 0;
                if s.recv_no_body && !payload.is_empty() {
                    return Err(protocol("DATA on bodyless response"));
                }
                if s.recv_length
                    .is_some_and(|n| s.received > n || (end && n != s.received))
                {
                    return Err(protocol("content-length mismatch"));
                }
                s.remote_end = end;
                // Padding is consumed by the protocol, never charged to the application.
                let padding = f.payload.len() - payload.len();
                if padding > 0 {
                    self.release_capacity(f.stream, padding as u32)?;
                }
                step.event = Some(Event::Data {
                    stream: f.stream,
                    bytes: payload,
                    end_stream: end,
                });
            }
            1 => {
                if f.stream == 0 {
                    return Err(protocol("HEADERS on connection"));
                }
                if self.index(f.stream).is_err() {
                    if self.role != Role::Server
                        || f.stream % 2 == 0
                        || f.stream <= self.last_remote
                        || self.draining
                    {
                        return Err(protocol("invalid new stream"));
                    }
                    self.last_remote = f.stream;
                    self.add_stream(f.stream)?;
                }
                let mut payload = unpadded(f)?;
                if f.flags & 32 != 0 {
                    if payload.len() < 5 {
                        return Err(frame_error());
                    }
                    if u32be(payload) & 0x7fffffff == f.stream {
                        return Err(protocol("stream depends on itself"));
                    }
                    payload = &payload[5..];
                }
                self.block.clear();
                self.append_block(payload)?;
                let end = f.flags & 1 != 0;
                if f.flags & 4 != 0 {
                    step.event = Some(self.finish_headers(f.stream, end)?);
                } else {
                    self.continuation = Some((f.stream, end, 0));
                }
            }
            9 => {
                let (id, end, count) = self.continuation.unwrap();
                if count >= self.limits.continuations {
                    return Err(error("ENHANCE_YOUR_CALM", "CONTINUATION limit"));
                }
                self.append_block(f.payload)?;
                if f.flags & 4 != 0 {
                    self.continuation = None;
                    step.event = Some(self.finish_headers(id, end)?);
                } else {
                    self.continuation = Some((id, end, count + 1));
                }
            }
            2 => {
                if f.stream == 0 {
                    return Err(protocol("PRIORITY on connection"));
                }
                if f.payload.len() != 5 {
                    return Err(frame_error());
                }
                if u32be(f.payload) & 0x7fffffff == f.stream {
                    return Err(protocol("stream depends on itself"));
                }
            }
            3 => {
                if f.stream == 0 {
                    return Err(protocol("RST_STREAM on connection"));
                }
                if f.payload.len() != 4 {
                    return Err(frame_error());
                }
                let i = self.index(f.stream)?;
                self.streams[i].remote_end = true;
                self.streams[i].local_end = true;
                step.event = Some(Event::Reset {
                    stream: f.stream,
                    code: u32be(f.payload),
                });
            }
            4 => {
                if f.stream != 0 {
                    return Err(protocol("SETTINGS on stream"));
                }
                if f.flags & 1 != 0 {
                    if !f.payload.is_empty() {
                        return Err(frame_error());
                    }
                    if !self.settings_awaiting_ack {
                        return Err(protocol("unsolicited SETTINGS ack"));
                    }
                    self.settings_awaiting_ack = false;
                } else {
                    if f.payload.len() % 6 != 0 {
                        return Err(frame_error());
                    }
                    for setting in f.payload.as_chunks::<6>().0 {
                        let id = u16::from_be_bytes(setting[..2].try_into().unwrap());
                        let value = u32be(&setting[2..]);
                        match id {
                            1 => self.encoder.set_table_size((value as usize).min(4096)),
                            2 => {
                                if value > 1 || self.role == Role::Client {
                                    return Err(protocol("invalid ENABLE_PUSH"));
                                }
                            }
                            3 => self.peer_streams = value as usize,
                            4 => {
                                if value > 0x7fffffff {
                                    return Err(error(
                                        "FLOW_CONTROL_ERROR",
                                        "invalid initial window",
                                    ));
                                }
                                let delta = value as i64 - self.initial_send;
                                for s in &mut self.streams {
                                    if s.send_window + delta > 0x7fffffff {
                                        return Err(error("FLOW_CONTROL_ERROR", "window overflow"));
                                    }
                                    s.send_window += delta;
                                }
                                self.initial_send = value as i64;
                            }
                            5 => {
                                if !(16384..=0xffffff).contains(&value) {
                                    return Err(protocol("invalid MAX_FRAME_SIZE"));
                                }
                                self.peer_frame = value as usize;
                            }
                            _ => {}
                        }
                    }
                    self.settings_received = true;
                    self.frame(4, 1, 0, &[])?;
                    step.event = Some(Event::Settings);
                }
            }
            5 => return Err(protocol("server push disabled")),
            6 => {
                if f.stream != 0 {
                    return Err(protocol("PING on stream"));
                }
                if f.payload.len() != 8 {
                    return Err(frame_error());
                }
                let data = f.payload.try_into().unwrap();
                let ack = f.flags & 1 != 0;
                if !ack {
                    self.frame(6, 1, 0, f.payload)?;
                }
                step.event = Some(Event::Ping { ack, data });
            }
            7 => {
                if f.stream != 0 {
                    return Err(protocol("GOAWAY on stream"));
                }
                if f.payload.len() < 8 {
                    return Err(frame_error());
                }
                self.draining = true;
                step.event = Some(Event::Goaway {
                    last_stream: u32be(f.payload) & 0x7fffffff,
                    code: u32be(&f.payload[4..]),
                });
            }
            8 => {
                if f.payload.len() != 4 {
                    return Err(frame_error());
                }
                let n = (u32be(f.payload) & 0x7fffffff) as i64;
                if n == 0 {
                    if f.stream != 0 {
                        self.reset(f.stream, 1)?;
                        step.event = Some(Event::Reset {
                            stream: f.stream,
                            code: 1,
                        });
                        return Ok(step);
                    }
                    return Err(protocol("zero WINDOW_UPDATE"));
                }
                let window = if f.stream == 0 {
                    &mut self.send_window
                } else {
                    let i = self.index(f.stream)?;
                    &mut self.streams[i].send_window
                };
                if *window + n > 0x7fffffff {
                    if f.stream != 0 {
                        self.reset(f.stream, 3)?;
                        step.event = Some(Event::Reset {
                            stream: f.stream,
                            code: 3,
                        });
                        return Ok(step);
                    }
                    return Err(error("FLOW_CONTROL_ERROR", "window overflow"));
                }
                *window += n;
                step.event = Some(Event::WindowUpdate { stream: f.stream });
            }
            _ => {}
        }
        Ok(step)
    }
    fn append_block(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > self.limits.header_block.saturating_sub(self.block.len()) {
            return Err(error("ENHANCE_YOUR_CALM", "compressed header block limit"));
        }
        self.block.extend_from_slice(bytes);
        Ok(())
    }
    fn finish_headers(&mut self, id: u32, end: bool) -> Result<Event<'static>> {
        let mut headers = Vec::new();
        self.decoder.decode(&self.block, &mut headers)?;
        let i = self.index(id)?;
        let s = &mut self.streams[i];
        if s.remote_end {
            return Err(error("STREAM_CLOSED", "headers after END_STREAM"));
        }
        validate_headers(&headers, self.role == Role::Server, s.received_head)?;
        let informational = headers
            .iter()
            .find(|h| h.name == ":status")
            .is_some_and(|h| h.value.starts_with(b"1"));
        if informational && end {
            return Err(protocol("informational END_STREAM"));
        }
        if s.received_head && !end {
            return Err(protocol("trailers without END_STREAM"));
        }
        if !s.received_head && !informational {
            s.recv_no_body = self.role == Role::Client
                && (s.head_request
                    || headers.iter().any(|h| {
                        h.name == ":status" && matches!(h.value.as_slice(), b"204" | b"304")
                    }));
            s.recv_length = if s.recv_no_body {
                None
            } else {
                content_length(&headers)?
            };
            if self.role == Role::Server {
                s.head_request = headers
                    .iter()
                    .any(|h| h.name == ":method" && h.value == b"HEAD");
            }
            s.received_head = true;
        }
        if end && s.recv_length.is_some_and(|n| n != s.received) {
            return Err(protocol("content-length mismatch"));
        }
        s.remote_end = end;
        Ok(Event::Headers {
            stream: id,
            headers,
            end_stream: end,
        })
    }
}
fn unpadded(f: Frame<'_>) -> Result<&[u8]> {
    if f.flags & 8 == 0 {
        return Ok(f.payload);
    }
    let pad = *f.payload.first().ok_or_else(frame_error)? as usize;
    if pad >= f.payload.len() {
        return Err(protocol("invalid padding"));
    }
    Ok(&f.payload[1..f.payload.len() - pad])
}
fn content_length(headers: &[Header]) -> Result<Option<u64>> {
    let mut found = None;
    for h in headers {
        if h.name == "content-length" {
            if found.is_some() || h.value.is_empty() || !h.value.iter().all(u8::is_ascii_digit) {
                return Err(protocol("invalid content-length"));
            }
            found = Some(
                h.value
                    .iter()
                    .try_fold(0u64, |n, b| {
                        n.checked_mul(10)?.checked_add((b - b'0') as u64)
                    })
                    .ok_or(protocol("content-length overflow"))?,
            );
        }
    }
    Ok(found)
}
fn validate_headers(headers: &[Header], request: bool, trailers: bool) -> Result<()> {
    let mut regular = false;
    let mut pseudo = [false; 5];
    for h in headers {
        if h.name.is_empty()
            || h.name.bytes().any(|b| b.is_ascii_uppercase())
            || h.value.iter().any(|b| matches!(b, 0 | 10 | 13))
            || h.value.first().is_some_and(|b| matches!(b, b' ' | b'\t'))
            || h.value.last().is_some_and(|b| matches!(b, b' ' | b'\t'))
        {
            return Err(protocol("invalid header field"));
        }
        if h.name.starts_with(':') {
            if regular || trailers {
                return Err(protocol("pseudo-header ordering"));
            }
            let index = match h.name.as_str() {
                ":method" if request => 0,
                ":scheme" if request => 1,
                ":path" if request => 2,
                ":authority" if request => 3,
                ":status" if !request => 4,
                _ => return Err(protocol("invalid pseudo-header")),
            };
            if pseudo[index] {
                return Err(protocol("duplicate pseudo-header"));
            }
            pseudo[index] = true;
        } else {
            regular = true;
            if !crate::http1::valid_token(h.name.as_bytes()) {
                return Err(protocol("invalid header name"));
            }
            if matches!(
                h.name.as_str(),
                "connection" | "proxy-connection" | "keep-alive" | "transfer-encoding" | "upgrade"
            ) || h.name == "te" && h.value != b"trailers"
            {
                return Err(protocol("connection-specific field"));
            }
        }
    }
    if !trailers {
        if request {
            let connect = headers
                .iter()
                .any(|h| h.name == ":method" && h.value == b"CONNECT");
            if !pseudo[0]
                || if connect {
                    !pseudo[3] || pseudo[1] || pseudo[2]
                } else {
                    !pseudo[1]
                        || !pseudo[2]
                        || headers
                            .iter()
                            .any(|h| h.name == ":path" && h.value.is_empty())
                }
            {
                return Err(protocol("missing request pseudo-header"));
            }
        } else {
            let Some(status) = headers.iter().find(|h| h.name == ":status") else {
                return Err(protocol("missing status"));
            };
            if status.value.len() != 3
                || !status.value.iter().all(u8::is_ascii_digit)
                || status.value.as_slice() < b"100".as_slice()
                || status.value == b"101"
            {
                return Err(protocol("invalid status"));
            }
        }
    }
    if trailers && headers.iter().any(|h| h.name == "content-length") {
        return Err(protocol("content-length in trailers"));
    }
    content_length(headers)?;
    Ok(())
}
