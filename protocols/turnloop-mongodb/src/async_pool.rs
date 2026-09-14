use super::connection::{instant, time};
use super::{ConnectOptions, Connection};
use crate::pool::{Lease, Pool as Core, PoolEvent};
use std::{io, time::Duration};
use turnloop_io::{
    Backend, ExecutorHandle, Instant,
    pool::{self as shared, Action, Policy},
};
impl Policy for Core {
    type Id = Lease;
    type Lease = Lease;
    fn id(lease: Lease) -> Lease {
        lease
    }
    fn checkout(&mut self, token: u64, now: Instant, _at: Instant) -> io::Result<()> {
        self.checkout(token, time(now)).map_err(io::Error::other)
    }
    fn cancel(&mut self, token: u64, _now: Instant) {
        self.cancel_checkout(token);
    }
    fn connected(&mut self, id: Lease, now: Instant) -> io::Result<()> {
        self.connected(id, time(now)).map_err(io::Error::other)
    }
    fn connect_failed(&mut self, id: Lease, now: Instant) -> io::Result<()> {
        self.connect_failed(
            id,
            crate::Error::new(crate::ErrorKind::Network, "connection failed"),
            time(now),
        )
        .map_err(io::Error::other)
    }
    fn checkin(&mut self, lease: Lease, now: Instant, destroy: bool) -> io::Result<()> {
        if destroy {
            self.clear();
            self.ready(time(now));
        }
        self.checkin(lease, time(now)).map_err(io::Error::other)
    }
    fn closed(&mut self, _id: Lease, _now: Instant) -> io::Result<()> {
        Ok(())
    }
    fn event(&mut self) -> Option<Action<Lease, Lease>> {
        loop {
            match self.poll_event()? {
                PoolEvent::Connect(id) => return Some(Action::Connect(id)),
                PoolEvent::Close(id) => return Some(Action::Close(id)),
                PoolEvent::CheckedOut { token, connection } => {
                    return Some(Action::Acquired {
                        token,
                        lease: connection,
                    });
                }
                PoolEvent::CheckoutFailed { token, error } => {
                    return Some(Action::Failed {
                        token,
                        error: io::Error::other(error),
                    });
                }
                PoolEvent::Closed => return Some(Action::Ended),
                _ => {}
            }
        }
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.next_timeout().map(instant)
    }
    fn expire(&mut self, now: Instant) {
        self.handle_timeout(time(now));
    }
    fn end(&mut self) -> io::Result<()> {
        self.close();
        Ok(())
    }
}
struct Connector {
    address: crate::uri::Address,
    options: ConnectOptions,
}
impl<B: Backend + 'static> shared::Connector<B> for Connector {
    type Connection = Connection<B>;
    fn connect(
        &self,
        executor: &ExecutorHandle<B>,
        at: Instant,
    ) -> impl std::future::Future<Output = io::Result<Connection<B>>> + 'static {
        let executor = executor.clone();
        let address = self.address.clone();
        let options = self.options.clone();
        async move {
            let socket = turnloop_io::resolve(&executor, &address.host, address.port, at).await?;
            Connection::connect(
                &executor,
                socket,
                options.protocol,
                options.tls.as_ref(),
                at,
            )
            .await
        }
    }
}
pub type PooledConnection<B> = shared::Lease<B, Core, Connection<B>>;
pub type Pool<B> = shared::Pool<B, Core, Connection<B>>;
pub(super) fn create<B: Backend + 'static>(
    executor: &ExecutorHandle<B>,
    address: crate::uri::Address,
    options: &ConnectOptions,
) -> io::Result<Pool<B>> {
    let config = crate::pool::PoolOptions {
        min_size: options.protocol.min_pool_size,
        max_size: options.protocol.max_pool_size,
        max_connecting: options.protocol.max_connecting,
        wait_timeout: options.protocol.wait_queue_timeout,
        max_idle: options.protocol.max_idle_time,
    };
    let mut core = Core::new(config).map_err(io::Error::other)?;
    core.ready(time(executor.now()));
    let timeout = if options.protocol.connect_timeout.is_zero() {
        Duration::from_secs(30)
    } else {
        options.protocol.connect_timeout
    };
    shared::Pool::new(
        executor,
        core,
        Connector {
            address,
            options: options.clone(),
        },
        timeout,
        options.protocol.max_pool_size.max(1),
    )
}
