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
use turnloop_mysql::{
    Event, Outcome, Value,
    asynchronous::{ConnectOptions, Connection, Pool},
};
#[test]
#[ignore = "private MySQL fixture; sandbox initializer crash is UNRUN"]
fn real_async_tls_rsa_text_binary_multi_results_transactions_pool() {
    let ca = std::fs::read(
        std::path::PathBuf::from(std::env::var_os("TURNLOOP_TEST_SQL_TOOLS").expect("SQL tools"))
            .join("server.der"),
    )
    .expect("CA");
    let options = ConnectOptions {
        address: ([127, 0, 0, 1], port("TURNLOOP_TEST_MYSQL_PORT")).into(),
        protocol: turnloop_mysql::Config {
            user: "tls_user".into(),
            password: b"fixture-password".to_vec(),
            database: Some("turnloop_test".into()),
            tls: true,
            multiple_statements: true,
            ..Default::default()
        },
        tls: Some(tls(ca)),
    };
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let mut task = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(30);
            let mut c = Connection::connect(&h, &options, at)
                .await
                .expect("TLS auth");
            let mut rows = 0;
            assert_eq!(
                c.query("SELECT 42; SELECT 43", at, |e| {
                    if let Event::Row { .. } = e {
                        rows += 1;
                    }
                    Ok(())
                })
                .await
                .expect("multi results"),
                Outcome::Success
            );
            assert_eq!(rows, 2);
            let stmt = c.prepare("SELECT ?", at).await.expect("prepare");
            assert_eq!(stmt.parameters, 1);
            let mut rows = 0;
            assert_eq!(
                c.execute(stmt, &[Value::Int(42)], at, |e| {
                    if let Event::Row { .. } = e {
                        rows += 1;
                    }
                    Ok(())
                })
                .await
                .expect("binary"),
                Outcome::Success
            );
            assert_eq!(rows, 1);
            c.close_statement(stmt, at).await.expect("close statement");
            assert_eq!(
                c.begin(at)
                    .await
                    .expect("begin")
                    .rollback(at)
                    .await
                    .expect("rollback"),
                Outcome::Success
            );
            let mut admin_options = options.clone();
            admin_options.protocol.user = "auth_admin".into();
            let mut admin = Connection::connect(&h, &admin_options, at)
                .await
                .expect("auth admin");
            assert_eq!(
                admin
                    .query("FLUSH PRIVILEGES", at, |_| Ok(()))
                    .await
                    .expect("clear SHA2 cache"),
                Outcome::Success
            );
            let mut rsa = options.clone();
            rsa.protocol.user = "auth_rsa_user".into();
            rsa.protocol.tls = false;
            rsa.tls = None;
            let mut rsa = Connection::connect(&h, &rsa, at).await.expect("RSA auth");
            assert!(
                rsa.rsa_authenticated,
                "full RSA path must execute after cache flush"
            );
            assert_eq!(rsa.ping(at).await.expect("RSA ping"), Outcome::Success);
            let pool = Pool::new(
                &h,
                options,
                turnloop_mysql::pool::Config {
                    max: 1,
                    max_idle: 1,
                    ..Default::default()
                },
                Duration::from_secs(5),
            )
            .expect("pool");
            let id = {
                let c = pool.acquire(at).await.expect("checkout");
                c.connection_id
            };
            let pooled = pool.acquire(at).await.expect("reuse");
            assert_eq!(pooled.connection_id, id);
            drop(pooled);
            pool.end().await.expect("pool end");
            assert_eq!(
                c.query(
                    "SELECT SLEEP(10)",
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
