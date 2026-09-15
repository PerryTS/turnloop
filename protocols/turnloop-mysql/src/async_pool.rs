//! Executor ownership for the existing sans-I/O pool policy.
use super::{ConnectOptions, Connection};
use crate::pool::{ConnectionId, Event, Lease as CoreLease, Pool as Core};
use std::{io, time::Duration};
use turnloop_io::{
    Backend, ExecutorHandle, Instant,
    pool::{self as shared, Action, Policy},
};
impl Policy for Core {
    type Id = ConnectionId;
    type Lease = CoreLease;
    fn id(lease: CoreLease) -> ConnectionId {
        lease.connection
    }
    fn checkout(&mut self, token: u64, now: Instant, at: Instant) -> io::Result<()> {
        self.checkout(
            token,
            super::client::time(now),
            Some(super::client::time(at)),
        )
        .map_err(io::Error::other)
    }
    fn cancel(&mut self, token: u64, now: Instant) {
        self.cancel_checkout(token, super::client::time(now));
    }
    fn connected(&mut self, id: ConnectionId, now: Instant) -> io::Result<()> {
        self.connected(id, super::client::time(now))
            .map_err(io::Error::other)
    }
    fn connect_failed(&mut self, id: ConnectionId, now: Instant) -> io::Result<()> {
        self.connect_failed(id, super::client::time(now))
            .map_err(io::Error::other)
    }
    fn checkin(&mut self, lease: CoreLease, now: Instant, destroy: bool) -> io::Result<()> {
        self.checkin(lease, super::client::time(now), destroy)
            .map_err(io::Error::other)
    }
    fn closed(&mut self, id: ConnectionId, now: Instant) -> io::Result<()> {
        self.closed(id, super::client::time(now))
            .map_err(io::Error::other)
    }
    fn event(&mut self) -> Option<Action<ConnectionId, CoreLease>> {
        loop {
            match self.next_event()? {
                Event::Connect(id) => return Some(Action::Connect(id)),
                Event::Close(id) => return Some(Action::Close(id)),
                Event::Removed(id) => return Some(Action::Remove(id)),
                Event::Acquired(lease) => {
                    return Some(Action::Acquired {
                        token: lease.token,
                        lease,
                    });
                }
                Event::CheckoutFailed { token, error } => {
                    return Some(Action::Failed {
                        token,
                        error: io::Error::other(error),
                    });
                }
                Event::Ended => return Some(Action::Ended),
                _ => {}
            }
        }
    }
    fn next_deadline(&self) -> Option<Instant> {
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            self.next_timeout()
                .map(|t| Instant::from_duration(t.as_duration()))
        }
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            self.next_timeout()
        }
    }
    fn expire(&mut self, now: Instant) {
        self.handle_timeout(super::client::time(now));
    }
    fn end(&mut self) -> io::Result<()> {
        self.end().map_err(io::Error::other)
    }
}
struct Connector(ConnectOptions);
impl<B: Backend + 'static> shared::Connector<B> for Connector {
    type Connection = Connection<B>;
    fn connect(
        &self,
        executor: &ExecutorHandle<B>,
        at: Instant,
    ) -> impl std::future::Future<Output = io::Result<Connection<B>>> + 'static {
        let executor = executor.clone();
        let options = self.0.clone();
        async move { Connection::connect(&executor, &options, at).await }
    }
}
pub type PooledConnection<B> = shared::Lease<B, Core, Connection<B>>;
pub struct Pool<B: Backend + 'static> {
    inner: shared::Pool<B, Core, Connection<B>>,
}
impl<B: Backend + 'static> Clone for Pool<B> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
impl<B: Backend + 'static> Pool<B> {
    pub fn new(
        executor: &ExecutorHandle<B>,
        options: ConnectOptions,
        config: crate::pool::Config,
        connect_timeout: Duration,
    ) -> io::Result<Self> {
        let capacity = config.max;
        Ok(Self {
            inner: shared::Pool::new(
                executor,
                Core::new(config).map_err(io::Error::other)?,
                Connector(options),
                connect_timeout,
                capacity,
            )?,
        })
    }
    pub async fn acquire(&self, at: Instant) -> io::Result<PooledConnection<B>> {
        self.inner.acquire(at).await
    }
    pub fn total(&self) -> usize {
        self.inner.total()
    }
    pub async fn end(&self) -> io::Result<()> {
        self.inner.end().await
    }
}
