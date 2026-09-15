//! Shared transport glue. See the crate README for the adapter ownership contract.
#![deny(unsafe_op_in_unsafe_fn)]
pub use futures_io::{AsyncRead, AsyncWrite};
use std::{
    future::{Future, poll_fn},
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
pub use turnloop::{self, AsyncIo, ExecutorHandle, Instant, backend::Backend};
pub mod dns;
pub mod pool;

/// A TCP, pipe, TLS or host stream with cancellation-safe futures-io operations.
pub trait Stream: AsyncRead + AsyncWrite + Unpin {}
impl<T: AsyncRead + AsyncWrite + Unpin> Stream for T {}

/// Preserve portable error kinds even where no OS error number exists.
pub fn error(e: turnloop::Error) -> io::Error {
    if let Some(code) = e.os {
        return io::Error::from_raw_os_error(code);
    }
    let kind = match e.kind {
        turnloop::ErrorKind::TimedOut => io::ErrorKind::TimedOut,
        turnloop::ErrorKind::Unsupported => io::ErrorKind::Unsupported,
        turnloop::ErrorKind::InvalidInput => io::ErrorKind::InvalidInput,
        turnloop::ErrorKind::Cancelled => io::ErrorKind::Interrupted,
        turnloop::ErrorKind::BrokenPipe => io::ErrorKind::BrokenPipe,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, e)
}
/// Read into caller storage; an empty read means transport EOF.
pub async fn read<S: AsyncRead + Unpin>(stream: &mut S, bytes: &mut [u8]) -> io::Result<usize> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_read(cx, bytes)).await
}
/// Write all bytes and confirm transport delivery before releasing their owner.
pub async fn write_all<S: AsyncWrite + Unpin>(stream: &mut S, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut *stream).poll_write(cx, bytes)).await?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[n..];
    }
    flush(stream).await
}
/// Wait for delivery of accepted writes.
pub async fn flush<S: AsyncWrite + Unpin>(stream: &mut S) -> io::Result<()> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_flush(cx)).await
}
/// Gracefully close after flushing accepted writes.
pub async fn close<S: AsyncWrite + Unpin>(stream: &mut S) -> io::Result<()> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_close(cx)).await
}
/// A stream whose write direction can end while its read direction stays open.
///
/// futures-io's `poll_close` releases the whole transport. A half-close instead
/// sends the peer EOF and keeps reading, which a server needs for a lingering
/// close: closing a socket with unread peer bytes sends RST, and a received RST
/// discards data the peer has not read yet on macOS and Windows. `AsyncIo`,
/// `turnloop_tls::asynchronous::TlsStream` and `Transport` implement it.
pub trait HalfClose: Stream {
    /// Backend of the executor that owns the native handle.
    type Backend: Backend;
    /// The executor owning the native handle, whose timers bound a lingering close.
    /// `None` once the transport has been released.
    fn executor(&self) -> Option<ExecutorHandle<Self::Backend>>;
    /// Flush accepted writes, then end the write direction without closing the
    /// handle (TLS sends close_notify first). Reads continue until the peer's EOF;
    /// later writes fail. Completed shutdowns stay complete on repeated polls.
    /// Transports without a separate write direction return
    /// `io::ErrorKind::Unsupported` (see `AsyncIo::poll_shutdown`).
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}
impl<B: Backend> HalfClose for AsyncIo<B> {
    type Backend = B;
    fn executor(&self) -> Option<ExecutorHandle<B>> {
        Some(AsyncIo::executor(self))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_shutdown(cx)
    }
}
/// Half-close: flush accepted writes and end the write direction, keeping the
/// transport open for reads. [`close`] keeps its full-close semantics.
pub async fn shutdown<S: HalfClose>(stream: &mut S) -> io::Result<()> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_shutdown(cx)).await
}
/// How the discard phase of a lingering close ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LingerEnd {
    /// The peer ended its stream; all its remaining input was read and discarded.
    Eof,
    /// The linger deadline passed before the peer's EOF.
    Deadline,
    /// The transport cannot half-close, so it closed without lingering.
    Unsupported,
    /// The half-close, a discard read or the deadline timer failed (for example
    /// the peer reset the connection); lingering stopped and the stream closed.
    Failed(io::ErrorKind),
}
/// Outcome of a lingering close, including proof of the discarded input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lingered {
    /// Why discarding stopped.
    pub end: LingerEnd,
    /// Non-empty reads performed after the half-close.
    pub reads: u64,
    /// Peer bytes read and discarded after the half-close.
    pub discarded: u64,
}
enum LingerPhase {
    Shutdown,
    Discard,
    Close,
    Done,
}
/// Future for [`linger_close`]. It borrows the stream and caller-retained scratch,
/// allocates nothing, and waits only on the stream's own read and one executor
/// timer, so an idle peer costs no turns before the deadline.
pub struct LingerClose<'a, S: HalfClose> {
    stream: &'a mut S,
    scratch: &'a mut [u8],
    executor: Option<ExecutorHandle<S::Backend>>,
    deadline: Option<Instant>,
    sleep: Option<(Instant, turnloop::Sleep<S::Backend>)>,
    phase: LingerPhase,
    lingered: Lingered,
}
/// Lingering close, as nginx `lingering_close`: flush and half-close, read and
/// discard peer input until its EOF or `deadline`, then close.
///
/// Flush the final response before calling this. Closing with unread peer bytes
/// (a pipelined request, HTTP/2 WINDOW_UPDATE or SETTINGS acknowledgements) sends
/// RST, which can discard the peer's unread copy of that response. `scratch` must
/// be non-empty; reuse a connection's retained input buffer. `None` lingers until
/// the peer's EOF. The deadline bounds the half-close too. Transports that cannot
/// half-close close at once ([`LingerEnd::Unsupported`]); half-close or read
/// failures stop lingering and close ([`LingerEnd::Failed`]).
///
/// The result is `Err` when scratch is empty, when the half-close has not finished
/// by the deadline (`TimedOut`: the peer stopped reading, so pending output cannot
/// flush) or when the final `poll_close` fails. The stream is then not closed: drop
/// it, which releases the handle. Dropping the future mid-way keeps in-flight
/// operations owned by the stream, so a new lingering close resumes them.
pub fn linger_close<'a, S: HalfClose>(
    stream: &'a mut S,
    scratch: &'a mut [u8],
    deadline: Option<Instant>,
) -> LingerClose<'a, S> {
    LingerClose {
        executor: stream.executor(),
        stream,
        scratch,
        deadline,
        sleep: None,
        phase: LingerPhase::Shutdown,
        lingered: Lingered {
            end: LingerEnd::Deadline,
            reads: 0,
            discarded: 0,
        },
    }
}
impl<S: HalfClose> LingerClose<'_, S> {
    /// Move the deadline, for example to honour an earlier server shutdown
    /// deadline. The timer is re-armed on the next poll only if it changed.
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }
    /// Discard progress so far; `end` is meaningful once the future completes.
    pub fn lingered(&self) -> Lingered {
        self.lingered
    }
    /// Whether the deadline has passed. Otherwise its timer is armed (re-armed
    /// only when the deadline moved) and registered with `cx`.
    fn poll_expired(&mut self, cx: &mut Context<'_>) -> Result<bool, io::ErrorKind> {
        let Some(deadline) = self.deadline else {
            self.sleep = None;
            return Ok(false);
        };
        if self.sleep.as_ref().is_none_or(|(at, _)| *at != deadline) {
            let Some(executor) = &self.executor else {
                return Ok(true);
            };
            if executor.now() >= deadline {
                self.sleep = None;
                return Ok(true);
            }
            // Replacing the timer drops (and closes) the previous one.
            self.sleep = Some((deadline, executor.sleep_until(deadline)));
        }
        let Some((_, sleep)) = self.sleep.as_mut() else {
            return Ok(false);
        };
        match Pin::new(sleep).poll(cx) {
            Poll::Ready(Ok(())) => Ok(true),
            Poll::Ready(Err(e)) => Err(error(e).kind()),
            Poll::Pending => Ok(false),
        }
    }
    fn poll_discard(&mut self, cx: &mut Context<'_>) -> Option<LingerEnd> {
        loop {
            match Pin::new(&mut *self.stream).poll_read(cx, self.scratch) {
                Poll::Ready(Ok(0)) => return Some(LingerEnd::Eof),
                Poll::Ready(Ok(n)) => {
                    self.lingered.reads += 1;
                    self.lingered.discarded += n as u64;
                }
                Poll::Ready(Err(e)) => return Some(LingerEnd::Failed(e.kind())),
                Poll::Pending => break,
            }
        }
        match self.poll_expired(cx) {
            Ok(false) => None,
            Ok(true) => Some(LingerEnd::Deadline),
            Err(kind) => Some(LingerEnd::Failed(kind)),
        }
    }
}
impl<S: HalfClose> Future for LingerClose<'_, S> {
    type Output = io::Result<Lingered>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            match this.phase {
                LingerPhase::Shutdown => {
                    if this.scratch.is_empty() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "lingering close needs non-empty scratch storage",
                        )));
                    }
                    match Pin::new(&mut *this.stream).poll_shutdown(cx) {
                        Poll::Pending => {
                            let expired = match this.poll_expired(cx) {
                                Ok(false) => return Poll::Pending,
                                Ok(true) => io::ErrorKind::TimedOut,
                                Err(kind) => kind,
                            };
                            this.lingered.end = if expired == io::ErrorKind::TimedOut {
                                LingerEnd::Deadline
                            } else {
                                LingerEnd::Failed(expired)
                            };
                            this.sleep = None;
                            this.phase = LingerPhase::Done;
                            return Poll::Ready(Err(io::Error::new(
                                expired,
                                "half-close did not complete before the linger deadline",
                            )));
                        }
                        Poll::Ready(Ok(())) => this.phase = LingerPhase::Discard,
                        Poll::Ready(Err(e)) => {
                            this.lingered.end = if e.kind() == io::ErrorKind::Unsupported {
                                LingerEnd::Unsupported
                            } else {
                                LingerEnd::Failed(e.kind())
                            };
                            this.phase = LingerPhase::Close;
                        }
                    }
                }
                LingerPhase::Discard => {
                    let Some(end) = this.poll_discard(cx) else {
                        return Poll::Pending;
                    };
                    this.lingered.end = end;
                    this.sleep = None;
                    this.phase = LingerPhase::Close;
                }
                LingerPhase::Close => {
                    let result = std::task::ready!(Pin::new(&mut *this.stream).poll_close(cx));
                    this.phase = LingerPhase::Done;
                    return Poll::Ready(result.map(|()| this.lingered));
                }
                LingerPhase::Done => return Poll::Ready(Ok(this.lingered)),
            }
        }
    }
}
/// One deadline shared across every phase of an exchange; uses the executor timer.
pub async fn deadline<B: Backend, F: Future<Output = io::Result<T>>, T>(
    executor: &ExecutorHandle<B>,
    at: Instant,
    future: F,
) -> io::Result<T> {
    executor.timeout_at(at, future).await.map_err(error)?
}
/// Resolve a service endpoint using the loop's DNS capability. Literal IPs need
/// no resolver and work on every raw-socket backend.
pub async fn resolve<B: Backend>(
    executor: &ExecutorHandle<B>,
    host: &str,
    port: u16,
    at: Instant,
) -> io::Result<SocketAddr> {
    if let Ok(ip) = host.parse() {
        return Ok(SocketAddr::new(ip, port));
    }
    deadline(executor, at, async {
        executor
            .resolve(turnloop::DnsRequest {
                host: host.to_owned(),
                port,
            })
            .await
            .map_err(error)?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "DNS returned no addresses"))
    })
    .await
}
/// Core-owned wire output. Acknowledged only after a successful complete flush.
pub trait Output {
    fn output(&self) -> &[u8];
    fn consume_output(&mut self, count: usize) -> io::Result<()>;
}
/// Drive one output phase without allocating another wire buffer.
pub async fn drain<S: Stream, C: Output>(stream: &mut S, core: &mut C) -> io::Result<()> {
    let n = core.output().len();
    write_all(stream, core.output()).await?;
    if n > 0 {
        core.consume_output(n)?;
    }
    Ok(())
}
/// Owned listener. Drop cancels accepts and closes the backend handle.
pub struct Listener<B: Backend> {
    executor: ExecutorHandle<B>,
    handle: turnloop::Handle,
}
impl<B: Backend> Listener<B> {
    pub fn bind(executor: &ExecutorHandle<B>, address: SocketAddr) -> io::Result<Self> {
        let handle = executor
            .driver()
            .tcp_listen(address, &Default::default())
            .map_err(error)?;
        Ok(Self {
            executor: executor.clone(),
            handle,
        })
    }
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.executor
            .driver()
            .local_addr(self.handle)
            .map_err(error)
    }
    pub async fn accept(&self) -> io::Result<AsyncIo<B>> {
        self.executor.accept(self.handle).await.map_err(error)
    }
}
impl<B: Backend> Drop for Listener<B> {
    fn drop(&mut self) {
        let _ = self
            .executor
            .driver()
            .close(self.handle, turnloop::Token(0));
    }
}

/// Close an owned stream if an exchange fails or its future is cancelled.
/// Commit only after all accepted output is flushed and parser progress is saved.
pub struct CloseOnDrop<'a, S> {
    slot: &'a mut Option<S>,
    committed: bool,
}
impl<'a, S> CloseOnDrop<'a, S> {
    pub fn new(slot: &'a mut Option<S>) -> Self {
        Self {
            slot,
            committed: false,
        }
    }
    pub fn stream(&mut self) -> io::Result<&mut S> {
        self.slot
            .as_mut()
            .ok_or_else(|| io::ErrorKind::NotConnected.into())
    }
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl<S> Drop for CloseOnDrop<'_, S> {
    fn drop(&mut self) {
        if !self.committed {
            self.slot.take();
        }
    }
}

/// Protocol-core contract used by database and mail adapters. Events can borrow
/// the core; the callback must finish consuming them before the next drive step.
pub trait SansIo: Output {
    type Event<'a>
    where
        Self: 'a;
    fn event(&mut self, receive: impl FnMut(Self::Event<'_>) -> io::Result<()>)
    -> io::Result<bool>;
    fn ingest(&mut self, bytes: &[u8], now: Instant) -> io::Result<usize>;
    fn disconnected(&mut self);
}
/// A shared sans-I/O driver. Protocol crates only implement SansIo; transport
/// flushing, partial input, cancellation and read scheduling live here once.
pub struct Driver<S, C: SansIo> {
    stream: Option<S>,
    core: Option<C>,
    input: Box<[u8]>,
    start: usize,
    end: usize,
}
impl<S: Stream, C: SansIo> Driver<S, C> {
    pub fn new(stream: S, core: C) -> Self {
        Self {
            stream: Some(stream),
            core: Some(core),
            input: vec![0; 16384].into_boxed_slice(),
            start: 0,
            end: 0,
        }
    }
    pub fn core(&self) -> &C {
        self.core.as_ref().expect("owned core")
    }
    pub fn core_mut(&mut self) -> &mut C {
        self.core.as_mut().expect("owned core")
    }
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
    pub fn stream(&self) -> Option<&S> {
        self.stream.as_ref()
    }
    /// Close immediately. Submitted transport storage remains owned by AsyncIo.
    pub fn abort(&mut self) {
        self.stream.take();
        self.core_mut().disconnected();
    }
    /// Install a new transport after the protocol has entered its reconnect
    /// state. Bytes from the failed transport are never fed to the new session.
    pub fn replace_stream(&mut self, stream: S) -> io::Result<()> {
        if self.stream.is_some() {
            return Err(io::Error::other("transport is still connected"));
        }
        self.start = 0;
        self.end = 0;
        self.stream = Some(stream);
        Ok(())
    }
    /// Guard a complete operation, including pauses between protocol events.
    pub fn exchange(&mut self) -> Exchange<'_, S, C> {
        Exchange {
            driver: self,
            committed: false,
        }
    }
    /// A TLS transition is legal only at a drained plaintext boundary.
    pub fn upgrade_stream(&mut self) -> io::Result<&mut S> {
        if self.start != self.end || !self.core().output().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "undrained TLS boundary",
            ));
        }
        self.stream
            .as_mut()
            .ok_or_else(|| io::ErrorKind::NotConnected.into())
    }
    /// Flush COPY or other output that has no corresponding input event.
    pub async fn flush(&mut self) -> io::Result<()> {
        let mut transport = CloseOnDrop::new(&mut self.stream);
        let result = drain(transport.stream()?, self.core.as_mut().expect("owned core")).await;
        if result.is_ok() {
            transport.commit();
        }
        result
    }
    /// Drive until one protocol event is delivered. Wrap this in `deadline` for
    /// the protocol's advertised timeout; cancellation closes and aborts the core.
    pub async fn next<B: Backend>(
        &mut self,
        executor: &ExecutorHandle<B>,
        mut receive: impl FnMut(C::Event<'_>) -> io::Result<()>,
    ) -> io::Result<()> {
        struct CoreGuard<'a, C: SansIo> {
            core: &'a mut C,
            done: bool,
        }
        impl<C: SansIo> Drop for CoreGuard<'_, C> {
            fn drop(&mut self) {
                if !self.done {
                    self.core.disconnected();
                }
            }
        }
        let mut core = CoreGuard {
            core: self.core.as_mut().expect("owned core"),
            done: false,
        };
        let mut transport = CloseOnDrop::new(&mut self.stream);
        loop {
            if core.core.event(&mut receive)? {
                core.done = true;
                transport.commit();
                return Ok(());
            }
            drain(transport.stream()?, core.core).await?;
            // A completed write can itself produce a terminal event.
            if core.core.event(&mut receive)? {
                core.done = true;
                transport.commit();
                return Ok(());
            }
            if self.start < self.end {
                let n = core
                    .core
                    .ingest(&self.input[self.start..self.end], executor.now())?;
                if n == 0 {
                    return Err(io::Error::other("core input stalled without an event"));
                }
                self.start += n;
                continue;
            }
            self.start = 0;
            self.end = read(transport.stream()?, &mut self.input).await?;
            if self.end == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
    }
    /// Transfer a validated TLS-upgrade boundary, including unread transport data.
    pub fn into_parts(mut self) -> io::Result<(S, C, Vec<u8>)> {
        let stream = self
            .stream
            .take()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))?;
        Ok((
            stream,
            self.core.take().expect("owned core"),
            self.input[self.start..self.end].to_vec(),
        ))
    }
}
/// Dropping an unfinished operation aborts both its protocol and transport.
pub struct Exchange<'a, S: Stream, C: SansIo> {
    driver: &'a mut Driver<S, C>,
    committed: bool,
}
impl<S: Stream, C: SansIo> Exchange<'_, S, C> {
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl<S: Stream, C: SansIo> std::ops::Deref for Exchange<'_, S, C> {
    type Target = Driver<S, C>;
    fn deref(&self) -> &Self::Target {
        self.driver
    }
}
impl<S: Stream, C: SansIo> std::ops::DerefMut for Exchange<'_, S, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.driver
    }
}
impl<S: Stream, C: SansIo> Drop for Exchange<'_, S, C> {
    fn drop(&mut self) {
        if !self.committed {
            self.driver.abort();
        }
    }
}
impl<S, C: SansIo> Drop for Driver<S, C> {
    fn drop(&mut self) {
        if let Some(core) = self.core.as_mut() {
            core.disconnected();
        }
    }
}
