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
fn tcp_roundtrip_deadline_and_cancelled_accept() {
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
