//! A local plaintext development-server example. For production TLS, supply
//! ClientTls and enable the protocol's TLS option; see the crate README.
use std::time::Duration;
use turnloop_io::{Backend, ExecutorHandle};
pub async fn example<B: Backend + 'static>(h: ExecutorHandle<B>) -> std::io::Result<()> {
    let address = std::env::var("TURNLOOP_DB_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:27017".into())
        .parse::<std::net::SocketAddr>()
        .map_err(std::io::Error::other)?;
    let at = h.now() + Duration::from_secs(30);
    use turnloop_mongodb::{
        asynchronous::{Client, ConnectOptions},
        bson::{doc, raw::RawDocumentBuf},
        operation::OperationKind,
        uri::Options,
    };
    let uri = std::env::var("MONGODB_URI")
        .unwrap_or_else(|_| format!("mongodb://{address}/?directConnection=true"));
    let options = ConnectOptions {
        protocol: Options::parse(&uri).map_err(std::io::Error::other)?,
        tls: None,
    };
    let mut client = Client::connect(&h, options, at).await?;
    let command =
        RawDocumentBuf::try_from(&doc! {"ping":1,"$db":"admin"}).map_err(std::io::Error::other)?;
    client
        .command(&command, &[], OperationKind::RunCommand, at, |reply| {
            println!("{:?}", reply);
            Ok(())
        })
        .await?;
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
