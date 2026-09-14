//! One authenticated MongoDB connection over the shared sans-I/O driver.
use super::AsyncConnection;
use crate::{connection::ConnectionEvent as Event, operation::Operation, uri::Options};
use bson::raw::RawDocument;
use std::{io, net::SocketAddr};
use turnloop_io::{AsyncIo, Backend, ExecutorHandle, Instant, deadline};
use turnloop_tls::asynchronous::{ClientTls, Transport};
pub(super) fn time(now: Instant) -> crate::Instant {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        crate::Instant::from_duration(now.as_duration())
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        now
    }
}
pub(super) fn instant(now: crate::Instant) -> Instant {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        Instant::from_duration(now.as_duration())
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        now
    }
}
pub struct Connection<B: Backend> {
    pub(super) driver: AsyncConnection<Transport<AsyncIo<B>>>,
    executor: ExecutorHandle<B>,
}
impl<B: Backend> Connection<B> {
    pub async fn connect(
        executor: &ExecutorHandle<B>,
        address: SocketAddr,
        options: Options,
        tls: Option<&ClientTls>,
        at: Instant,
    ) -> io::Result<Self> {
        let stream = deadline(executor, at, async {
            executor
                .connect(address, Default::default())
                .await
                .map_err(turnloop_io::error)
        })
        .await?;
        let mut entropy = [0; 24];
        turnloop_tls::rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut entropy)
            .map_err(|_| io::Error::other("secure entropy unavailable"))?;
        use base64::Engine;
        let nonce = base64::engine::general_purpose::STANDARD.encode(entropy);
        let mut core = crate::Connection::new(options);
        core.connected(time(executor.now()), &nonce)
            .map_err(io::Error::other)?;
        let mut driver = AsyncConnection::new(Transport::Plain(stream), core);
        deadline(executor, at, async {
            loop {
                let mut event = None;
                driver
                    .next(executor, |e| {
                        event = Some(e);
                        Ok(())
                    })
                    .await?;
                match event {
                    Some(Event::Ready) => return Ok(()),
                    Some(Event::UpgradeTls) => {
                        let tls = tls.ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "TLS configuration required",
                            )
                        })?;
                        driver.upgrade_stream()?.upgrade(tls, executor, at).await?;
                        driver
                            .core_mut()
                            .tls_established()
                            .map_err(io::Error::other)?;
                    }
                    Some(Event::Failed { error, .. }) => return Err(io::Error::other(error)),
                    Some(Event::Closed) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    _ => {}
                }
            }
        })
        .await?;
        Ok(Self {
            driver,
            executor: executor.clone(),
        })
    }
    pub fn is_reusable(&self) -> bool {
        self.driver.is_connected() && self.driver.core().is_ready()
    }
    pub fn hello(&self) -> Option<&bson::Document> {
        self.driver.core().hello.as_ref()
    }
    pub fn close(&mut self) {
        self.driver.abort();
    }
    pub async fn command(
        &mut self,
        body: &RawDocument,
        sequences: &[(&str, &[&RawDocument])],
        at: Instant,
        mut receive: impl FnMut(&RawDocument) -> io::Result<()>,
    ) -> io::Result<()> {
        self.driver
            .core_mut()
            .command(1, body, sequences, time(self.executor.now()))
            .map_err(io::Error::other)?;
        let mut exchange = self.driver.exchange();
        deadline(&self.executor, at, async {
            loop {
                let mut event = None;
                exchange
                    .next(&self.executor, |e| {
                        event = Some(e);
                        Ok(())
                    })
                    .await?;
                match event {
                    Some(Event::Reply { .. }) => {
                        let reply = exchange.core().reply().map_err(io::Error::other)?;
                        let result = crate::Error::from_response(reply)
                            .map_err(io::Error::other)
                            .and_then(|_| receive(reply));
                        exchange
                            .core_mut()
                            .release_reply()
                            .map_err(io::Error::other)?;
                        return result;
                    }
                    Some(Event::Unacknowledged { .. }) => return Ok(()),
                    Some(Event::Failed { error, .. }) => return Err(io::Error::other(error)),
                    Some(Event::Closed) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    _ => {}
                }
            }
        })
        .await?;
        exchange.commit();
        Ok(())
    }
    pub(super) async fn operation(
        &mut self,
        operation: &mut Operation,
        at: Instant,
        mut receive: impl FnMut(&RawDocument) -> io::Result<()>,
    ) -> io::Result<bool> {
        let mut exchange = self.driver.exchange();
        operation
            .send(exchange.core_mut(), time(self.executor.now()))
            .map_err(io::Error::other)?;
        let result = deadline(&self.executor, at, async {
            loop {
                let mut event = None;
                exchange
                    .next(&self.executor, |e| {
                        event = Some(e);
                        Ok(())
                    })
                    .await?;
                match event {
                    Some(Event::Reply { .. }) => {
                        let reply = exchange.core().reply().map_err(io::Error::other)?;
                        let complete = operation.response(reply).map_err(io::Error::other)?;
                        if complete {
                            receive(reply)?;
                        }
                        exchange
                            .core_mut()
                            .release_reply()
                            .map_err(io::Error::other)?;
                        return Ok(complete);
                    }
                    Some(Event::Failed { error, .. }) => {
                        operation.failed(error);
                        return Ok(false);
                    }
                    Some(Event::Unacknowledged { .. }) => return Ok(true),
                    Some(Event::Closed) => return Err(io::ErrorKind::UnexpectedEof.into()),
                    _ => {}
                }
            }
        })
        .await;
        match result {
            Ok(done) => {
                if exchange.core().is_ready() {
                    exchange.commit();
                }
                Ok(done)
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::TimedOut {
                    return Err(e);
                }
                operation.failed(crate::Error::new(crate::ErrorKind::Network, e.to_string()));
                Ok(false)
            }
        }
    }
}
impl<B: Backend + 'static> turnloop_io::pool::Connection for Connection<B> {
    fn reusable(&self) -> bool {
        self.is_reusable()
    }
    fn handle(&self) -> Option<turnloop_io::turnloop::Handle> {
        self.driver.stream()?.get_ref().map(AsyncIo::handle)
    }
}
