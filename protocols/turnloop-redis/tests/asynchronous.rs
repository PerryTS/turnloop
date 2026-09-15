#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[path = "../../../crates/turnloop-io/tests/support/count.rs"]
mod count;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use turnloop_io::{
    turnloop::{Config as LoopConfig, LocalExecutor, Timeout, backend::Platform},
    *,
};
fn finish<T>(task: &mut turnloop::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task must finish"),
    }
}
fn drive<T>(executor: &mut LocalExecutor<Platform>, task: &mut turnloop::JoinHandle<T>) -> T {
    let at = executor.handle().now() + Duration::from_secs(90);
    while !task.is_finished() {
        assert!(executor.handle().now() < at, "test deadline");
        executor.turn(Timeout::Until(at)).expect("turn");
    }
    finish(task)
}
async fn exact<S: Stream>(s: &mut S, mut bytes: &mut [u8]) {
    while !bytes.is_empty() {
        let n = read(s, bytes).await.expect("read");
        assert!(n > 0, "unexpected EOF");
        bytes = &mut bytes[n..];
    }
}
use turnloop_redis::{
    asynchronous::{Client, ConnectOptions},
    resp::Value,
};
fn options(address: std::net::SocketAddr) -> ConnectOptions {
    ConnectOptions {
        address,
        protocol: turnloop_redis::Config {
            prefer_resp3: false,
            ..Default::default()
        },
        tls: None,
        retry_delay: Duration::from_millis(2),
        max_reconnects: 2,
    }
}
async fn incr<S: Stream>(s: &mut S) {
    let mut b = [0; 27];
    exact(s, &mut b).await;
    assert_eq!(&b, b"*2\r\n$4\r\nINCR\r\n$7\r\ncounter\r\n");
}
#[test]
fn pipeline_reconnect_replays_and_timeout_closes() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut first = listener.accept().await.expect("accept");
            incr(&mut first).await;
            drop(first);
            let mut s = listener.accept().await.expect("reconnect");
            incr(&mut s).await;
            incr(&mut s).await;
            write_all(&mut s, b":1\r\n:2\r\n").await.expect("replies");
            incr(&mut s).await;
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("cancel EOF"), 0);
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(3);
            let mut c = Client::connect(&h, &options(address), at)
                .await
                .expect("connect");
            let mut replies = 0;
            c.pipeline(
                &[&[b"INCR", b"counter"], &[b"INCR", b"counter"]],
                at,
                |i, value| {
                    assert_eq!(value.expect("reply"), Value::Integer(i as i64 + 1));
                    replies += 1;
                    Ok(())
                },
            )
            .await
            .expect("pipeline");
            assert_eq!(replies, 2);
            assert_eq!(c.reconnect_count(), 1);
            assert_eq!(
                c.command(&[b"INCR", b"counter"], h.now() + Duration::from_millis(2))
                    .await
                    .expect_err("deadline")
                    .kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(!c.is_connected());
        })
        .expect("spawn");
    drive(&mut ex, &mut client);
    drive(&mut ex, &mut server);
}
#[test]
fn warmed_async_pipeline_allocates_zero() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            for _ in 0..1001 {
                incr(&mut s).await;
                incr(&mut s).await;
                write_all(&mut s, b":41\r\n:42\r\n").await.expect("reply");
            }
            2002
        })
        .expect("server");
    let mut client = ex
        .spawn_local(async move {
            count::prove_counter().await;
            let at = h.now() + Duration::from_secs(30);
            let mut c = Client::connect(&h, &options(address), at)
                .await
                .expect("connect");
            let mut replies = 0;
            for i in 0..1001 {
                let (result, n) = count::measure(c.pipeline(
                    &[&[b"INCR", b"counter"], &[b"INCR", b"counter"]],
                    at,
                    |j, v| {
                        assert_eq!(v.expect("value"), Value::Integer(41 + j as i64));
                        replies += 1;
                        Ok(())
                    },
                ))
                .await;
                result.expect("pipeline");
                if i > 0 {
                    assert_eq!(n, 0, "pipeline async overhead");
                }
            }
            assert_eq!(replies, 2002);
        })
        .expect("client");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 2002);
}
