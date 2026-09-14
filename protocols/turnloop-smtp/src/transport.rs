//! SMTP delivery on the host's executor with certificate-verified TLS upgrades.
use super::AsyncConnection;
use crate::{Config, Envelope, Event, Rejection, SendInfo};
use std::{io, net::SocketAddr};
use turnloop_io::{AsyncIo, Backend, ExecutorHandle, Instant, Stream, deadline};
use turnloop_tls::asynchronous::{ClientTls, Transport as Wire};
#[derive(Clone)]
pub struct ConnectOptions {
    pub address: SocketAddr,
    pub protocol: Config,
    pub tls: Option<ClientTls>,
}
/// Complete failure details, including recipient responses already received.
#[derive(Debug)]
pub struct SendFailure {
    pub error: crate::Error,
    pub envelope: Option<Envelope>,
    pub accepted: Vec<String>,
    pub rejected: Vec<Rejection>,
}
impl std::fmt::Display for SendFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for SendFailure {}
pub struct Transport<B: Backend, S: Stream = AsyncIo<B>> {
    executor: ExecutorHandle<B>,
    driver: AsyncConnection<Wire<S>>,
}
fn now<B: Backend>(executor: &ExecutorHandle<B>) -> io::Result<std::time::Instant> {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        let _ = executor;
        Err(io::ErrorKind::Unsupported.into())
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        Ok(executor.now())
    }
}
impl<B: Backend> Transport<B> {
    pub async fn connect(
        executor: &ExecutorHandle<B>,
        options: &ConnectOptions,
        at: Instant,
    ) -> io::Result<Self> {
        now(executor)?;
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
impl<B: Backend, S: Stream> Transport<B, S> {
    /// Returns only after greeting, EHLO, TLS and authentication have completed.
    pub async fn from_stream(
        executor: &ExecutorHandle<B>,
        stream: S,
        config: Config,
        tls: Option<&ClientTls>,
        at: Instant,
    ) -> io::Result<Self> {
        let mut core = crate::Connection::new(config).map_err(io::Error::other)?;
        core.connected(now(executor)?).map_err(io::Error::other)?;
        let mut driver = AsyncConnection::new(Wire::Plain(stream), core);
        deadline(executor, at, async {
            loop {
                let mut upgrade = false;
                let mut ready = false;
                driver
                    .next(executor, |event| {
                        match event {
                            Event::UpgradeTls => upgrade = true,
                            Event::Ready => ready = true,
                            Event::Failed { error, .. } => return Err(io::Error::other(error)),
                            Event::Closed | Event::CloseTransport => {
                                return Err(io::ErrorKind::UnexpectedEof.into());
                            }
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
                        .tls_established(now(executor)?)
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
        })
    }
    pub fn capabilities(&self) -> &crate::Capabilities {
        self.driver.core().capabilities()
    }
    pub fn is_reusable(&self) -> bool {
        self.driver.is_connected() && self.driver.core().state() == crate::State::Ready
    }
    pub fn close(&mut self) {
        self.driver.abort();
    }
    /// MAIL/RCPT use PIPELINING when negotiated. Per-recipient failures are
    /// preserved both on successful partial delivery and on total failure.
    pub async fn send(
        &mut self,
        envelope: Envelope,
        message_id: String,
        message: &[u8],
        at: Instant,
    ) -> io::Result<SendInfo> {
        self.driver
            .core_mut()
            .send(1, envelope, message_id, message, now(&self.executor)?)
            .map_err(io::Error::other)?;
        let mut exchange = self.driver.exchange();
        let result = deadline(&self.executor, at, async {
            loop {
                let mut result = None;
                exchange
                    .next(&self.executor, |event| {
                        match event {
                            Event::Sent { info, .. } => result = Some(Ok(info)),
                            Event::Failed {
                                error,
                                envelope,
                                accepted,
                                rejected,
                                ..
                            } => {
                                result = Some(Err(io::Error::other(SendFailure {
                                    error,
                                    envelope,
                                    accepted,
                                    rejected,
                                })))
                            }
                            Event::Closed | Event::CloseTransport => {
                                return Err(io::ErrorKind::UnexpectedEof.into());
                            }
                            _ => {}
                        }
                        Ok(())
                    })
                    .await?;
                if let Some(result) = result {
                    return result;
                }
            }
        })
        .await?;
        exchange.commit();
        Ok(result)
    }
}
