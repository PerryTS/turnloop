//! WebSocket protocol over in-memory Read/Write views, never sockets. HTTP upgrade
//! heads come from turnloop-http; pass unconsumed upgrade bytes to `receive`.
//! permessage-deflate is deliberately not negotiated (tungstenite has no support).
//!
//! # Getting started on turnloop
//! Enable the `turnloop` feature for the `asynchronous` module. The embedding
//! host owns `LocalExecutor` and calls `turn`; adapters await its streams and
//! deadline futures. See the crate README and turnloop-io for ownership, streaming
//! and cancellation examples. Default features retain the sans-I/O API.
#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(all(target_os = "wasi", target_env = "p3"))]
use turnloop_wasi_random as _;

use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    io::{self, Read, Write},
    time::Instant,
};
use tungstenite::protocol::WebSocketContext;
pub use tungstenite::{
    Error, Message,
    protocol::{CloseFrame, Role, WebSocketConfig},
};
use turnloop_http::{
    Error as HttpError,
    http1::{Head, Header},
};
struct Memory<'a> {
    input: &'a [u8],
    output: &'a mut Vec<u8>,
    read: usize,
}
impl Read for Memory<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.input.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = out.len().min(self.input.len());
        out[..n].copy_from_slice(&self.input[..n]);
        self.input = &self.input[n..];
        self.read += n;
        Ok(n)
    }
}
impl Write for Memory<'_> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub struct Connection {
    context: WebSocketContext,
    deadline: Option<Instant>,
    terminal: bool,
    peer_close: Option<u16>,
}
/// One [`Connection::receive`] step: how much of `input` was taken, and the
/// message it completed, if any.
///
/// **`consumed` and `message` are independent**, as they are for
/// `turnloop_http`'s `Step`. `receive` reads input into the connection's own
/// buffer and parses from that buffer before it reads again, so the bytes a
/// message is made of may have been consumed by an earlier call:
///
/// | `consumed` | `message` | meaning |
/// |---|---|---|
/// | `0` | `None` | **stop.** Nothing complete is buffered and the input is empty; read more from the transport before calling again. |
/// | `> 0` | `None` | **keep going.** The input was taken in, all of it, but completes no message yet: a partial frame, or a non-final fragment. The next call returns the stop shape unless more input arrived. |
/// | `> 0` | `Some` | a message, possibly with input left over. |
/// | `0` | `Some` | a message completed from bytes an earlier call consumed - the second of two frames that arrived in one read, say. Normal, and easy to drop. |
///
/// So the loop condition is the same one-liner, `consumed > 0 ||
/// message.is_some()`:
///
/// ```
/// # use turnloop_websocket::*;
/// # let mut connection = Connection::new(Role::Server, WebSocketConfig::default());
/// # let (mut input, mut replies) = (Vec::new(), Vec::new());
/// loop {
///     let step = connection.receive(&input, &mut replies)?;
///     input.drain(..step.consumed);
///     let progressed = step.consumed > 0 || step.message.is_some();
///     if let Some(message) = step.message {
///         let _ = message;
///     }
///     connection.flush(&mut replies)?; // automatic pong/close replies
///     if !progressed {
///         break; // read more bytes from the transport, then continue
///     }
/// }
/// # Ok::<(), Error>(())
/// ```
///
/// Looping while `consumed > 0` **drops messages**: the `0`/`Some` step reads
/// as "stop" and its message is never looked at. Reading the transport
/// whenever `input` is empty **stalls** on the same step: no input is left,
/// yet a message is waiting, and a peer waiting for its answer never sends
/// the bytes that would wake the host. Looping while a message came back is,
/// for this type, safe - a step with no message has always taken in all of
/// its input, so nothing is stranded - but it is not the documented
/// condition, and the one above works for every step type in the workspace.
///
/// `receive` is not idempotent. The `consumed` bytes are in the connection's
/// buffer once it returns, so input that is fed again without being drained
/// is parsed again: a complete message in it is delivered twice, with no
/// error to notice.
pub struct Received {
    pub consumed: usize,
    pub message: Option<Message>,
}
impl Connection {
    pub fn new(role: Role, config: WebSocketConfig) -> Self {
        Self {
            context: WebSocketContext::new(role, Some(config)),
            deadline: None,
            terminal: false,
            peer_close: None,
        }
    }
    /// One decode step. `consumed` and `message` are independent, and
    /// `consumed == 0` does not by itself mean "stop": see [`Received`].
    pub fn receive(&mut self, input: &[u8], output: &mut Vec<u8>) -> Result<Received, Error> {
        if self.terminal {
            return Err(Error::AlreadyClosed);
        }
        let mut memory = Memory {
            input,
            output,
            read: 0,
        };
        let message = match self.context.read(&mut memory) {
            Ok(message) => {
                if let Message::Close(frame) = &message {
                    self.peer_close = Some(frame.as_ref().map_or(1005, |f| u16::from(f.code)));
                    self.deadline = None;
                }
                Some(message)
            }
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => None,
            Err(e) => {
                self.terminal = true;
                return Err(e);
            }
        };
        Ok(Received {
            consumed: memory.read,
            message,
        })
    }
    pub fn send(&mut self, message: Message, output: &mut Vec<u8>) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::AlreadyClosed);
        }
        let mut memory = Memory {
            input: &[],
            output,
            read: 0,
        };
        self.context.write(&mut memory, message)?;
        self.context.flush(&mut memory)
    }
    /// Flush automatic pong/close replies after consuming an incoming message.
    pub fn flush(&mut self, output: &mut Vec<u8>) -> Result<(), Error> {
        if self.terminal {
            return Err(Error::AlreadyClosed);
        }
        let mut memory = Memory {
            input: &[],
            output,
            read: 0,
        };
        let result = self.context.flush(&mut memory);
        if matches!(result, Err(Error::ConnectionClosed | Error::AlreadyClosed)) {
            self.terminal = true;
            self.deadline = None;
        }
        result
    }
    pub fn close(
        &mut self,
        frame: Option<CloseFrame>,
        deadline: Instant,
        output: &mut Vec<u8>,
    ) -> Result<(), Error> {
        let mut memory = Memory {
            input: &[],
            output,
            read: 0,
        };
        self.context.close(&mut memory, frame)?;
        self.deadline = Some(deadline);
        Ok(())
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        if self.terminal { None } else { self.deadline }
    }
    pub fn handle_timeout(&mut self, now: Instant) -> Option<u16> {
        if self.next_timeout().is_some_and(|d| d <= now) {
            self.terminal = true;
            self.deadline = None;
            Some(1006)
        } else {
            None
        }
    }
    pub fn eof(&mut self) -> Option<u16> {
        if self.terminal {
            None
        } else {
            self.terminal = true;
            self.peer_close.or(Some(1006))
        }
    }
}
fn invalid(message: &'static str) -> HttpError {
    HttpError::new("WS_ERR_INVALID_HANDSHAKE", message)
}
fn unique<'a>(head: &'a Head, name: &str) -> Result<&'a [u8], HttpError> {
    let mut values = head
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(name));
    let value = values.next().ok_or(invalid("missing handshake header"))?;
    if values.next().is_some() {
        return Err(invalid("duplicate handshake header"));
    }
    Ok(&value.value)
}
fn protocol_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
pub struct ClientHandshake {
    key: String,
    protocols: Vec<String>,
}
impl ClientHandshake {
    /// The host supplies a fresh cryptographic random nonce.
    pub fn new(
        host: &str,
        target: &str,
        nonce: [u8; 16],
        protocols: Vec<String>,
    ) -> Result<(Self, Head), HttpError> {
        for (i, p) in protocols.iter().enumerate() {
            if !protocol_token(p) || protocols[..i].contains(p) {
                return Err(invalid("invalid or duplicate subprotocol"));
            }
        }
        let key = STANDARD.encode(nonce);
        let mut headers = vec![
            Header::new("host", host),
            Header::new("connection", "Upgrade"),
            Header::new("upgrade", "websocket"),
            Header::new("sec-websocket-version", "13"),
            Header::new("sec-websocket-key", &key),
        ];
        if !protocols.is_empty() {
            headers.push(Header::new("sec-websocket-protocol", protocols.join(", ")));
        }
        Ok((
            Self { key, protocols },
            Head {
                method: "GET".into(),
                target: target.into(),
                status: 0,
                version: 1,
                headers,
                keep_alive: true,
            },
        ))
    }
    pub fn verify(&self, response: &Head) -> Result<Option<String>, HttpError> {
        if response.status != 101
            || response.version != 1
            || !response.token("connection", "upgrade")
            || !unique(response, "upgrade")?.eq_ignore_ascii_case(b"websocket")
        {
            return Err(invalid("invalid upgrade response"));
        }
        if unique(response, "sec-websocket-accept")?
            != tungstenite::handshake::derive_accept_key(self.key.as_bytes()).as_bytes()
        {
            return Err(invalid("invalid accept key"));
        }
        if response.get("sec-websocket-extensions").is_some() {
            return Err(invalid("unsolicited extension"));
        }
        if response.get("sec-websocket-protocol").is_some() {
            let protocol = std::str::from_utf8(unique(response, "sec-websocket-protocol")?)
                .map_err(|_| invalid("invalid protocol"))?;
            if !self.protocols.iter().any(|p| p == protocol) {
                return Err(invalid("unsolicited subprotocol"));
            }
            Ok(Some(protocol.into()))
        } else if !self.protocols.is_empty() {
            Err(invalid("server did not select subprotocol"))
        } else {
            Ok(None)
        }
    }
}
pub fn accept(request: &Head, protocols: &[&str]) -> Result<(Head, Option<String>), HttpError> {
    if request.method != "GET"
        || request.version != 1
        || !request.token("connection", "upgrade")
        || !unique(request, "upgrade")?.eq_ignore_ascii_case(b"websocket")
        || unique(request, "sec-websocket-version")? != b"13"
    {
        return Err(invalid("invalid upgrade request"));
    }
    let key = unique(request, "sec-websocket-key")?;
    let mut decoded = [0; 18];
    if STANDARD
        .decode_slice(key, &mut decoded)
        .map_err(|_| invalid("invalid key"))?
        != 16
    {
        return Err(invalid("invalid key length"));
    }
    let offered = request
        .get("sec-websocket-protocol")
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|_| invalid("invalid protocol"))?
        .unwrap_or("");
    if !offered.is_empty() {
        let mut seen = Vec::new();
        for p in offered.split(',').map(str::trim) {
            if !protocol_token(p) || seen.contains(&p) {
                return Err(invalid("invalid or duplicate subprotocol"));
            }
            seen.push(p);
        }
    }
    let selected = protocols
        .iter()
        .find(|p| offered.split(',').map(str::trim).any(|x| x == **p))
        .map(|p| p.to_string());
    let mut headers = vec![
        Header::new("connection", "Upgrade"),
        Header::new("upgrade", "websocket"),
        Header::new(
            "sec-websocket-accept",
            tungstenite::handshake::derive_accept_key(key),
        ),
    ];
    if let Some(p) = &selected {
        headers.push(Header::new("sec-websocket-protocol", p));
    }
    Ok((
        Head {
            method: String::new(),
            target: String::new(),
            status: 101,
            version: 1,
            headers,
            keep_alive: false,
        },
        selected,
    ))
}
pub fn node_error_code(error: &Error) -> &'static str {
    match error {
        Error::Capacity(_) => "WS_ERR_UNSUPPORTED_MESSAGE_LENGTH",
        Error::Utf8(_) => "WS_ERR_INVALID_UTF8",
        Error::Protocol(_) => "WS_ERR_PROTOCOL_ERROR",
        Error::ConnectionClosed | Error::AlreadyClosed => "WS_ERR_CLOSED",
        _ => "WS_ERR_SOCKET",
    }
}

#[cfg(feature = "turnloop")]
pub mod asynchronous;
#[cfg(feature = "turnloop")]
pub use asynchronous::WebSocketStream;
