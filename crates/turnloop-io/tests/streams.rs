#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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
fn finish<T>(task: &mut turnloop::executor::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task must complete successfully"),
    }
}
#[test]
fn tcp_roundtrip_and_deadline() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            let mut data = [0; 5];
            let mut n = 0;
            while n < data.len() {
                let got = read(&mut s, &mut data[n..]).await.expect("read");
                assert!(got > 0);
                n += got;
            }
            assert_eq!(&data, b"hello");
            write_all(&mut s, b"world").await.expect("write");
            n
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let mut s = h
                .connect(address, Default::default())
                .await
                .expect("connect");
            write_all(&mut s, b"hello").await.expect("write");
            let mut data = [0; 5];
            let mut n = 0;
            while n < data.len() {
                let got = read(&mut s, &mut data[n..]).await.expect("read");
                assert!(got > 0);
                n += got;
            }
            assert_eq!(&data, b"world");
            n
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(3);
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut server) + finish(&mut client), 10);
    let h = executor.handle();
    let mut timeout = executor
        .spawn_local(async move {
            let at = h.now() + Duration::from_millis(2);
            let result = deadline(&h, at, std::future::pending::<std::io::Result<()>>()).await;
            assert_eq!(
                result.expect_err("deadline").kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(h.now() >= at);
        })
        .expect("spawn");
    while !timeout.is_finished() {
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    finish(&mut timeout);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn resolve_and_cancel_reuse_executor_slots() {
    use turnloop::executor::ExecutorConfig;
    let mut executor = LocalExecutor::<Platform>::with_config(
        Config::default(),
        ExecutorConfig {
            tasks: 4,
            operations: 4,
            buffer_size: 256,
        },
    )
    .expect("executor");
    let h = executor.handle();
    let mut task = executor.spawn_local(async move {
        let mut completed = 0;
        for _ in 0..16 {
            let mut lookup = h.resolve(turnloop::DnsRequest {host: "localhost".into(), port: 4242});
            std::future::poll_fn(|cx| {
                assert!(Pin::new(&mut lookup).poll(cx).is_pending());
                Poll::Ready(())
            }).await;
            drop(lookup);
            // Give the cancelled operation its terminal completion before reusing
            // a bounded four-slot executor. No late result may complete this timer.
            h.sleep(Duration::from_millis(2)).await.expect("cancel settlement");
            let mut lookup = h.resolve(turnloop::DnsRequest {host: "localhost".into(), port: 4242});
            let addresses = (&mut lookup).await.expect("resolve");
            assert!(!addresses.is_empty());
            assert!(addresses.iter().all(|a| a.ip().is_loopback() && a.port() == 4242));
            assert!(matches!(Pin::new(&mut lookup).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Err(e)) if e.kind == turnloop::ErrorKind::InvalidInput));
            completed += 1;
        }
        completed
    }).expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 16);
}

#[cfg(target_os = "wasi")]
#[test]
fn unsupported_wasi_dns_releases_reserved_slots() {
    use turnloop::executor::ExecutorConfig;
    let mut executor = LocalExecutor::<Platform>::with_config(
        Config::default(),
        ExecutorConfig {
            tasks: 4,
            operations: 4,
            buffer_size: 256,
        },
    )
    .expect("executor");
    let h = executor.handle();
    let mut task = executor
        .spawn_local(async move {
            for _ in 0..16 {
                let error = h
                    .resolve(turnloop::DnsRequest {
                        host: "localhost".into(),
                        port: 4242,
                    })
                    .await
                    .expect_err("WASI has no native blocking DNS pool");
                assert_eq!(error.kind, turnloop::ErrorKind::Unsupported);
                h.sleep(Duration::from_millis(1))
                    .await
                    .expect("slot released on submission error");
            }
            16
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 16);
}
