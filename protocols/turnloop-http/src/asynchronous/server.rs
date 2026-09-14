//! Owned accept loop and explicit graceful shutdown. Services own each stream.
use std::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    io,
    pin::pin,
    rc::Rc,
    task::{Poll, Waker},
};
use turnloop_io::{AsyncIo, Backend, ExecutorHandle, Listener, turnloop::executor::JoinHandle};
/// Cloneable shutdown signal. Waiting futures register their own waker.
#[derive(Clone, Default)]
pub struct Shutdown {
    inner: Rc<State>,
}
#[derive(Default)]
struct State {
    stopped: Cell<bool>,
    waiters: RefCell<Vec<Option<Waker>>>,
}
impl Shutdown {
    pub fn stop(&self) {
        self.inner.stopped.set(true);
        let mut waiters = self.inner.waiters.borrow_mut();
        for w in waiters.iter_mut().filter_map(Option::take) {
            w.wake();
        }
    }
    pub fn is_stopped(&self) -> bool {
        self.inner.stopped.get()
    }
    /// Race one operation against shutdown. Cancellation drops the operation.
    pub async fn until<F: Future>(&self, future: F) -> Option<F::Output> {
        let index = {
            let mut waiters = self.inner.waiters.borrow_mut();
            if let Some(i) = waiters.iter().position(Option::is_none) {
                i
            } else {
                waiters.push(None);
                waiters.len() - 1
            }
        };
        struct Registration<'a> {
            signal: &'a Shutdown,
            index: usize,
        }
        impl Drop for Registration<'_> {
            fn drop(&mut self) {
                self.signal.inner.waiters.borrow_mut()[self.index] = None;
            }
        }
        let _registration = Registration {
            signal: self,
            index,
        };
        let mut future = pin!(future);
        poll_fn(|cx| {
            if self.is_stopped() {
                return Poll::Ready(None);
            }
            self.inner.waiters.borrow_mut()[index] = Some(cx.waker().clone());
            future.as_mut().poll(cx).map(Some)
        })
        .await
    }
}
/// TCP accept loop. Dropping the server cancels all in-flight services. Calling
/// `shutdown().stop()` stops accepts and asks services to drain; `run` joins them.
pub struct Server<B: Backend> {
    executor: ExecutorHandle<B>,
    listener: Option<Listener<B>>,
    shutdown: Shutdown,
    tasks: Vec<JoinHandle<io::Result<()>>>,
}
impl<B: Backend + 'static> Server<B> {
    pub fn bind(executor: ExecutorHandle<B>, address: std::net::SocketAddr) -> io::Result<Self> {
        Ok(Self {
            listener: Some(Listener::bind(&executor, address)?),
            executor,
            shutdown: Shutdown::default(),
            tasks: Vec::new(),
        })
    }
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.listener
            .as_ref()
            .ok_or_else(|| io::Error::other("server stopped"))?
            .local_addr()
    }
    pub fn shutdown(&self) -> Shutdown {
        self.shutdown.clone()
    }
    pub async fn run<F, Fut>(&mut self, service: F) -> io::Result<()>
    where
        F: Fn(AsyncIo<B>, Shutdown) -> Fut,
        Fut: Future<Output = io::Result<()>> + 'static,
    {
        while let Some(listener) = self.listener.as_ref() {
            // Reap successful and failed services; connection errors do not kill
            // the listener, while their JoinHandles still observe task completion.
            let mut i = 0;
            while i < self.tasks.len() {
                if self.tasks[i].is_finished() {
                    let task = self.tasks.swap_remove(i);
                    let _ = task.await;
                } else {
                    i += 1;
                }
            }
            let Some(stream) = self.shutdown.until(listener.accept()).await else {
                break;
            };
            self.tasks.push(
                self.executor
                    .spawn_local(service(stream?, self.shutdown.clone()))
                    .map_err(turnloop_io::error)?,
            );
        }
        self.listener.take();
        for task in self.tasks.drain(..) {
            task.await
                .map_err(|_| io::Error::other("service cancelled"))??;
        }
        Ok(())
    }
}
impl<B: Backend> Drop for Server<B> {
    fn drop(&mut self) {
        self.shutdown.stop();
        self.tasks.clear();
        self.listener.take();
    }
}

/// Streaming HTTP/1 response encoder passed to a service callback. Its retained
/// output is flushed after every event, bounding echo-server body storage.
pub struct Response {
    encoder: Option<crate::http1::Encoder>,
    output: Vec<u8>,
    finished: bool,
    keep_alive: bool,
}
impl Default for Response {
    fn default() -> Self {
        Self {
            encoder: None,
            output: Vec::with_capacity(65536),
            finished: false,
            keep_alive: true,
        }
    }
}
impl Response {
    pub fn start(
        &mut self,
        head: &crate::http1::Head,
        length: crate::http1::BodyLength,
    ) -> io::Result<()> {
        if self.encoder.is_some() {
            return Err(io::Error::other("response already started"));
        }
        self.encoder = Some(super::encode_head(head, length, &mut self.output)?);
        self.keep_alive = head.keep_alive && !head.token("connection", "close");
        Ok(())
    }
    pub fn body(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.encoder
            .as_mut()
            .ok_or_else(|| io::Error::other("response not started"))?
            .body(bytes, &mut self.output)
            .map_err(io::Error::other)
    }
    pub fn finish(&mut self, trailers: &[crate::http1::Header]) -> io::Result<()> {
        self.encoder
            .as_mut()
            .ok_or_else(|| io::Error::other("response not started"))?
            .finish(trailers, &mut self.output)
            .map_err(io::Error::other)?;
        self.finished = true;
        Ok(())
    }
}
/// Serve HTTP/1 keep-alive with streaming request/response callbacks. Idle reads
/// are cancelled on shutdown; an active request finishes before the connection closes.
pub async fn http1<S: turnloop_io::Stream>(
    stream: S,
    shutdown: Shutdown,
    mut service: impl FnMut(crate::http1::Event<'_>, &mut Response) -> io::Result<()>,
) -> io::Result<()> {
    let mut conn = super::Http1::new(stream, crate::http1::Mode::Request);
    let mut response = Response::default();
    loop {
        let Some(head) = shutdown.until(conn.head()).await else {
            return Ok(());
        };
        let head = head?;
        service(crate::http1::Event::Head(head), &mut response)?;
        turnloop_io::write_all(
            conn.stream.as_mut().ok_or_else(super::closed)?,
            &response.output,
        )
        .await?;
        response.output.clear();
        loop {
            let mut ended = false;
            let received = conn
                .event(|event| {
                    ended = matches!(event, crate::http1::Event::End);
                    service(event, &mut response)
                })
                .await?;
            turnloop_io::write_all(
                conn.stream.as_mut().ok_or_else(super::closed)?,
                &response.output,
            )
            .await?;
            response.output.clear();
            if !received {
                return Err(io::Error::other("service must handle upgrades explicitly"));
            }
            if ended {
                break;
            }
        }
        if !response.finished {
            return Err(io::Error::other("service did not finish response"));
        }
        if shutdown.is_stopped() || !conn.reusable() || !response.keep_alive {
            if let Some(stream) = conn.stream.as_mut() {
                turnloop_io::close(stream).await?;
            }
            return Ok(());
        }
        conn.reset()?;
        response.encoder = None;
        response.finished = false;
    }
}
/// Serve HTTP/2 and flush control frames even on protocol failure. Services own
/// stream-level application state and release capacity after consuming DATA.
/// Shutdown sends GOAWAY and continues processing existing streams until drained.
pub async fn http2<S: turnloop_io::Stream>(
    stream: S,
    shutdown: Shutdown,
    mut service: impl FnMut(&mut crate::http2::Connection, crate::http2::Event<'_>) -> io::Result<()>,
) -> io::Result<()> {
    let mut conn = super::Http2::new(stream, crate::http2::Role::Server)?;
    conn.shutdown = Some(shutdown);
    loop {
        if conn.core.is_drained() {
            if let Some(stream) = conn.stream.as_mut() {
                turnloop_io::close(stream).await?;
            }
            return Ok(());
        }
        if !conn.event(&mut service).await? {
            return Ok(());
        }
    }
}
