//! Async PostgreSQL sessions. Borrowed row callbacks keep the steady path allocation-free.
use crate::asynchronous::AsyncConnection;
use crate::{ChannelBinding, Config, Event, ExtendedQuery, Outcome, ScramSha256};
use std::{io, net::SocketAddr};
use turnloop_io::{AsyncIo, Backend, ExecutorHandle, Instant, Stream, deadline};
use turnloop_tls::asynchronous::{ClientTls, Transport};

#[derive(Clone)]
pub struct ConnectOptions {
    pub address: SocketAddr,
    pub protocol: Config,
    pub tls: Option<ClientTls>,
    /// tls-server-end-point digest for SCRAM-PLUS, computed from the verified leaf.
    pub channel_binding: Option<Vec<u8>>,
}

pub struct Client<B: Backend, S: Stream = AsyncIo<B>> {
    executor: ExecutorHandle<B>,
    driver: AsyncConnection<Transport<S>>,
    cancel: Option<CancelToken>,
}
#[derive(Clone, Copy)]
pub struct CancelToken {
    address: SocketAddr,
    packet: [u8; 16],
}
impl CancelToken {
    /// PostgreSQL cancellation uses a fresh connection, never the query stream.
    pub async fn cancel<B: Backend>(
        &self,
        executor: &ExecutorHandle<B>,
        at: Instant,
    ) -> io::Result<()> {
        deadline(executor, at, async {
            let mut stream = executor
                .connect(self.address, Default::default())
                .await
                .map_err(turnloop_io::error)?;
            turnloop_io::write_all(&mut stream, &self.packet).await?;
            // The backend closes the cancellation connection after processing it.
            let mut byte = [0];
            if turnloop_io::read(&mut stream, &mut byte).await? != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected CancelRequest response",
                ));
            }
            Ok(())
        })
        .await
    }
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
impl<B: Backend> Client<B> {
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
        let mut client = Self::from_stream(
            executor,
            stream,
            options.protocol.clone(),
            options.tls.as_ref(),
            options.channel_binding.as_deref(),
            at,
        )
        .await?;
        client.cancel = client
            .driver
            .core()
            .cancel_request()
            .map(|packet| CancelToken {
                address: options.address,
                packet,
            });
        Ok(client)
    }
}
impl<B: Backend, S: Stream> Client<B, S> {
    pub async fn from_stream(
        executor: &ExecutorHandle<B>,
        stream: S,
        config: Config,
        tls: Option<&ClientTls>,
        binding: Option<&[u8]>,
        at: Instant,
    ) -> io::Result<Self> {
        let password = config.password.clone();
        let mut driver = AsyncConnection::new(
            Transport::Plain(stream),
            crate::Connection::new(config).map_err(io::Error::other)?,
        );
        deadline(executor, at, async {
            loop {
                let mut upgrade = false;
                let mut scram = None;
                let mut ready = false;
                driver
                    .next(executor, |event| {
                        match event {
                            Event::UpgradeTls => upgrade = true,
                            Event::ScramNeeded { plus } => scram = Some(plus),
                            Event::Connected => ready = true,
                            Event::Error { error, .. } => {
                                return Err(io::Error::other(error.message().to_owned()));
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
                    driver.upgrade_stream()?.upgrade(tls, executor, at).await?;
                    driver
                        .core_mut()
                        .tls_established()
                        .map_err(io::Error::other)?;
                }
                if let Some(plus) = scram {
                    let channel = if plus {
                        ChannelBinding::tls_server_end_point(
                            binding
                                .ok_or_else(|| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidInput,
                                        "SCRAM-PLUS requires certificate binding",
                                    )
                                })?
                                .to_vec(),
                        )
                    } else {
                        ChannelBinding::unsupported()
                    };
                    driver
                        .core_mut()
                        .start_scram(ScramSha256::new(&password, channel))
                        .map_err(io::Error::other)?;
                }
                if ready {
                    return Ok(());
                }
            }
        })
        .await?;
        Ok(Self {
            executor: executor.clone(),
            driver,
            cancel: None,
        })
    }
    pub fn is_reusable(&self) -> bool {
        self.driver.is_connected()
            && self.driver.core().is_ready()
            && self.driver.core().pending_count() == 0
            && self.driver.core().transaction_status() == crate::TransactionStatus::Idle
    }
    pub fn cancel_token(&self) -> Option<CancelToken> {
        self.cancel
    }
    pub fn close(&mut self) {
        self.driver.abort();
    }
    /// Each callback consumes a borrowed event before another read can occur.
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
    /// Named queries use the core's prepared statement cache, including SQL/OID validation.
    pub async fn execute(
        &mut self,
        query: ExtendedQuery<'_>,
        at: Instant,
        receive: impl FnMut(Event<'_>) -> io::Result<()>,
    ) -> io::Result<Outcome> {
        self.driver
            .core_mut()
            .execute(1, query, Some(time(at)))
            .map_err(io::Error::other)?;
        self.complete(at, receive).await
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
                            Event::CopyIn { .. } => {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "use copy_in for COPY FROM STDIN",
                                ));
                            }
                            Event::Closed { reason } => return Err(io::Error::other(*reason)),
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
    /// COPY OUT chunks borrow the receive buffer and provide natural backpressure.
    pub async fn copy_out(
        &mut self,
        sql: &str,
        at: Instant,
        mut chunk: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<Outcome> {
        self.query(sql, at, |event| {
            if let Event::CopyData { data, .. } = event {
                chunk(data)?;
            }
            Ok(())
        })
        .await
    }
    pub async fn copy_in(&mut self, sql: &str, at: Instant) -> io::Result<CopyIn<'_, B, S>> {
        self.driver
            .core_mut()
            .query(1, sql, Some(time(at)))
            .map_err(io::Error::other)?;
        let mut exchange = self.driver.exchange();
        deadline(&self.executor, at, async {
            loop {
                let mut ready = false;
                exchange
                    .next(&self.executor, |event| {
                        match event {
                            Event::CopyIn { .. } => ready = true,
                            Event::Error { error, .. } => {
                                return Err(io::Error::other(error.message().to_owned()));
                            }
                            Event::Completed { .. } | Event::Closed { .. } => {
                                return Err(io::Error::other("COPY input not accepted"));
                            }
                            _ => {}
                        }
                        Ok(())
                    })
                    .await?;
                if ready {
                    return Ok(());
                }
            }
        })
        .await?;
        exchange.commit();
        drop(exchange);
        Ok(CopyIn {
            client: self,
            at,
            finished: false,
        })
    }
    /// Await an unsolicited notification. Cancelling this read closes the session.
    pub async fn notification(&mut self, at: Instant) -> io::Result<Notification> {
        deadline(&self.executor, at, async {
            loop {
                let mut notification = None;
                self.driver
                    .next(&self.executor, |event| {
                        match event {
                            Event::Notification {
                                process_id,
                                channel,
                                payload,
                            } => {
                                notification = Some(Notification {
                                    process_id,
                                    channel: channel.into(),
                                    payload: payload.into(),
                                })
                            }
                            Event::Closed { reason } => return Err(io::Error::other(reason)),
                            _ => {}
                        }
                        Ok(())
                    })
                    .await?;
                if let Some(n) = notification {
                    return Ok(n);
                }
            }
        })
        .await
    }
}
#[derive(Debug, PartialEq, Eq)]
pub struct Notification {
    pub process_id: i32,
    pub channel: String,
    pub payload: String,
}
pub struct CopyIn<'a, B: Backend, S: Stream> {
    client: &'a mut Client<B, S>,
    at: Instant,
    finished: bool,
}
impl<B: Backend, S: Stream> CopyIn<'_, B, S> {
    pub async fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.client
            .driver
            .core_mut()
            .copy_data(bytes)
            .map_err(io::Error::other)?;
        deadline(&self.client.executor, self.at, self.client.driver.flush()).await
    }
    pub async fn finish(mut self) -> io::Result<Outcome> {
        self.client
            .driver
            .core_mut()
            .copy_finish(None)
            .map_err(io::Error::other)?;
        let outcome = self.client.complete(self.at, |_| Ok(())).await?;
        self.finished = true;
        Ok(outcome)
    }
}
impl<B: Backend, S: Stream> Drop for CopyIn<'_, B, S> {
    fn drop(&mut self) {
        if !self.finished {
            self.client.close();
        }
    }
}

impl<B: Backend + 'static> turnloop_io::pool::Connection for Client<B> {
    fn reusable(&self) -> bool {
        self.is_reusable()
    }
    fn handle(&self) -> Option<turnloop_io::turnloop::Handle> {
        self.driver.stream()?.get_ref().map(AsyncIo::handle)
    }
}
