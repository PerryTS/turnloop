//! Async WebSocket upgrade and framed streams over turnloop TCP/pipes/TLS.
use crate::{ClientHandshake, Connection, Message, Role, WebSocketConfig};
use std::io;
use turnloop_http::{
    asynchronous::Http1,
    http1::{BodyLength, Event, Mode},
};
use turnloop_io::{Backend, CloseOnDrop, ExecutorHandle, Instant, Stream};
/// A framed connection with one retained input/output buffer. A cancelled frame
/// operation closes the stream, preventing partial frames from being replayed.
pub struct WebSocketStream<S> {
    stream: Option<S>,
    core: Connection,
    input: Vec<u8>,
    output: Vec<u8>,
    closed: bool,
}
impl<S: Stream> WebSocketStream<S> {
    /// Complete a client HTTP upgrade. The host supplies a fresh random nonce.
    pub async fn connect<B: Backend>(
        stream: S,
        host: &str,
        target: &str,
        nonce: [u8; 16],
        protocols: Vec<String>,
        executor: &ExecutorHandle<B>,
        deadline: Instant,
    ) -> io::Result<(Self, Option<String>)> {
        turnloop_io::deadline(executor, deadline, async {
            let (handshake, head) =
                ClientHandshake::new(host, target, nonce, protocols).map_err(io::Error::other)?;
            let mut http = Http1::new(stream, Mode::Response);
            http.response_to("GET");
            http.send_head(&head, BodyLength::Empty).await?;
            http.finish_body(&[]).await?;
            let head = http.head().await?;
            let protocol = handshake.verify(&head).map_err(io::Error::other)?;
            http.event(|event| {
                if matches!(event, Event::Upgrade) {
                    Ok(())
                } else {
                    Err(io::Error::other("missing upgrade boundary"))
                }
            })
            .await?;
            let (stream, input) = http.into_upgrade()?;
            Ok((
                Self::from_upgrade(stream, input, Role::Client, Default::default()),
                protocol,
            ))
        })
        .await
    }
    /// Accept an HTTP/1 WebSocket upgrade, preserving coalesced first-frame bytes.
    pub async fn accept<B: Backend>(
        stream: S,
        protocols: &[&str],
        executor: &ExecutorHandle<B>,
        deadline: Instant,
    ) -> io::Result<(Self, Option<String>)> {
        turnloop_io::deadline(executor, deadline, async {
            let mut http = Http1::new(stream, Mode::Request);
            let request = http.head().await?;
            let (head, protocol) = crate::accept(&request, protocols).map_err(io::Error::other)?;
            http.event(|event| {
                if matches!(event, Event::Upgrade | Event::End) {
                    Ok(())
                } else {
                    Err(io::Error::other("missing upgrade boundary"))
                }
            })
            .await?;
            http.send_head(&head, BodyLength::Empty).await?;
            http.finish_body(&[]).await?;
            let (stream, input) = http.into_parts()?;
            Ok((
                Self::from_upgrade(stream, input, Role::Server, Default::default()),
                protocol,
            ))
        })
        .await
    }
    pub fn from_upgrade(
        stream: S,
        mut input: Vec<u8>,
        role: Role,
        config: WebSocketConfig,
    ) -> Self {
        if input.capacity() < 65536 {
            input.reserve(65536 - input.len());
        }
        Self {
            stream: Some(stream),
            core: Connection::new(role, config),
            input,
            output: Vec::with_capacity(65536),
            closed: false,
        }
    }
    pub async fn send(&mut self, message: Message) -> io::Result<()> {
        let mut guard = CloseOnDrop::new(&mut self.stream);
        self.core
            .send(message, &mut self.output)
            .map_err(io::Error::other)?;
        turnloop_io::write_all(guard.stream()?, &self.output).await?;
        self.output.clear();
        guard.commit();
        Ok(())
    }
    /// Receive one message and deliver automatic pong/close replies first.
    pub async fn receive(&mut self) -> io::Result<Option<Message>> {
        if self.closed {
            return Ok(None);
        }
        let mut guard = CloseOnDrop::new(&mut self.stream);
        loop {
            let step = self
                .core
                .receive(&self.input, &mut self.output)
                .map_err(io::Error::other)?;
            self.input.drain(..step.consumed);
            match self.core.flush(&mut self.output) {
                Ok(()) => {}
                Err(crate::Error::ConnectionClosed | crate::Error::AlreadyClosed) => {
                    self.closed = true
                }
                Err(e) => return Err(io::Error::other(e)),
            }
            turnloop_io::write_all(guard.stream()?, &self.output).await?;
            self.output.clear();
            if let Some(message) = step.message {
                guard.commit();
                return Ok(Some(message));
            }
            if self.closed {
                guard.commit();
                return Ok(None);
            }
            let mut bytes = [0; 16384];
            let n = turnloop_io::read(guard.stream()?, &mut bytes).await?;
            if n == 0 {
                let code = self.core.eof();
                self.closed = true;
                if code == Some(1006) {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                guard.commit();
                return Ok(None);
            }
            self.input.extend_from_slice(&bytes[..n]);
        }
    }
    /// Send close, await its peer acknowledgement under the supplied deadline,
    /// then close the underlying stream (including TLS close_notify when present).
    pub async fn close<B: Backend>(
        &mut self,
        executor: &ExecutorHandle<B>,
        deadline: Instant,
    ) -> io::Result<()> {
        let result = turnloop_io::deadline(executor, deadline, async {
            self.send(Message::Close(None)).await?;
            while let Some(message) = self.receive().await? {
                if matches!(message, Message::Close(_)) {
                    break;
                }
            }
            if let Some(stream) = &mut self.stream {
                turnloop_io::close(stream).await?;
            }
            self.closed = true;
            Ok(())
        })
        .await;
        if result.is_err() {
            self.stream.take();
        }
        result
    }
}
