#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use turnloop_http::asynchronous::server::Server;
    use turnloop_io::turnloop::{Config, LocalExecutor, Timeout, backend::Platform};
    use turnloop_websocket::{Message, WebSocketStream};
    let mut executor = LocalExecutor::<Platform>::new(Config::default())?;
    let h = executor.handle();
    let mut server = Server::bind(
        h.clone(),
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "127.0.0.1:8080".into())
            .parse()?,
    )?;
    println!("{}", server.local_addr()?);
    let task = executor.spawn_local(async move {
        server
            .run(move |stream, signal| {
                let h = h.clone();
                async move {
                    let (mut ws, _) =
                        WebSocketStream::accept(stream, &[], &h, h.now() + Duration::from_secs(10))
                            .await?;
                    while let Some(message) = signal.until(ws.receive()).await {
                        match message? {
                            Some(Message::Text(text)) => ws.send(Message::Text(text)).await?,
                            Some(Message::Binary(data)) => ws.send(Message::Binary(data)).await?,
                            Some(Message::Close(_)) | None => break,
                            _ => {}
                        }
                    }
                    Ok(())
                }
            })
            .await
    })?;
    while !task.is_finished() {
        executor.turn(Timeout::Forever)?;
    }
    Ok(())
}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn main() {
    eprintln!("Browser listening sockets are unsupported");
}
