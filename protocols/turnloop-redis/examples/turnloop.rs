//! A local plaintext development-server example. For production TLS, supply
//! ClientTls and enable the protocol's TLS option; see the crate README.
use std::time::Duration;
use turnloop_io::{Backend, ExecutorHandle};
pub async fn example<B: Backend + 'static>(h: ExecutorHandle<B>) -> std::io::Result<()> {
    let address = std::env::var("TURNLOOP_DB_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:6379".into())
        .parse::<std::net::SocketAddr>()
        .map_err(std::io::Error::other)?;
    let at = h.now() + Duration::from_secs(30);
    use turnloop_redis::asynchronous::{Client, ConnectOptions};
    let options = ConnectOptions {
        address,
        protocol: turnloop_redis::Config {
            username: std::env::var("REDIS_USER").ok(),
            password: std::env::var("REDIS_PASSWORD").ok(),
            ..Default::default()
        },
        tls: None,
        retry_delay: Duration::from_millis(100),
        max_reconnects: 3,
    };
    let mut client = Client::connect(&h, &options, at).await?;
    println!("{:?}", client.command(&[b"PING"], at).await?);
    Ok(())
}
#[cfg(not(any(windows, all(target_arch = "wasm32", target_os = "unknown"))))]
fn main() -> std::io::Result<()> {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use turnloop_io::turnloop::{Config, LocalExecutor, Timeout, backend::Platform};
    let mut executor =
        LocalExecutor::<Platform>::new(Config::default()).map_err(turnloop_io::error)?;
    let h = executor.handle();
    let mut task = executor
        .spawn_local(example(h))
        .map_err(turnloop_io::error)?;
    while !task.is_finished() {
        executor
            .turn(Timeout::After(Duration::from_secs(30)))
            .map_err(turnloop_io::error)?;
    }
    match Pin::new(&mut task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(result)) => result,
        _ => Err(std::io::Error::other("example task did not complete")),
    }
}
#[cfg(any(windows, all(target_arch = "wasm32", target_os = "unknown")))]
fn main() {
    eprintln!(
        "Embed example(handle) with a host-provided TCP Backend; this target has no built-in TCP Platform provider."
    );
}
