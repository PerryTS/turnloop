//! Incremental RFC 9112 framing. The host retains unconsumed input and owns writes.
//! `Body` borrows input; head/trailer allocations are the returned representation.
use crate::{Error, Result};
use std::io::Write;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub headers: usize,
    pub head_bytes: usize,
    pub line_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            headers: 128,
            head_bytes: 32768,
            line_bytes: 8192,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}
impl Header {
    pub fn new(name: &str, value: impl AsRef<[u8]>) -> Self {
        Self {
            name: name.to_ascii_lowercase(),
            value: value.as_ref().to_vec(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub status: u16,
    pub version: u8,
    pub headers: Vec<Header>,
    pub keep_alive: bool,
}
impl Head {
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_slice())
    }
    pub fn token(&self, name: &str, token: &str) -> bool {
        self.headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case(name))
            .any(|h| tokens(&h.value).any(|t| t.eq_ignore_ascii_case(token.as_bytes())))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Request,
    Response,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Head,
    Fixed(u64),
    ChunkSize,
    Chunk(u64),
    ChunkEnd,
    Trailers,
    Eof,
    End,
    Done,
    Upgrade,
    Failed,
}
#[derive(Debug)]
pub enum Event<'a> {
    Head(Head),
    Informational(Head),
    Body(&'a [u8]),
    Trailers(Vec<Header>),
    End,
    Upgrade,
}
#[derive(Debug)]
pub struct Step<'a> {
    pub consumed: usize,
    pub event: Option<Event<'a>>,
}
#[derive(Debug)]
pub struct Decoder {
    mode: Mode,
    limits: Limits,
    state: State,
    head_request: bool,
    connect_request: bool,
    keep_alive: bool,
}
fn invalid(message: &'static str) -> Error {
    Error::new("HPE_INVALID_HEADER_TOKEN", message)
}
fn tokens(v: &[u8]) -> impl Iterator<Item = &[u8]> {
    v.split(|b| *b == b',').map(trim)
}
fn trim(v: &[u8]) -> &[u8] {
    v.trim_ascii()
}
fn boundary(input: &[u8]) -> Option<usize> {
    input
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .map(|n| n + 4)
}
fn lines_valid(input: &[u8], max: usize) -> Result<()> {
    let mut start = 0;
    for (i, b) in input.iter().enumerate() {
        if *b == b'\n' {
            if i == 0 || input[i - 1] != b'\r' {
                return Err(invalid("bare LF"));
            }
            if i - start > max {
                return Err(Error::new("HPE_HEADER_OVERFLOW", "line limit"));
            }
            start = i + 1;
        } else if *b == b'\r' && i + 1 < input.len() && input[i + 1] != b'\n' {
            return Err(invalid("bare CR"));
        }
    }
    if input.len() - start > max {
        return Err(Error::new("HPE_HEADER_OVERFLOW", "line limit"));
    }
    Ok(())
}
fn headers(raw: &[httparse::Header<'_>], limits: Limits) -> Result<Vec<Header>> {
    if raw.len() > limits.headers {
        return Err(Error::new("HPE_HEADER_OVERFLOW", "header count limit"));
    }
    Ok(raw.iter().map(|h| Header::new(h.name, h.value)).collect())
}
fn lengths(head: &Head) -> Result<(Option<u64>, bool)> {
    let mut cl = None;
    let mut te = false;
    for h in &head.headers {
        if h.name == "content-length" {
            // Deliberately reject even identical duplicates: no downstream ambiguity.
            if cl.is_some() || h.value.is_empty() || !h.value.iter().all(u8::is_ascii_digit) {
                return Err(Error::new(
                    "HPE_UNEXPECTED_CONTENT_LENGTH",
                    "invalid or duplicate content-length",
                ));
            }
            cl = Some(
                h.value
                    .iter()
                    .try_fold(0u64, |n, b| {
                        n.checked_mul(10)?.checked_add((b - b'0') as u64)
                    })
                    .ok_or(invalid("content-length overflow"))?,
            );
        }
        if h.name == "transfer-encoding" {
            if te || !h.value.eq_ignore_ascii_case(b"chunked") {
                return Err(Error::new(
                    "HPE_INVALID_TRANSFER_ENCODING",
                    "unsupported or duplicate transfer coding",
                ));
            }
            te = true;
        }
    }
    if cl.is_some() && te {
        return Err(Error::new(
            "HPE_UNEXPECTED_CONTENT_LENGTH",
            "transfer-encoding with content-length",
        ));
    }
    if te && head.version == 0 {
        return Err(invalid("transfer-encoding on HTTP/1.0"));
    }
    Ok((cl, te))
}
impl Decoder {
    pub fn new(mode: Mode, limits: Limits) -> Self {
        Self {
            mode,
            limits,
            state: State::Head,
            head_request: false,
            connect_request: false,
            keep_alive: false,
        }
    }
    /// Must precede response parsing. Pipelining is intentionally not provided.
    pub fn response_to(&mut self, method: &str) {
        self.head_request = method == "HEAD";
        self.connect_request = method == "CONNECT";
    }
    pub fn reusable(&self) -> bool {
        self.state == State::Done && self.keep_alive
    }
    pub fn reset(&mut self) -> Result<()> {
        if !self.reusable() {
            return Err(invalid("connection is not reusable"));
        }
        self.state = State::Head;
        Ok(())
    }
    pub fn receive<'a>(&mut self, input: &'a [u8]) -> Result<Step<'a>> {
        let result = self.receive_inner(input);
        if result.is_err() {
            self.state = State::Failed;
        }
        result
    }
    fn receive_inner<'a>(&mut self, input: &'a [u8]) -> Result<Step<'a>> {
        let mut step = Step {
            consumed: 0,
            event: None,
        };
        match self.state {
            State::Head => {
                let end = boundary(input);
                let n = end.unwrap_or(input.len());
                if n > self.limits.head_bytes {
                    return Err(Error::new("HPE_HEADER_OVERFLOW", "head size limit"));
                }
                lines_valid(&input[..n], self.limits.line_bytes)?;
                let Some(end) = end else { return Ok(step) };
                let mut slots = [httparse::EMPTY_HEADER; 256];
                let mut head = if self.mode == Mode::Response {
                    let mut parsed = httparse::Response::new(&mut slots);
                    parsed
                        .parse(&input[..end])
                        .map_err(|_| invalid("invalid response head"))?;
                    Head {
                        method: String::new(),
                        target: String::new(),
                        status: parsed.code.ok_or(invalid("missing status"))?,
                        version: parsed.version.unwrap_or(1),
                        headers: headers(parsed.headers, self.limits)?,
                        keep_alive: false,
                    }
                } else {
                    let mut parsed = httparse::Request::new(&mut slots);
                    parsed
                        .parse(&input[..end])
                        .map_err(|_| invalid("invalid request head"))?;
                    Head {
                        method: parsed.method.ok_or(invalid("missing method"))?.into(),
                        target: parsed.path.ok_or(invalid("missing target"))?.into(),
                        status: 0,
                        version: parsed.version.unwrap_or(1),
                        headers: headers(parsed.headers, self.limits)?,
                        keep_alive: false,
                    }
                };
                if self.mode == Mode::Request
                    && head.version == 1
                    && (head.headers.iter().filter(|h| h.name == "host").count() != 1
                        || head.get("host").is_none_or(|v| v.is_empty()))
                {
                    return Err(invalid("HTTP/1.1 requires exactly one Host"));
                }
                let (cl, te) = lengths(&head)?;
                head.keep_alive = !head.token("connection", "close")
                    && (head.version == 1 || head.token("connection", "keep-alive"));
                self.keep_alive = head.keep_alive;
                step.consumed = end;
                if self.mode == Mode::Response
                    && (100..200).contains(&head.status)
                    && head.status != 101
                {
                    if cl.is_some() || te {
                        return Err(invalid("informational response has body framing"));
                    }
                    step.event = Some(Event::Informational(head));
                    return Ok(step);
                }
                self.state = if self.mode == Mode::Response
                    && (head.status == 101
                        || (self.connect_request && (200..300).contains(&head.status)))
                {
                    self.keep_alive = false;
                    State::Upgrade
                } else if self.mode == Mode::Response
                    && (self.head_request || head.status == 204 || head.status == 304)
                {
                    State::End
                } else if te {
                    State::ChunkSize
                } else if let Some(n) = cl {
                    if n == 0 { State::End } else { State::Fixed(n) }
                } else if self.mode == Mode::Request {
                    State::End
                } else {
                    self.keep_alive = false;
                    State::Eof
                };
                if self.mode == Mode::Response && head.status == 204 && (cl.is_some() || te) {
                    return Err(invalid("204 has body framing"));
                }
                step.event = Some(Event::Head(head));
            }
            State::Fixed(left) | State::Chunk(left) => {
                let n = input.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                if n > 0 {
                    step.consumed = n;
                    step.event = Some(Event::Body(&input[..n]));
                    self.state = match self.state {
                        State::Fixed(_) => {
                            if left == n as u64 {
                                State::End
                            } else {
                                State::Fixed(left - n as u64)
                            }
                        }
                        _ => {
                            if left == n as u64 {
                                State::ChunkEnd
                            } else {
                                State::Chunk(left - n as u64)
                            }
                        }
                    };
                }
            }
            State::ChunkSize => {
                let end = input.windows(2).position(|w| w == b"\r\n");
                let n = end.unwrap_or(input.len());
                if n > self.limits.line_bytes {
                    return Err(invalid("chunk line limit"));
                }
                let Some(n) = end else { return Ok(step) };
                let digits = input[..n].split(|b| *b == b';').next().unwrap();
                if digits.is_empty() || !digits.iter().all(u8::is_ascii_hexdigit) {
                    return Err(invalid("invalid chunk size"));
                }
                if input[..n].iter().any(|b| *b < 32 || *b == 127) {
                    return Err(invalid("invalid chunk extension"));
                }
                let size = digits
                    .iter()
                    .try_fold(0u64, |v, b| {
                        v.checked_mul(16)?
                            .checked_add((*b as char).to_digit(16)? as u64)
                    })
                    .ok_or(invalid("chunk overflow"))?;
                self.state = if size == 0 {
                    State::Trailers
                } else {
                    State::Chunk(size)
                };
                step.consumed = n + 2;
            }
            State::ChunkEnd => {
                if input.len() >= 2 {
                    if &input[..2] != b"\r\n" {
                        return Err(invalid("missing chunk CRLF"));
                    }
                    step.consumed = 2;
                    self.state = State::ChunkSize;
                }
            }
            State::Trailers => {
                if input.starts_with(b"\r\n") {
                    step.consumed = 2;
                    self.state = State::End;
                    return Ok(step);
                }
                let end = boundary(input);
                let n = end.unwrap_or(input.len());
                if n > self.limits.head_bytes {
                    return Err(invalid("trailer size limit"));
                }
                lines_valid(&input[..n], self.limits.line_bytes)?;
                let Some(n) = end else { return Ok(step) };
                let mut slots = [httparse::EMPTY_HEADER; 256];
                let httparse::Status::Complete((_, raw)) =
                    httparse::parse_headers(&input[..n], &mut slots)
                        .map_err(|_| invalid("invalid trailers"))?
                else {
                    return Err(invalid("incomplete trailers"));
                };
                let trailers = headers(raw, self.limits)?;
                if trailers.iter().any(|h| {
                    matches!(
                        h.name.as_str(),
                        "content-length"
                            | "transfer-encoding"
                            | "host"
                            | "connection"
                            | "trailer"
                            | "upgrade"
                    )
                }) {
                    return Err(invalid("forbidden trailer"));
                }
                step.consumed = n;
                step.event = Some(Event::Trailers(trailers));
                self.state = State::End;
            }
            State::Eof => {
                if !input.is_empty() {
                    step.consumed = input.len();
                    step.event = Some(Event::Body(input));
                }
            }
            State::End => {
                self.state = State::Done;
                step.event = Some(Event::End);
            }
            State::Upgrade => {
                self.state = State::Done;
                step.event = Some(Event::Upgrade);
            }
            State::Done => {}
            State::Failed => return Err(invalid("failed connection")),
        }
        Ok(step)
    }
    pub fn eof(&mut self) -> Result<()> {
        if self.state == State::Eof {
            self.state = State::End;
            Ok(())
        } else if matches!(self.state, State::End | State::Done) {
            Ok(())
        } else {
            self.state = State::Failed;
            Err(Error::new("UND_ERR_SOCKET", "unexpected EOF"))
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub enum BodyLength {
    Empty,
    Known(u64),
    Chunked,
}
/// Caller-owned wire buffer, reused across commands. No body buffering.
pub struct Encoder {
    left: BodyLength,
    finished: bool,
}
impl Encoder {
    pub fn start(head: &Head, body: BodyLength, out: &mut Vec<u8>) -> Result<Self> {
        // Validate everything before mutating the output.
        for h in &head.headers {
            http::header::HeaderName::from_bytes(h.name.as_bytes())
                .map_err(|_| invalid("invalid header name"))?;
            http::HeaderValue::from_bytes(&h.value).map_err(|_| invalid("invalid header value"))?;
        }
        let (cl, te) = lengths(head)?;
        let expect_cl = match body {
            BodyLength::Known(n) => Some(n),
            BodyLength::Empty => Some(0),
            BodyLength::Chunked => None,
        };
        if (cl.is_some() && cl != expect_cl)
            || (te && !matches!(body, BodyLength::Chunked))
            || (cl.is_some() && matches!(body, BodyLength::Chunked))
        {
            return Err(invalid("body length conflicts with headers"));
        }
        if head.status == 0 {
            http::Method::from_bytes(head.method.as_bytes())
                .map_err(|_| invalid("invalid method"))?;
            if head.target.is_empty() || head.target.bytes().any(|b| b <= 32 || b == 127) {
                return Err(invalid("invalid request target"));
            }
            write!(out, "{} {} HTTP/1.1\r\n", head.method, head.target).unwrap();
        } else {
            let status =
                http::StatusCode::from_u16(head.status).map_err(|_| invalid("invalid status"))?;
            write!(
                out,
                "HTTP/1.1 {} {}\r\n",
                head.status,
                status.canonical_reason().unwrap_or("")
            )
            .unwrap();
        }
        for h in &head.headers {
            out.extend_from_slice(h.name.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(&h.value);
            out.extend_from_slice(b"\r\n");
        }
        if cl.is_none() && !te {
            match body {
                BodyLength::Known(n) => write!(out, "content-length: {n}\r\n").unwrap(),
                BodyLength::Chunked => out.extend_from_slice(b"transfer-encoding: chunked\r\n"),
                BodyLength::Empty => {}
            }
        }
        out.extend_from_slice(b"\r\n");
        Ok(Self {
            left: body,
            finished: false,
        })
    }
    pub fn body(&mut self, bytes: &[u8], out: &mut Vec<u8>) -> Result<()> {
        if self.finished {
            return Err(invalid("body already finished"));
        }
        match self.left {
            BodyLength::Empty => {
                if !bytes.is_empty() {
                    return Err(invalid("unexpected body"));
                }
            }
            BodyLength::Known(n) => {
                if bytes.len() as u64 > n {
                    return Err(invalid("body exceeds content-length"));
                }
                out.extend_from_slice(bytes);
                self.left = BodyLength::Known(n - bytes.len() as u64);
            }
            BodyLength::Chunked => {
                if !bytes.is_empty() {
                    write!(out, "{:x}\r\n", bytes.len()).unwrap();
                    out.extend_from_slice(bytes);
                    out.extend_from_slice(b"\r\n");
                }
            }
        }
        Ok(())
    }
    pub fn finish(&mut self, trailers: &[Header], out: &mut Vec<u8>) -> Result<()> {
        if self.finished || matches!(self.left, BodyLength::Known(1..)) {
            return Err(invalid("incomplete or already finished body"));
        }
        if !trailers.is_empty() && !matches!(self.left, BodyLength::Chunked) {
            return Err(invalid("trailers require chunked body"));
        }
        for h in trailers {
            if matches!(
                h.name.as_str(),
                "content-length"
                    | "transfer-encoding"
                    | "host"
                    | "connection"
                    | "trailer"
                    | "upgrade"
            ) {
                return Err(invalid("forbidden trailer"));
            }
            http::header::HeaderName::from_bytes(h.name.as_bytes())
                .map_err(|_| invalid("invalid trailer"))?;
            http::HeaderValue::from_bytes(&h.value).map_err(|_| invalid("invalid trailer"))?;
        }
        if matches!(self.left, BodyLength::Chunked) {
            out.extend_from_slice(b"0\r\n");
            for h in trailers {
                out.extend_from_slice(h.name.as_bytes());
                out.extend_from_slice(b": ");
                out.extend_from_slice(&h.value);
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(b"\r\n");
        }
        self.finished = true;
        Ok(())
    }
}
