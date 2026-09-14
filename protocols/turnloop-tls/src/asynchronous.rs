//! TLS over turnloop streams, using the same unbuffered client/server cores.
use crate::{
    Client, ClientConfig, ConnectionState, Server, ServerConfig, UnbufferedStatus, rustls,
};
use std::{
    future::poll_fn,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use turnloop_io::{AsyncRead, AsyncWrite, Backend, ExecutorHandle, Instant, Stream};

/// Certificate verification and wall time supplied by the embedding host.
#[derive(Clone)]
pub struct ClientTls {
    pub config: ClientConfig,
    pub server_name: rustls::pki_types::ServerName<'static>,
    pub unix_seconds: u64,
}
/// A stream that can cross a protocol's explicit STARTTLS boundary once.
/// TLS state is allocated at connection setup, never per operation.
pub enum Transport<S> {
    Plain(S),
    Tls(Box<TlsStream<S>>),
    Closed,
}
impl<S: Stream> Transport<S> {
    pub async fn upgrade<B: Backend>(&mut self, tls: &ClientTls, executor: &ExecutorHandle<B>, at: Instant) -> io::Result<()> {
        let Self::Plain(stream) = std::mem::replace(self, Self::Closed) else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "TLS already negotiated"));
        };
        *self = Self::Tls(Box::new(TlsStream::connect(stream, &tls.config, tls.server_name.clone(), executor, at, tls.unix_seconds).await?));
        Ok(())
    }
    pub fn is_tls(&self) -> bool { matches!(self, Self::Tls(_)) }
    pub fn get_ref(&self) -> Option<&S> {
        match self { Self::Plain(s) => Some(s), Self::Tls(s) => Some(s.get_ref()), Self::Closed => None }
    }
}
impl<S: Stream> AsyncRead for Transport<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &mut [u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, bytes),
            Self::Tls(s) => Pin::new(&mut **s).poll_read(cx, bytes),
            Self::Closed => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
}
impl<S: Stream> AsyncWrite for Transport<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, bytes),
            Self::Tls(s) => Pin::new(&mut **s).poll_write(cx, bytes),
            Self::Closed => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(&mut **s).poll_flush(cx),
            Self::Closed => Poll::Ready(Err(io::ErrorKind::NotConnected.into())),
        }
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_close(cx),
            Self::Tls(s) => Pin::new(&mut **s).poll_close(cx),
            Self::Closed => Poll::Ready(Ok(())),
        }
    }
}

trait Endpoint {
    type Data;
    fn process<'c, 'i>(
        &'c mut self,
        input: &'i mut [u8],
        now: u64,
    ) -> UnbufferedStatus<'c, 'i, Self::Data>;
}
impl Endpoint for Client {
    type Data = rustls::client::ClientConnectionData;
    fn process<'c, 'i>(
        &'c mut self,
        input: &'i mut [u8],
        now: u64,
    ) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process(input, now)
    }
}
impl Endpoint for Server {
    type Data = rustls::server::ServerConnectionData;
    fn process<'c, 'i>(
        &'c mut self,
        input: &'i mut [u8],
        now: u64,
    ) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process(input, now)
    }
}
enum Session {
    Client(Client),
    Server(Server),
}
struct Buffers {
    input: Vec<u8>,
    output: Vec<u8>,
    plain: Vec<u8>,
    plain_at: usize,
    output_at: usize,
    output_len: usize,
    transmitted: bool,
    peer_closed: bool,
    close_sent: bool,
}
impl Buffers {
    fn new() -> Self {
        Self {
            input: Vec::with_capacity(65536),
            output: vec![0; 65536],
            plain: Vec::with_capacity(65536),
            plain_at: 0,
            output_at: 0,
            output_len: 0,
            transmitted: false,
            peer_closed: false,
            close_sent: false,
        }
    }
}
enum Intent<'a> {
    Handshake,
    Read,
    Write(&'a [u8]),
    Close,
}
enum Action {
    Progress,
    Read,
    Flush,
    Ready(usize),
}
fn step<E: Endpoint>(
    tls: &mut E,
    b: &mut Buffers,
    time: u64,
    intent: &Intent<'_>,
) -> io::Result<Action> {
    let status = tls.process(&mut b.input, time);
    let mut discard = status.discard;
    let action = match status.state.map_err(io::Error::other)? {
        ConnectionState::EncodeTlsData(mut encode) => {
            let n = encode
                .encode(&mut b.output[b.output_len..])
                .map_err(io::Error::other)?;
            b.output_len += n;
            Action::Progress
        }
        ConnectionState::TransmitTlsData(tx) => {
            if b.transmitted {
                tx.done();
                b.transmitted = false;
                Action::Progress
            } else {
                Action::Flush
            }
        }
        ConnectionState::ReadTraffic(mut read) => {
            // rustls currently owns decrypted records. Retain partial AsyncRead
            // results because the record view cannot outlive this state.
            if let Some(record) = read.next_record() {
                let record = record.map_err(io::Error::other)?;
                discard += record.discard;
                if b.plain_at > 0 {
                    b.plain.drain(..b.plain_at);
                    b.plain_at = 0;
                }
                if record.payload.len() > b.plain.capacity() - b.plain.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "TLS unread plaintext limit",
                    ));
                }
                b.plain.extend_from_slice(record.payload);
            }
            Action::Progress
        }
        ConnectionState::WriteTraffic(mut write) => match intent {
            Intent::Handshake => Action::Ready(0),
            Intent::Read => Action::Read,
            Intent::Write(bytes) => {
                let n = bytes.len().min(16384);
                b.output_len = write
                    .encrypt(&bytes[..n], &mut b.output)
                    .map_err(io::Error::other)?;
                Action::Ready(n)
            }
            Intent::Close => {
                if !b.close_sent {
                    b.output_len = write
                        .queue_close_notify(&mut b.output)
                        .map_err(io::Error::other)?;
                    b.close_sent = true;
                    Action::Flush
                } else {
                    Action::Ready(0)
                }
            }
        },
        ConnectionState::BlockedHandshake => Action::Read,
        ConnectionState::PeerClosed => {
            b.peer_closed = true;
            Action::Progress
        }
        ConnectionState::Closed => {
            b.peer_closed = true;
            Action::Ready(0)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "TLS early data is disabled",
            ));
        }
    };
    b.input.drain(..discard);
    Ok(action)
}
/// An async client or server TLS connection. Construction completes the handshake
/// under a single absolute deadline. Drop aborts the owned transport; `close`
/// flushes close_notify first. Retained storage is bounded per connection.
pub struct TlsStream<S> {
    stream: S,
    session: Session,
    buffers: Buffers,
    unix_seconds: u64,
}
impl<S: Stream> TlsStream<S> {
    pub async fn connect<B: Backend>(
        stream: S,
        config: &ClientConfig,
        name: rustls::pki_types::ServerName<'static>,
        executor: &ExecutorHandle<B>,
        deadline: Instant,
        unix_seconds: u64,
    ) -> io::Result<Self> {
        let mut this = Self {
            stream,
            session: Session::Client(config.connect(name).map_err(io::Error::other)?),
            buffers: Buffers::new(),
            unix_seconds,
        };
        turnloop_io::deadline(
            executor,
            deadline,
            poll_fn(|cx| {
                this.poll_drive(cx, &Intent::Handshake)
                    .map(|r| r.map(|_| ()))
            }),
        )
        .await?;
        Ok(this)
    }
    pub async fn accept<B: Backend>(
        stream: S,
        config: &ServerConfig,
        executor: &ExecutorHandle<B>,
        deadline: Instant,
        unix_seconds: u64,
    ) -> io::Result<Self> {
        let mut this = Self {
            stream,
            session: Session::Server(config.accept().map_err(io::Error::other)?),
            buffers: Buffers::new(),
            unix_seconds,
        };
        turnloop_io::deadline(
            executor,
            deadline,
            poll_fn(|cx| {
                this.poll_drive(cx, &Intent::Handshake)
                    .map(|r| r.map(|_| ()))
            }),
        )
        .await?;
        Ok(this)
    }
    pub fn alpn_protocol(&self) -> Option<&[u8]> {
        match &self.session {
            Session::Client(c) => c.alpn_protocol(),
            Session::Server(s) => s.alpn_protocol(),
        }
    }
    /// Refresh wall time supplied by the embedding host.
    pub fn set_unix_seconds(&mut self, seconds: u64) {
        self.unix_seconds = seconds;
    }
    pub fn get_ref(&self) -> &S {
        &self.stream
    }
    fn poll_output(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let b = &mut self.buffers;
        while b.output_at < b.output_len {
            let n = std::task::ready!(
                Pin::new(&mut self.stream).poll_write(cx, &b.output[b.output_at..b.output_len])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            b.output_at += n;
        }
        std::task::ready!(Pin::new(&mut self.stream).poll_flush(cx))?;
        b.output_at = 0;
        b.output_len = 0;
        Poll::Ready(Ok(()))
    }
    fn poll_drive(&mut self, cx: &mut Context<'_>, intent: &Intent<'_>) -> Poll<io::Result<usize>> {
        // A pending output phase is always completed before re-entering rustls.
        std::task::ready!(self.poll_output(cx))?;
        loop {
            if matches!(intent, Intent::Read)
                && (!self.buffers.plain.is_empty() || self.buffers.peer_closed)
            {
                return Poll::Ready(Ok(0));
            }
            if matches!(intent, Intent::Close) && self.buffers.close_sent {
                return Poll::Ready(Ok(0));
            }
            let action = match &mut self.session {
                Session::Client(c) => step(c, &mut self.buffers, self.unix_seconds, intent),
                Session::Server(s) => step(s, &mut self.buffers, self.unix_seconds, intent),
            }?;
            match action {
                Action::Progress => {}
                Action::Ready(n) => return Poll::Ready(Ok(n)),
                Action::Flush => {
                    // Set before yielding; the next poll must acknowledge only
                    // after poll_output has observed the terminal write result.
                    self.buffers.transmitted = true;
                    std::task::ready!(self.poll_output(cx))?;
                }
                Action::Read => {
                    let b = &mut self.buffers;
                    let start = b.input.len();
                    let capacity = b.input.capacity();
                    if start == capacity {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "TLS input limit",
                        )));
                    }
                    b.input.resize(capacity, 0);
                    let result = Pin::new(&mut self.stream).poll_read(cx, &mut b.input[start..]);
                    let n = match result {
                        Poll::Ready(Ok(n)) => n,
                        Poll::Ready(Err(e)) => {
                            b.input.truncate(start);
                            return Poll::Ready(Err(e));
                        }
                        Poll::Pending => {
                            b.input.truncate(start);
                            return Poll::Pending;
                        }
                    };
                    b.input.truncate(start + n);
                    if n == 0 {
                        return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                    }
                }
            }
        }
    }
}
impl<S: Stream> AsyncRead for TlsStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let this = self.get_mut();
        std::task::ready!(this.poll_drive(cx, &Intent::Read))?;
        let b = &mut this.buffers;
        let n = bytes.len().min(b.plain.len() - b.plain_at);
        bytes[..n].copy_from_slice(&b.plain[b.plain_at..b.plain_at + n]);
        b.plain_at += n;
        if b.plain_at == b.plain.len() {
            b.plain.clear();
            b.plain_at = 0;
        }
        Poll::Ready(Ok(n))
    }
}
impl<S: Stream> AsyncWrite for TlsStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        self.get_mut().poll_drive(cx, &Intent::Write(bytes))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_output(cx)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.poll_drive(cx, &Intent::Close))?;
        std::task::ready!(this.poll_output(cx))?;
        Pin::new(&mut this.stream).poll_close(cx)
    }
}
