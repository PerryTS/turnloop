#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
        time::Duration,
    };
    use turnloop_io::turnloop::{Config, LocalExecutor, Timeout, backend::Platform};
    use turnloop_websocket::{Message, WebSocketStream};
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".into());
    let socket = address.parse()?;
    let mut executor = LocalExecutor::<Platform>::new(Config::default())?;
    let h = executor.handle();
    let mut task = executor.spawn_local(async move {
        let stream = h
            .connect(socket, Default::default())
            .await
            .map_err(turnloop_io::error)?;
        let mut nonce = [0; 16];
        getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
        let (mut ws, _) = WebSocketStream::connect(
            stream,
            &address,
            "/",
            nonce,
            vec![],
            &h,
            h.now() + Duration::from_secs(10),
        )
        .await?;
        ws.send(Message::Text("hello from turnloop".into())).await?;
        let message = ws
            .receive()
            .await?
            .ok_or_else(|| std::io::Error::other("missing echo"))?;
        assert_eq!(message, Message::Text("hello from turnloop".into()));
        println!("{message}");
        ws.close(&h, h.now() + Duration::from_secs(5)).await
    })?;
    while !task.is_finished() {
        executor.turn(Timeout::Forever)?;
    }
    match Pin::new(&mut task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(result)) => result?,
        _ => return Err("client cancelled".into()),
    };
    Ok(())
}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn main() {
    eprintln!("Use the browser WebSocket host capability");
}
