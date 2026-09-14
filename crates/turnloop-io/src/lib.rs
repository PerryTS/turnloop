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
/// Core-owned wire output. Acknowledged only after a successful complete flush.
pub trait Output {
    fn output(&self) -> &[u8];
    fn consume_output(&mut self, count: usize) -> io::Result<()>;
}
/// Drive one output phase without allocating another wire buffer.
pub async fn drain<S: Stream, C: Output>(stream: &mut S, core: &mut C) -> io::Result<()> {
    let n = core.output().len();
    write_all(stream, core.output()).await?;
    core.consume_output(n)
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
