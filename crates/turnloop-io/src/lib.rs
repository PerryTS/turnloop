//! Shared transport glue. See the crate README for the adapter ownership contract.
#![deny(unsafe_op_in_unsafe_fn)]
pub use futures_io::{AsyncRead, AsyncWrite};
use std::{
    future::{Future, poll_fn},
    io,
    net::SocketAddr,
    pin::Pin,
};
pub use turnloop::{self, AsyncIo, ExecutorHandle, Instant, backend::Backend};
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
pub async fn resolve<B: Backend>(executor: &ExecutorHandle<B>, host: &str, port: u16, at: Instant) -> io::Result<SocketAddr> {
    if let Ok(ip) = host.parse() { return Ok(SocketAddr::new(ip, port)); }
    deadline(executor, at, async {
        executor.resolve(turnloop::DnsRequest { host: host.to_owned(), port }).await.map_err(error)?
            .into_iter().next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "DNS returned no addresses"))
    }).await
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
    input: [u8; 16384],
    start: usize,
    end: usize,
}
impl<S: Stream, C: SansIo> Driver<S, C> {
    pub fn new(stream: S, core: C) -> Self {
        Self {
            stream: Some(stream),
            core: Some(core),
            input: [0; 16384],
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
    pub fn stream(&self) -> Option<&S> { self.stream.as_ref() }
    /// Close immediately. Submitted transport storage remains owned by AsyncIo.
    pub fn abort(&mut self) {
        self.stream.take();
        self.core_mut().disconnected();
    }
    /// Install a new transport after the protocol has entered its reconnect
    /// state. Bytes from the failed transport are never fed to the new session.
    pub fn replace_stream(&mut self, stream: S) -> io::Result<()> {
        if self.stream.is_some() { return Err(io::Error::other("transport is still connected")); }
        self.start = 0;
        self.end = 0;
        self.stream = Some(stream);
        Ok(())
    }
    /// Guard a complete operation, including pauses between protocol events.
    pub fn exchange(&mut self) -> Exchange<'_, S, C> {
        Exchange { driver: self, committed: false }
    }
    /// A TLS transition is legal only at a drained plaintext boundary.
    pub fn upgrade_stream(&mut self) -> io::Result<&mut S> {
        if self.start != self.end || !self.core().output().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "undrained TLS boundary"));
        }
        self.stream.as_mut().ok_or_else(|| io::ErrorKind::NotConnected.into())
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
    pub fn commit(&mut self) { self.committed = true; }
}
impl<S: Stream, C: SansIo> std::ops::Deref for Exchange<'_, S, C> {
    type Target = Driver<S, C>;
    fn deref(&self) -> &Self::Target { self.driver }
}
impl<S: Stream, C: SansIo> std::ops::DerefMut for Exchange<'_, S, C> {
    fn deref_mut(&mut self) -> &mut Self::Target { self.driver }
}
impl<S: Stream, C: SansIo> Drop for Exchange<'_, S, C> {
    fn drop(&mut self) { if !self.committed { self.driver.abort(); } }
}
impl<S, C: SansIo> Drop for Driver<S, C> {
    fn drop(&mut self) {
        if let Some(core) = self.core.as_mut() {
            core.disconnected();
        }
    }
}
