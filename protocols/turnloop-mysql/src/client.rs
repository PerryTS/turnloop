//! Sequential mysql2-shaped sessions over the shared stream driver.
use crate::asynchronous::AsyncConnection;
use crate::{Config, Event, Outcome, Statement, Value};
use std::{io, net::SocketAddr};
use turnloop_io::{AsyncIo, Backend, ExecutorHandle, Instant, Stream, deadline};
use turnloop_tls::asynchronous::{ClientTls, Transport};

#[derive(Clone)]
pub struct ConnectOptions {
    pub address: SocketAddr,
    pub protocol: Config,
    pub tls: Option<ClientTls>,
}
pub struct Connection<B: Backend, S: Stream = AsyncIo<B>> {
    executor: ExecutorHandle<B>,
    driver: AsyncConnection<Transport<S>>,
    pub connection_id: u32,
}
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
impl<B: Backend> Connection<B> {
    pub async fn connect(
        executor: &ExecutorHandle<B>,
        options: &ConnectOptions,
        at: Instant,
    ) -> io::Result<Self> {
        let stream = deadline(executor, at, async {
            executor
                .connect(options.address, Default::default())
                .await
                .map_err(turnloop_io::error)
        })
        .await?;
        Self::from_stream(
            executor,
            stream,
            options.protocol.clone(),
            options.tls.as_ref(),
            at,
        )
        .await
    }
}
impl<B: Backend, S: Stream> Connection<B, S> {
    pub async fn from_stream(
        executor: &ExecutorHandle<B>,
        stream: S,
        config: Config,
        tls: Option<&ClientTls>,
        at: Instant,
    ) -> io::Result<Self> {
        let mut driver = AsyncConnection::new(
            Transport::Plain(stream),
            crate::Connection::new(config).map_err(io::Error::other)?,
        );
        let connection_id = deadline(executor, at, async {
            loop {
                let mut upgrade = false;
                let mut seed = false;
                let mut ready = None;
                driver
                    .next(executor, |event| {
                        match event {
                            Event::UpgradeTls => upgrade = true,
                            Event::RsaSeedNeeded => seed = true,
                            Event::Connected { connection_id } => ready = Some(connection_id),
                            Event::Error { error, .. } => {
                                return Err(io::Error::other(format!("{error:?}")));
                            }
                            Event::Closed { reason } => return Err(io::Error::other(reason)),
                            _ => {}
                        }
                        Ok(())
                    })
                    .await?;
                if upgrade {
                    let tls = tls.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "TLS configuration required")
                    })?;
                    driver.flush().await?;
                    driver.upgrade_stream()?.upgrade(tls, executor, at).await?;
                    driver
                        .core_mut()
                        .tls_established()
                        .map_err(io::Error::other)?;
                }
                if seed {
                    let mut entropy = [0; 20];
                    turnloop_tls::rustls::crypto::ring::default_provider()
                        .secure_random
                        .fill(&mut entropy)
                        .map_err(|_| io::Error::other("secure entropy unavailable"))?;
                    driver
                        .core_mut()
                        .rsa_seed(entropy)
                        .map_err(io::Error::other)?;
                }
                if let Some(id) = ready {
                    return Ok(id);
                }
            }
        })
        .await?;
        Ok(Self {
            executor: executor.clone(),
            driver,
            connection_id,
        })
    }
    pub fn is_reusable(&self) -> bool {
        self.driver.is_connected()
            && self.driver.core().is_ready()
            && !self
                .driver
                .core()
                .status()
                .contains(crate::StatusFlags::SERVER_STATUS_IN_TRANS)
    }
    pub fn close(&mut self) {
        self.driver.abort();
    }
    async fn complete(
        &mut self,
        at: Instant,
        mut receive: impl FnMut(Event<'_>) -> io::Result<()>,
    ) -> io::Result<Outcome> {
        let mut exchange = self.driver.exchange();
        let outcome = deadline(&self.executor, at, async {
            loop {
                let mut done = None;
                exchange
                    .next(&self.executor, |event| {
                        match &event {
                            Event::Completed { outcome, .. } => done = Some(*outcome),
                            Event::Closed { reason } => return Err(io::Error::other(*reason)),
                            Event::LocalInfile { .. } => {
                                return Err(io::Error::new(
                                    io::ErrorKind::Unsupported,
                                    "LOCAL INFILE needs an explicit source",
                                ));
                            }
                            _ => {}
                        }
                        receive(event)
                    })
                    .await?;
                if let Some(outcome) = done {
                    return Ok(outcome);
                }
            }
        })
        .await?;
        exchange.commit();
        Ok(outcome)
    }
    /// Streams every text row, OK packet and field, including all multi-results.
    pub async fn query(
        &mut self,
        sql: &str,
        at: Instant,
        receive: impl FnMut(Event<'_>) -> io::Result<()>,
    ) -> io::Result<Outcome> {
        self.driver
            .core_mut()
            .query(1, sql, Some(time(at)))
            .map_err(io::Error::other)?;
        self.complete(at, receive).await
    }
    pub async fn prepare(&mut self, sql: &str, at: Instant) -> io::Result<Statement> {
        self.driver
            .core_mut()
            .prepare(1, sql, Some(time(at)))
            .map_err(io::Error::other)?;
        let mut prepared = None;
        let outcome = self
            .complete(at, |event| {
                if let Event::Prepared { statement, .. } = event {
                    prepared = Some(statement);
                }
                Ok(())
            })
            .await?;
        if outcome != Outcome::Success {
            return Err(io::Error::other("prepare failed"));
        }
        prepared.ok_or_else(|| io::Error::other("missing prepared statement"))
    }
    pub async fn execute(
        &mut self,
        statement: Statement,
        params: &[Value],
        at: Instant,
        receive: impl FnMut(Event<'_>) -> io::Result<()>,
    ) -> io::Result<Outcome> {
        self.driver
            .core_mut()
            .execute(1, statement.id, params, Some(time(at)))
            .map_err(io::Error::other)?;
        self.complete(at, receive).await
    }
    pub async fn close_statement(
        &mut self,
        statement: Statement,
        at: Instant,
    ) -> io::Result<Outcome> {
        self.driver
            .core_mut()
            .close_statement(1, statement.id)
            .map_err(io::Error::other)?;
        self.complete(at, |_| Ok(())).await
    }
    pub async fn ping(&mut self, at: Instant) -> io::Result<Outcome> {
        self.driver.core_mut().ping(1).map_err(io::Error::other)?;
        self.complete(at, |_| Ok(())).await
    }
    pub async fn begin(&mut self, at: Instant) -> io::Result<Transaction<'_, B, S>> {
        if self.query("START TRANSACTION", at, |_| Ok(())).await? != Outcome::Success {
            return Err(io::Error::other("START TRANSACTION failed"));
        }
        Ok(Transaction {
            connection: self,
            finished: false,
        })
    }
}
/// An unfinished transaction is closed on drop, letting MySQL roll it back.
pub struct Transaction<'a, B: Backend, S: Stream> {
    connection: &'a mut Connection<B, S>,
    finished: bool,
}
impl<B: Backend, S: Stream> std::ops::Deref for Transaction<'_, B, S> {
    type Target = Connection<B, S>;
    fn deref(&self) -> &Self::Target {
        self.connection
    }
}
impl<B: Backend, S: Stream> std::ops::DerefMut for Transaction<'_, B, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
    }
}
impl<B: Backend, S: Stream> Transaction<'_, B, S> {
    pub async fn commit(mut self, at: Instant) -> io::Result<Outcome> {
        let result = self.connection.query("COMMIT", at, |_| Ok(())).await?;
        self.finished = result == Outcome::Success;
        Ok(result)
    }
    pub async fn rollback(mut self, at: Instant) -> io::Result<Outcome> {
        let result = self.connection.query("ROLLBACK", at, |_| Ok(())).await?;
        self.finished = result == Outcome::Success;
        Ok(result)
    }
}
impl<B: Backend, S: Stream> Drop for Transaction<'_, B, S> {
    fn drop(&mut self) {
        if !self.finished {
            self.connection.close();
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
