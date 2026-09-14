#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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

fn port(name: &str) -> u16 {
    std::env::var(name)
        .expect("required private fixture port; run scripts/test-servers.py")
        .parse()
        .expect("port")
}
fn tls(ca: Vec<u8>) -> turnloop_tls::asynchronous::ClientTls {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall time")
        .as_secs();
    turnloop_tls::asynchronous::ClientTls {
        config: turnloop_tls::ClientConfig::new(
            turnloop_tls::ClientOptions {
                ca: Some(vec![turnloop_tls::rustls::pki_types::CertificateDer::from(
                    ca,
                )]),
                alpn: vec![],
                ..Default::default()
            },
            now,
        )
        .expect("TLS config"),
        server_name: "localhost".try_into().expect("name"),
        unix_seconds: now,
    }
}
use turnloop_postgres::{
    Event, ExtendedQuery, Outcome, Parameter, SslMode,
    asynchronous::{Client, ConnectOptions, Pool},
};
#[test]
#[ignore = "private PostgreSQL fixture; sandbox shmget is UNRUN"]
fn real_async_tls_queries_copy_cancel_pool_and_connection_kill() {
    let ca = std::fs::read(
        std::path::PathBuf::from(std::env::var_os("TURNLOOP_TEST_SQL_TOOLS").expect("SQL tools"))
            .join("server.der"),
    )
    .expect("CA");
    let options = ConnectOptions {
        address: ([127, 0, 0, 1], port("TURNLOOP_TEST_POSTGRES_PORT")).into(),
        protocol: turnloop_postgres::Config {
            user: "tls_user".into(),
            password: b"fixture-password".to_vec(),
            ssl: SslMode::Require,
            ..Default::default()
        },
        tls: Some(tls(ca)),
        channel_binding: None,
    };
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let mut task = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(30);
            let mut c = Client::connect(&h, &options, at).await.expect("TLS auth");
            let mut observed = 0;
            assert_eq!(
                c.query("SELECT 42; SELECT 43", at, |e| {
                    if let Event::Row { .. } = e {
                        observed += 1;
                    }
                    Ok(())
                })
                .await
                .expect("simple"),
                Outcome::Success
            );
            assert_eq!(observed, 2);
            let q = ExtendedQuery {
                name: "answer",
                sql: "SELECT $1::int4",
                oids: &[23],
                params: &[Parameter {
                    value: Some(b"42"),
                    format: 0,
                }],
                result_formats: &[0],
            };
            for _ in 0..2 {
                assert_eq!(
                    c.execute(q, at, |e| {
                        if let Event::Row { mut row, .. } = e {
                            assert_eq!(
                                row.next().expect("row").expect("value"),
                                Some(b"42".as_slice())
                            );
                        }
                        Ok(())
                    })
                    .await
                    .expect("prepared"),
                    Outcome::Success
                );
            }
            assert_eq!(
                c.query("CREATE TEMP TABLE async_copy(v int)", at, |_| Ok(()))
                    .await
                    .expect("create"),
                Outcome::Success
            );
            let mut copy = c
                .copy_in("COPY async_copy FROM STDIN", at)
                .await
                .expect("COPY IN");
            copy.write(b"1\n2\n").await.expect("chunk");
            assert_eq!(copy.finish().await.expect("finish"), Outcome::Success);
            let mut copied = Vec::new();
            assert_eq!(
                c.copy_out("COPY async_copy TO STDOUT", at, |b| {
                    copied.extend_from_slice(b);
                    Ok(())
                })
                .await
                .expect("COPY OUT"),
                Outcome::Success
            );
            assert_eq!(copied, b"1\n2\n");
            c.query("LISTEN async_channel", at, |_| Ok(()))
                .await
                .expect("LISTEN");
            let mut other = Client::connect(&h, &options, at).await.expect("other");
            other
                .query("NOTIFY async_channel, 'payload'", at, |_| Ok(()))
                .await
                .expect("NOTIFY");
            let n = c.notification(at).await.expect("notification");
            assert_eq!(n.channel, "async_channel");
            assert_eq!(n.payload, "payload");
            let cancel = c.cancel_token().expect("backend cancel key");
            let cancel_h = h.clone();
            let mut canceller = h
                .spawn_local(async move {
                    cancel_h
                        .sleep(Duration::from_millis(20))
                        .await
                        .expect("timer");
                    cancel.cancel(&cancel_h, at).await.expect("CancelRequest");
                })
                .expect("cancel task");
            let mut cancelled = false;
            assert_eq!(
                c.query("SELECT pg_sleep(10)", at, |e| {
                    if let Event::Error { error, .. } = e {
                        assert_eq!(error.code(), "57014");
                        cancelled = true;
                    }
                    Ok(())
                })
                .await
                .expect("cancel result"),
                Outcome::ServerError
            );
            assert!(cancelled);
            (&mut canceller).await.expect("cancel finished");
            assert_eq!(
                c.query("SELECT 1", at, |_| Ok(())).await.expect("reuse"),
                Outcome::Success
            );
            let pool = Pool::new(
                &h,
                options,
                turnloop_postgres::pool::Config {
                    max: 1,
                    max_idle: 1,
                    max_uses: Some(2),
                    ..Default::default()
                },
                Duration::from_secs(5),
            )
            .expect("pool");
            for _ in 0..3 {
                let mut c = pool.acquire(at).await.expect("checkout");
                assert_eq!(
                    c.query("SELECT 42", at, |_| Ok(()))
                        .await
                        .expect("pooled query"),
                    Outcome::Success
                );
            }
            pool.end().await.expect("end");
            assert_eq!(
                c.query(
                    "SELECT pg_sleep(10)",
                    h.now() + Duration::from_millis(2),
                    |_| Ok(())
                )
                .await
                .expect_err("cancel future")
                .kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(!c.is_reusable());
        })
        .expect("spawn");
    drive(&mut ex, &mut task);
}
