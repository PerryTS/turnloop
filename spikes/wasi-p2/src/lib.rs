#![deny(unsafe_op_in_unsafe_fn)]
//! Bounded WASI p2 driver experiment. Fixed capacity, one active operation per handle.
//! The generated WASI list bindings allocate on poll/read; see the allocation gate.
use wasi::clocks::monotonic_clock as clock;
use wasi::io::{
    poll::{self, Pollable},
    streams::{InputStream, OutputStream, StreamError},
};
use wasi::sockets::{
    instance_network::instance_network,
    network::{ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress},
    tcp::TcpSocket,
    tcp_create_socket::create_tcp_socket,
};

pub const CAPACITY: usize = 256;
pub const BYTES: usize = 257;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    Timer,
    Error(ErrorCode),
    Listening(u16),
    Connected,
    Accepted(usize),
    Read(usize),
    Wrote(usize),
    Eof,
    Cancelled,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub token: u64,
    pub result: ResultKind,
}
#[derive(Clone, Copy, Debug)]
pub enum Timeout {
    Now,
    After(u64),
    Forever,
}
#[derive(Clone, Copy)]
enum Operation {
    Bind,
    Listen,
    Connect,
    Accept,
    Read,
    Write(usize),
}
// Rust drops fields in declaration order: subscriptions before streams before sockets.
struct Streams {
    read_poll: Pollable,
    write_poll: Pollable,
    input: InputStream,
    output: OutputStream,
}
struct Socket {
    streams: Option<Streams>,
    poll: Pollable,
    socket: TcpSocket,
}
enum Resource {
    Timer { poll: Pollable, at: u64 },
    Socket(Socket),
}
struct Entry {
    resource: Resource,
    op: Option<Operation>,
    token: u64,
    closing: bool,
    delivered: bool,
    data: [u8; BYTES],
}
pub struct Driver {
    _owner: std::marker::PhantomData<std::rc::Rc<()>>,
    entries: Vec<Option<Entry>>,
    queued: Vec<Completion>,
    pub turns: u64,
    pub waits: u64,
    pub io_attempts: u64,
}
fn addr(port: u16) -> IpSocketAddress {
    IpSocketAddress::Ipv4(Ipv4SocketAddress {
        port,
        address: (127, 0, 0, 1),
    })
}
fn streams(input: InputStream, output: OutputStream) -> Streams {
    Streams {
        read_poll: input.subscribe(),
        write_poll: output.subscribe(),
        input,
        output,
    }
}
impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}
impl Driver {
    pub fn new() -> Self {
        Self {
            _owner: std::marker::PhantomData,
            entries: (0..CAPACITY).map(|_| None).collect(),
            queued: Vec::with_capacity(CAPACITY * 2),
            turns: 0,
            waits: 0,
            io_attempts: 0,
        }
    }
    fn insert(&mut self, entry: Entry) -> Result<usize, ErrorCode> {
        let id = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(ErrorCode::OutOfMemory)?;
        self.entries[id] = Some(entry);
        Ok(id)
    }
    pub fn timer(&mut self, at: u64, token: u64) -> Result<usize, ErrorCode> {
        self.insert(Entry {
            resource: Resource::Timer {
                poll: clock::subscribe_instant(at),
                at,
            },
            op: None,
            token,
            closing: false,
            delivered: false,
            data: [0; BYTES],
        })
    }
    fn socket(&mut self, port: u16, token: u64, listen: bool) -> Result<usize, ErrorCode> {
        let socket = create_tcp_socket(IpAddressFamily::Ipv4)?;
        if listen {
            socket.start_bind(&instance_network(), addr(port))?;
        } else {
            socket.start_connect(&instance_network(), addr(port))?;
        }
        let poll = socket.subscribe();
        self.insert(Entry {
            resource: Resource::Socket(Socket {
                streams: None,
                poll,
                socket,
            }),
            op: Some(if listen {
                Operation::Bind
            } else {
                Operation::Connect
            }),
            token,
            closing: false,
            delivered: false,
            data: [0; BYTES],
        })
    }
    pub fn listen(&mut self, token: u64) -> Result<usize, ErrorCode> {
        self.socket(0, token, true)
    }
    pub fn connect(&mut self, port: u16, token: u64) -> Result<usize, ErrorCode> {
        self.socket(port, token, false)
    }
    fn submit(&mut self, id: usize, token: u64, op: Operation) -> Result<(), ErrorCode> {
        let e = self
            .entries
            .get_mut(id)
            .and_then(Option::as_mut)
            .ok_or(ErrorCode::InvalidArgument)?;
        if e.closing || e.op.is_some() || !matches!(e.resource, Resource::Socket(_)) {
            return Err(ErrorCode::InvalidState);
        }
        e.token = token;
        e.op = Some(op);
        Ok(())
    }
    pub fn accept(&mut self, id: usize, token: u64) -> Result<(), ErrorCode> {
        self.submit(id, token, Operation::Accept)
    }
    pub fn read(&mut self, id: usize, token: u64) -> Result<(), ErrorCode> {
        self.submit(id, token, Operation::Read)
    }
    pub fn write(&mut self, id: usize, bytes: &[u8], token: u64) -> Result<(), ErrorCode> {
        if bytes.len() > BYTES {
            return Err(ErrorCode::InvalidArgument);
        }
        self.submit(id, token, Operation::Write(bytes.len()))?;
        if let Some(e) = &mut self.entries[id] {
            e.data[..bytes.len()].copy_from_slice(bytes);
        }
        Ok(())
    }
    pub fn data(&self, id: usize) -> Option<&[u8; BYTES]> {
        self.entries.get(id)?.as_ref().map(|e| &e.data)
    }
    pub fn cancel(&mut self, id: usize) -> bool {
        let Some(e) = self.entries.get_mut(id).and_then(Option::as_mut) else {
            return false;
        };
        if e.closing {
            return false;
        }
        let timer = matches!(e.resource, Resource::Timer { .. });
        if timer || e.op.take().is_some() {
            self.queued.push(Completion {
                token: e.token,
                result: ResultKind::Cancelled,
            });
            if timer {
                e.closing = true;
            }
            true
        } else {
            false
        }
    }
    pub fn close(&mut self, id: usize, token: u64) -> bool {
        if !self
            .entries
            .get(id)
            .and_then(Option::as_ref)
            .is_some_and(|e| !e.closing)
        {
            return false;
        }
        self.cancel(id);
        if let Some(e) = &mut self.entries[id] {
            e.closing = true;
        }
        self.queued.push(Completion {
            token,
            result: ResultKind::Closed,
        });
        true
    }
    fn progress(&mut self) -> Result<(), ErrorCode> {
        for id in 0..CAPACITY {
            let Some(e) = &mut self.entries[id] else {
                continue;
            };
            if e.closing {
                continue;
            }
            let mut accepted = None;
            let result = match &mut e.resource {
                Resource::Timer { at, .. } => {
                    if clock::now() >= *at {
                        e.closing = true;
                        Some(ResultKind::Timer)
                    } else {
                        None
                    }
                }
                Resource::Socket(s) => {
                    let Some(op) = e.op else { continue };
                    self.io_attempts += 1;
                    let result: Result<Option<ResultKind>, ErrorCode> = (|| match op {
                        Operation::Bind => {
                            s.socket.finish_bind()?;
                            s.socket.set_listen_backlog_size(128)?;
                            s.socket.start_listen()?;
                            e.op = Some(Operation::Listen);
                            Ok(None)
                        }
                        Operation::Listen => {
                            s.socket.finish_listen()?;
                            match s.socket.local_address()? {
                                IpSocketAddress::Ipv4(a) => Ok(Some(ResultKind::Listening(a.port))),
                                _ => Err(ErrorCode::InvalidState),
                            }
                        }
                        Operation::Connect => {
                            let (input, output) = s.socket.finish_connect()?;
                            s.streams = Some(streams(input, output));
                            Ok(Some(ResultKind::Connected))
                        }
                        Operation::Accept => {
                            let (socket, input, output) = s.socket.accept()?;
                            accepted = Some(Socket {
                                streams: Some(streams(input, output)),
                                poll: socket.subscribe(),
                                socket,
                            });
                            Ok(None)
                        }
                        Operation::Read => {
                            let st = s.streams.as_ref().ok_or(ErrorCode::InvalidState)?;
                            match st.input.read(BYTES as u64) {
                                Ok(bytes) if bytes.is_empty() => Ok(None),
                                Ok(bytes) => {
                                    e.data[..bytes.len()].copy_from_slice(&bytes);
                                    Ok(Some(ResultKind::Read(bytes.len())))
                                }
                                Err(StreamError::Closed) => Ok(Some(ResultKind::Eof)),
                                Err(_) => Err(ErrorCode::Unknown),
                            }
                        }
                        Operation::Write(len) => {
                            let st = s.streams.as_ref().ok_or(ErrorCode::InvalidState)?;
                            if len == 0 {
                                return Ok(Some(ResultKind::Wrote(0)));
                            }
                            let n = (st.output.check_write().map_err(|_| ErrorCode::Unknown)?
                                as usize)
                                .min(len);
                            if n == 0 {
                                return Ok(None);
                            }
                            st.output
                                .write(&e.data[..n])
                                .map_err(|_| ErrorCode::Unknown)?;
                            st.output.flush().map_err(|_| ErrorCode::Unknown)?;
                            Ok(Some(ResultKind::Wrote(n)))
                        }
                    })();
                    match result {
                        Ok(r) => r,
                        Err(ErrorCode::WouldBlock) => None,
                        Err(err) => Some(ResultKind::Error(err)),
                    }
                }
            };
            let token = e.token;
            if let Some(r) = result {
                e.op = None;
                self.queued.push(Completion { token, result: r });
            }
            if let Some(socket) = accepted {
                let conn = self.insert(Entry {
                    resource: Resource::Socket(socket),
                    op: None,
                    token: 0,
                    closing: false,
                    delivered: false,
                    data: [0; BYTES],
                });
                if let Some(e) = &mut self.entries[id] {
                    e.op = None;
                }
                self.queued.push(Completion {
                    token,
                    result: match conn {
                        Ok(id) => ResultKind::Accepted(id),
                        Err(err) => ResultKind::Error(err),
                    },
                });
            }
        }
        Ok(())
    }
    /// Only `poll::poll` can wait, and it is called at most once. Queued work skips it.
    /// Resources marked closing survive until the *following* turn, after delivery.
    pub fn turn(&mut self, timeout: Timeout, out: &mut Vec<Completion>) -> Result<(), ErrorCode> {
        self.turns += 1;
        out.clear();
        for slot in &mut self.entries {
            if slot.as_ref().is_some_and(|e| e.delivered) {
                *slot = None;
            }
        }
        self.progress()?;
        if self.queued.is_empty() && !matches!(timeout, Timeout::Now) {
            let deadline = match timeout {
                Timeout::After(ns) => clock::now().saturating_add(ns),
                _ => u64::MAX,
            };
            let timer = clock::subscribe_instant(deadline);
            let mut refs = [&timer; CAPACITY + 1];
            let mut len = 1;
            for e in self.entries.iter().flatten().filter(|e| !e.closing) {
                let p = match &e.resource {
                    Resource::Timer { poll, .. } => Some(poll),
                    Resource::Socket(s) => match e.op {
                        Some(Operation::Read) => s.streams.as_ref().map(|s| &s.read_poll),
                        Some(Operation::Write(_)) => s.streams.as_ref().map(|s| &s.write_poll),
                        Some(_) => Some(&s.poll),
                        None => None,
                    },
                };
                if let Some(p) = p {
                    refs[len] = p;
                    len += 1;
                }
            }
            self.waits += 1;
            let ready = poll::poll(&refs[..len]);
            assert!(!ready.is_empty());
            self.progress()?;
        }
        out.append(&mut self.queued);
        for e in self.entries.iter_mut().flatten() {
            if e.closing {
                e.delivered = true;
            }
        }
        Ok(())
    }
}
