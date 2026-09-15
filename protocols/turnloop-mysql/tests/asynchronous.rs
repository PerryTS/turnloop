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

use mysql_common::constants::CapabilityFlags as Caps;
use turnloop_mysql::{
    Outcome,
    asynchronous::{ConnectOptions, Connection, Pool},
};
fn frame(seq: u8, body: &[u8]) -> Vec<u8> {
    let mut b = (body.len() as u32).to_le_bytes()[..3].to_vec();
    b.push(seq);
    b.extend_from_slice(body);
    b
}
fn handshake(plugin: &str, extra: Caps) -> Vec<u8> {
    let caps = Caps::CLIENT_PROTOCOL_41
        | Caps::CLIENT_SECURE_CONNECTION
        | Caps::CLIENT_PLUGIN_AUTH
        | Caps::CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA
        | Caps::CLIENT_LONG_PASSWORD
        | Caps::CLIENT_MULTI_RESULTS
        | Caps::CLIENT_PS_MULTI_RESULTS
        | Caps::CLIENT_TRANSACTIONS
        | extra;
    let mut b = vec![10];
    b.extend_from_slice(b"9.6.0\0");
    b.extend_from_slice(&7u32.to_le_bytes());
    b.extend_from_slice(b"12345678\0");
    b.extend_from_slice(&(caps.bits() as u16).to_le_bytes());
    b.push(45);
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&((caps.bits() >> 16) as u16).to_le_bytes());
    b.push(21);
    b.extend_from_slice(&[0; 10]);
    b.extend_from_slice(b"901234567890\0");
    b.extend_from_slice(plugin.as_bytes());
    b.push(0);
    frame(0, &b)
}
fn ok(seq: u8, affected: u8, status: u16) -> Vec<u8> {
    let mut b = vec![0, affected, 0];
    b.extend_from_slice(&status.to_le_bytes());
    b.extend_from_slice(&[0, 0]);
    frame(seq, &b)
}
async fn packet<S: Stream>(s: &mut S) -> Vec<u8> {
    let mut header = [0; 4];
    exact(s, &mut header).await;
    let n = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    assert!(n < 1024);
    let mut body = vec![0; n];
    exact(s, &mut body).await;
    body
}
#[test]
fn prepare_binary_execute_transaction_and_cancel() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            write_all(&mut s, &handshake("mysql_native_password", Caps::empty()))
                .await
                .expect("hello");
            assert!(!packet(&mut s).await.is_empty());
            write_all(&mut s, &ok(2, 0, 2)).await.expect("auth");
            assert_eq!(packet(&mut s).await, b"\x16SELECT 42");
            write_all(&mut s, &frame(1, &[0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .await
                .expect("prepared");
            let execute = packet(&mut s).await;
            assert_eq!(execute[0], 0x17);
            assert_eq!(&execute[1..5], &9u32.to_le_bytes());
            write_all(&mut s, &ok(1, 5, 2)).await.expect("execute");
            assert_eq!(packet(&mut s).await, b"\x03START TRANSACTION");
            write_all(&mut s, &ok(1, 0, 3)).await.expect("begin");
            assert_eq!(packet(&mut s).await, b"\x03ROLLBACK");
            write_all(&mut s, &ok(1, 0, 2)).await.expect("rollback");
            assert_eq!(packet(&mut s).await, b"\x03SELECT stall");
            let mut b = [0];
            assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
            5
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(5);
            let mut c = Connection::connect(
                &h,
                &ConnectOptions {
                    address,
                    protocol: Default::default(),
                    tls: None,
                },
                at,
            )
            .await
            .expect("connect");
            assert_eq!(c.connection_id, 7);
            let stmt = c.prepare("SELECT 42", at).await.expect("prepare");
            assert_eq!(stmt.id, 9);
            assert_eq!(
                c.execute(stmt, &[], at, |_| Ok(())).await.expect("execute"),
                Outcome::Success
            );
            assert_eq!(
                c.begin(at)
                    .await
                    .expect("begin")
                    .rollback(at)
                    .await
                    .expect("rollback"),
                Outcome::Success
            );
            assert!(c.is_reusable());
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
        })
        .expect("spawn");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 5);
}
#[test]
fn pool_max_uses_opens_replacement() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            for _ in 0..2 {
                let mut s = listener.accept().await.expect("accept");
                write_all(&mut s, &handshake("mysql_native_password", Caps::empty()))
                    .await
                    .expect("hello");
                assert!(!packet(&mut s).await.is_empty());
                write_all(&mut s, &ok(2, 0, 2)).await.expect("auth");
                assert_eq!(packet(&mut s).await, [14]);
                write_all(&mut s, &ok(1, 0, 2)).await.expect("ping");
                let mut b = [0];
                assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
            }
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(5);
            let pool = Pool::new(
                &h,
                ConnectOptions {
                    address,
                    protocol: Default::default(),
                    tls: None,
                },
                turnloop_mysql::pool::Config {
                    max: 1,
                    max_idle: 1,
                    max_uses: Some(1),
                    ..Default::default()
                },
                Duration::from_secs(2),
            )
            .expect("pool");
            for _ in 0..2 {
                let mut c = pool.acquire(at).await.expect("checkout");
                assert_eq!(c.ping(at).await.expect("ping"), Outcome::Success);
            }
            pool.end().await.expect("end");
        })
        .expect("spawn");
    drive(&mut ex, &mut client);
    drive(&mut ex, &mut server);
}
#[test]
fn warmed_async_pool_ping_allocates_zero() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            write_all(&mut s, &handshake("mysql_native_password", Caps::empty()))
                .await
                .expect("hello");
            assert!(!packet(&mut s).await.is_empty());
            write_all(&mut s, &ok(2, 0, 2)).await.expect("auth");
            for _ in 0..1001 {
                assert_eq!(packet(&mut s).await, [14]);
                write_all(&mut s, &ok(1, 0, 2)).await.expect("ping");
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
                },
                turnloop_mysql::pool::Config {
                    max: 1,
                    max_idle: 1,
                    idle_timeout: Some(Duration::from_millis(100)),
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
                let (result, n) = count::measure(async {
                    let mut c = pool.acquire(at).await.expect("acquire");
                    c.ping(at).await
                })
                .await;
                assert_eq!(result.expect("ping"), Outcome::Success);
                if i > 0 {
                    assert_eq!(n, 0, "async ping + checkout/release");
                }
            }
            assert_eq!(pool.total(), 1);
            client_idle.set(true);
            h.sleep(Duration::from_millis(150))
                .await
                .expect("idle expiry timer");
            assert_eq!(pool.total(), 0, "real idle deadline must retire the socket");
            pool.end().await.expect("end");
        })
        .expect("client");
    let end = ex.handle().now() + Duration::from_secs(30);
    while !idle.get() {
        assert!(ex.handle().now() < end);
        ex.turn(Timeout::Until(end)).expect("turn");
    }
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
    assert!(waited, "idle MySQL pool spun instead of parking");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 1001);
}
