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
use turnloop_postgres::{
    Event, Outcome,
    asynchronous::{Client, ConnectOptions, Pool},
};
const AUTH: &[u8] = b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I";
const RESULT: &[u8] = b"D\0\0\0\x0c\0\x01\0\0\0\x0242C\0\0\0\x0dSELECT 1\0Z\0\0\0\x05I";
async fn startup<S: Stream>(s: &mut S) {
    let mut length = [0; 4];
    exact(s, &mut length).await;
    let n = u32::from_be_bytes(length) as usize;
    assert!((8..1024).contains(&n));
    let mut body = [0; 1024];
    exact(s, &mut body[..n - 4]).await;
    assert_eq!(&body[..4], &[0, 3, 0, 0]);
    write_all(s, AUTH).await.expect("auth");
}
async fn query<S: Stream>(s: &mut S) -> Vec<u8> {
    let mut header = [0; 5];
    exact(s, &mut header).await;
    assert_eq!(header[0], b'Q');
    let n = u32::from_be_bytes(header[1..].try_into().expect("length")) as usize;
    let mut b = vec![0; n - 4];
    exact(s, &mut b).await;
    b
}
#[test]
fn simple_query_timeout_and_pool_reuse() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let server_h = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            startup(&mut s).await;
            for _ in 0..2 {
                assert_eq!(query(&mut s).await, b"SELECT 42\0");
                write_all(&mut s, RESULT).await.expect("result");
            }
            assert_eq!(query(&mut s).await, b"SELECT stall\0");
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("cancel EOF"), 0);
            let mut next = listener.accept().await.expect("replacement");
            startup(&mut next).await;
            assert_eq!(query(&mut next).await, b"SELECT 42\0");
            write_all(&mut next, RESULT).await.expect("result");
            let mut b = [0];
            assert_eq!(read(&mut next, &mut b).await.expect("end EOF"), 0);
            drop(server_h);
            4
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(5);
            let options = ConnectOptions {
                address,
                protocol: Default::default(),
                tls: None,
                channel_binding: None,
            };
            let pool = Pool::new(
                &h,
                options,
                turnloop_postgres::pool::Config {
                    max: 1,
                    max_idle: 1,
                    ..Default::default()
                },
                Duration::from_secs(2),
            )
            .expect("pool");
            let mut rows = 0;
            for _ in 0..2 {
                let mut c = pool.acquire(at).await.expect("checkout");
                assert_eq!(
                    c.query("SELECT 42", at, |e| {
                        if let Event::Row { mut row, .. } = e {
                            assert_eq!(
                                row.next().expect("row").expect("value"),
                                Some(b"42".as_slice())
                            );
                            rows += 1;
                        }
                        Ok(())
                    })
                    .await
                    .expect("query"),
                    Outcome::Success
                );
            }
            let mut c = pool.acquire(at).await.expect("checkout");
            assert_eq!(
                c.query("SELECT stall", h.now() + Duration::from_millis(2), |_| Ok(
                    ()
                ))
                .await
                .expect_err("timeout")
                .kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(!c.is_reusable());
            drop(c);
            let mut c = pool.acquire(at).await.expect("replacement");
            assert_eq!(
                c.query("SELECT 42", at, |_| Ok(())).await.expect("query"),
                Outcome::Success
            );
            drop(c);
            pool.end().await.expect("end");
            assert_eq!(rows, 2);
            rows
        })
        .expect("spawn");
    assert_eq!(drive(&mut ex, &mut client), 2);
    assert_eq!(drive(&mut ex, &mut server), 4);
}
#[test]
fn copy_stream_drop_closes_session() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            startup(&mut s).await;
            assert_eq!(query(&mut s).await, b"COPY t FROM STDIN\0");
            write_all(&mut s, b"G\0\0\0\x07\0\0\0")
                .await
                .expect("copy ready");
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(3);
            let mut c = Client::connect(
                &h,
                &ConnectOptions {
                    address,
                    protocol: Default::default(),
                    tls: None,
                    channel_binding: None,
                },
                at,
            )
            .await
            .expect("connect");
            drop(c.copy_in("COPY t FROM STDIN", at).await.expect("COPY"));
            assert!(!c.is_reusable());
        })
        .expect("spawn");
    drive(&mut ex, &mut client);
    drive(&mut ex, &mut server);
}

#[test]
fn warmed_async_pool_queries_allocate_zero_and_idle_waits() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            startup(&mut s).await;
            for _ in 0..1001 {
                assert_eq!(query(&mut s).await, b"SELECT 42\0");
                write_all(&mut s, RESULT).await.expect("reply");
            }
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
            1001
        })
        .expect("server");
    let idle = std::rc::Rc::new(std::cell::Cell::new(false));
    let client_idle = idle.clone();
    let mut client = ex
        .spawn_local(async move {
            count::prove_counter().await;
            let at = h.now() + Duration::from_secs(30);
            let pool = Pool::new(
                &h,
                ConnectOptions {
                    address,
                    protocol: Default::default(),
                    tls: None,
                    channel_binding: None,
                },
                turnloop_postgres::pool::Config {
                    max: 1,
                    max_idle: 1,
                    idle_timeout: Some(Duration::from_secs(60)),
                    ..Default::default()
                },
                Duration::from_secs(5),
            )
            .expect("pool");
            let held = pool.acquire(at).await.expect("hold only slot");
            let blocked = pool.acquire(h.now() + Duration::from_millis(10)).await;
            assert!(
                matches!(blocked, Err(e) if e.kind() == std::io::ErrorKind::TimedOut),
                "queued acquire deadline must run"
            );
            drop(held);
            for i in 0..1001 {
                let (rows, n) = count::measure(async {
                    let mut c = pool.acquire(at).await.expect("acquire");
                    let mut rows = 0;
                    assert_eq!(
                        c.query("SELECT 42", at, |e| {
                            if let Event::Row { mut row, .. } = e {
                                assert_eq!(
                                    row.next().expect("row").expect("value"),
                                    Some(b"42".as_slice())
                                );
                                rows += 1;
                            }
                            Ok(())
                        })
                        .await
                        .expect("query"),
                        Outcome::Success
                    );
                    rows
                })
                .await;
                assert_eq!(rows, 1);
                if i > 0 {
                    assert_eq!(n, 0, "query + checkout/release must allocate zero");
                }
            }
            client_idle.set(true);
            h.sleep(Duration::from_millis(60))
                .await
                .expect("idle timer");
            pool.end().await.expect("end");
        })
        .expect("client");
    let end = ex.handle().now() + Duration::from_secs(30);
    while !idle.get() {
        assert!(ex.handle().now() < end);
        ex.turn(Timeout::Until(end)).expect("turn");
    }
    // Drain finite completions; then the pooled idle socket has no readiness
    // subscription, and a turn must actually wait for the timer.
    let mut waited = false;
    for _ in 0..8 {
        let before = ex.handle().now();
        let info = ex.turn(Timeout::Until(end)).expect("idle turn");
        if ex.handle().now().duration_since(before) >= Duration::from_millis(10) {
            assert_eq!(info.os_waits, 1);
            waited = true;
            break;
        }
    }
    assert!(waited, "idle pooled connection spun instead of parking");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 1001);
}
