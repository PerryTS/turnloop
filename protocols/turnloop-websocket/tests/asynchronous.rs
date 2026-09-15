#![cfg(all(
    feature = "turnloop",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use turnloop_io::{
    turnloop::{Config, LocalExecutor, Timeout, backend::Platform},
    *,
};
use turnloop_websocket::{Message, WebSocketStream};
fn finish<T>(task: &mut turnloop::executor::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task incomplete"),
    }
}
#[test]
fn async_upgrade_echo_ping_and_clean_close() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let h2 = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
    let address = listener.local_addr().expect("address");
    let end = h.now() + Duration::from_secs(5);
    let mut server = executor
        .spawn_local(async move {
            let stream = listener.accept().await.expect("accept");
            let (mut ws, protocol) = WebSocketStream::accept(stream, &["echo"], &h, end)
                .await
                .expect("upgrade");
            assert_eq!(protocol.as_deref(), Some("echo"));
            let mut echoed = 0;
            while let Some(message) = ws.receive().await.expect("receive") {
                match message {
                    Message::Text(text) => {
                        echoed += 1;
                        ws.send(Message::Text(text)).await.expect("echo");
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            echoed
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let stream = h2
                .connect(address, Default::default())
                .await
                .expect("connect");
            let (mut ws, protocol) = WebSocketStream::connect(
                stream,
                &address.to_string(),
                "/",
                [7; 16],
                vec!["echo".into()],
                &h2,
                end,
            )
            .await
            .expect("upgrade");
            assert_eq!(protocol.as_deref(), Some("echo"));
            ws.send(Message::Ping(b"probe".as_slice().into()))
                .await
                .expect("ping");
            assert_eq!(
                ws.receive().await.expect("pong"),
                Some(Message::Pong(b"probe".as_slice().into()))
            );
            for _ in 0..100 {
                ws.send(Message::Text("hello".into())).await.expect("send");
                assert_eq!(
                    ws.receive().await.expect("echo"),
                    Some(Message::Text("hello".into()))
                );
            }
            ws.close(&h2, end).await.expect("close");
            100
        })
        .expect("spawn");
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut server), finish(&mut client));
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn node_websocket_against_async_server() {
    use std::process::Command;
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
    let address = listener.local_addr().expect("address");
    let end = h.now() + Duration::from_secs(10);
    let mut task = executor
        .spawn_local(async move {
            let stream = listener.accept().await.expect("accept");
            let (mut ws, _) = WebSocketStream::accept(stream, &[], &h, end)
                .await
                .expect("upgrade");
            let mut count = 0;
            while let Some(message) = ws.receive().await.expect("receive") {
                match message {
                    Message::Text(text) => {
                        assert_eq!(text.as_str(), "node async adapter");
                        ws.send(Message::Text(text)).await.expect("echo");
                        count += 1;
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            count
        })
        .expect("spawn");
    let mut node=Command::new("node").args(["-e",&format!("const ws=new WebSocket('ws://{address}');let count=0;ws.onopen=()=>ws.send('node async adapter');ws.onmessage=e=>{{if(e.data!=='node async adapter')process.exit(2);count++;ws.close()}};ws.onclose=e=>{{if(count!==1||!e.wasClean)process.exit(3)}};ws.onerror=()=>process.exit(4);setTimeout(()=>process.exit(5),8000).unref();")]).spawn().expect("Node 26 required");
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 1);
    executor.turn(Timeout::Now).expect("deliver socket close");
    let status = node.wait().expect("Node exit");
    assert!(status.success(), "Node status: {status}");
}

/// Server-side gate: its TCP close (or half-close) waits until the peer has sent
/// bytes the server never reads, so they are unread when it closes. Without a
/// lingering close that close sends RST and the peer reads ECONNRESET, not EOF.
#[derive(Default)]
struct CloseGate {
    closing: std::cell::Cell<bool>,
    late_sent: std::cell::Cell<bool>,
    shut: std::cell::Cell<bool>,
    discarded: std::cell::Cell<usize>,
    server: std::cell::RefCell<Option<Waker>>,
    peer: std::cell::RefCell<Option<Waker>>,
}
fn wake(slot: &std::cell::RefCell<Option<Waker>>) {
    if let Some(waker) = slot.borrow_mut().take() {
        waker.wake();
    }
}
async fn peer_until(gate: &CloseGate, ready: impl Fn(&CloseGate) -> bool) {
    std::future::poll_fn(|cx| {
        if ready(gate) {
            Poll::Ready(())
        } else {
            *gate.peer.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    })
    .await
}
struct Gated {
    inner: AsyncIo<turnloop::backend::Platform>,
    gate: std::rc::Rc<CloseGate>,
}
impl Gated {
    fn poll_gate(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if !self.gate.closing.replace(true) {
            wake(&self.gate.peer);
        }
        if self.gate.late_sent.get() {
            Poll::Ready(())
        } else {
            *self.gate.server.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
    fn ended_writes(&self) {
        self.gate.shut.set(true);
        wake(&self.gate.peer);
    }
}
impl AsyncRead for Gated {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_read(cx, bytes);
        if let (true, Poll::Ready(Ok(n))) = (this.gate.shut.get(), &result) {
            this.gate.discarded.set(this.gate.discarded.get() + n);
        }
        result
    }
}
impl AsyncWrite for Gated {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.poll_gate(cx));
        std::task::ready!(Pin::new(&mut this.inner).poll_close(cx))?;
        this.ended_writes();
        Poll::Ready(Ok(()))
    }
}
impl HalfClose for Gated {
    type Backend = turnloop::backend::Platform;
    fn executor(&self) -> Option<ExecutorHandle<Self::Backend>> {
        HalfClose::executor(&self.inner)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::task::ready!(this.poll_gate(cx));
        std::task::ready!(HalfClose::poll_shutdown(Pin::new(&mut this.inner), cx))?;
        this.ended_writes();
        Poll::Ready(Ok(()))
    }
}
/// Client transport shared with the test body, so raw bytes can follow the
/// WebSocket closing handshake on the same socket.
#[derive(Clone)]
struct Shared(std::rc::Rc<std::cell::RefCell<AsyncIo<turnloop::backend::Platform>>>);
impl AsyncRead for Shared {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.0.borrow_mut()).poll_read(cx, bytes)
    }
}
impl AsyncWrite for Shared {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.0.borrow_mut()).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.0.borrow_mut()).poll_flush(cx)
    }
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.0.borrow_mut()).poll_close(cx)
    }
}
#[test]
fn server_close_lingers_until_client_eof() {
    const LATE: &[u8] = b"bytes after the closing handshake";
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let h2 = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
    let address = listener.local_addr().expect("address");
    let end = h.now() + Duration::from_secs(5);
    let gate = std::rc::Rc::new(CloseGate::default());
    let server_gate = gate.clone();
    let mut server = executor
        .spawn_local(async move {
            let stream = Gated {
                inner: listener.accept().await.expect("accept"),
                gate: server_gate,
            };
            let (mut ws, _) = WebSocketStream::accept(stream, &[], &h, end)
                .await
                .expect("upgrade");
            ws.send(Message::Text("bye".into())).await.expect("send");
            ws.close(&h, end)
                .await
                .expect("close handshake and lingering close");
            1
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let io = h2
                .connect(address, Default::default())
                .await
                .expect("connect");
            let raw = Shared(std::rc::Rc::new(std::cell::RefCell::new(io)));
            let (mut ws, _) = WebSocketStream::connect(
                raw.clone(),
                &address.to_string(),
                "/",
                [3; 16],
                vec![],
                &h2,
                end,
            )
            .await
            .expect("upgrade");
            assert_eq!(
                ws.receive().await.expect("text"),
                Some(Message::Text("bye".into()))
            );
            // Receiving the server's Close also flushes our Close reply.
            assert!(matches!(
                ws.receive().await.expect("close"),
                Some(Message::Close(_))
            ));
            let mut raw = raw;
            peer_until(&gate, |g| g.closing.get()).await;
            write_all(&mut raw, LATE).await.expect("late bytes");
            gate.late_sent.set(true);
            wake(&gate.server);
            peer_until(&gate, |g| g.shut.get()).await;
            let mut bytes = [0; 64];
            let n = read(&mut raw, &mut bytes)
                .await
                .expect("clean EOF after the server's close, not a reset");
            assert_eq!(n, 0);
            close(&mut raw).await.expect("client close");
            gate
        })
        .expect("spawn");
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end, "close hung");
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut server), 1);
    let gate = finish(&mut client);
    assert_eq!(
        gate.discarded.get(),
        LATE.len(),
        "server drained late bytes"
    );
}
