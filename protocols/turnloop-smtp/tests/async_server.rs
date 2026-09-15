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
use turnloop_smtp::{
    Envelope, Tls,
    asynchronous::{ConnectOptions, Transport},
};
#[test]
#[ignore = "private Postfix smtp-sink fixture"]
fn real_async_postfix_delivery() {
    let port = std::env::var("TURNLOOP_TEST_SMTP_PORT")
        .expect("private SMTP port")
        .parse::<u16>()
        .expect("port");
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let mut task = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(5);
            let mut t = Transport::connect(
                &h,
                &ConnectOptions {
                    address: ([127, 0, 0, 1], port).into(),
                    protocol: turnloop_smtp::Config {
                        tls: Tls::None,
                        ..Default::default()
                    },
                    tls: None,
                },
                at,
            )
            .await
            .expect("SMTP connect");
            let info = t
                .send(
                    Envelope {
                        from: "sender@example.test".into(),
                        to: vec!["recipient@example.test".into()],
                    },
                    "async-fixture".into(),
                    b"Subject: async-turnloop\r\n\r\nasync delivery bytes\r\n",
                    at,
                )
                .await
                .expect("delivery");
            assert_eq!(info.response_code, 250);
            assert_eq!(info.accepted, ["recipient@example.test"]);
            assert!(info.rejected.is_empty());
        })
        .expect("spawn");
    drive(&mut ex, &mut task);
    let root = std::path::PathBuf::from(
        std::env::var_os("TURNLOOP_TEST_SMTP_TOOLS").expect("dump directory"),
    );
    let matching = std::fs::read_dir(root)
        .expect("dumps")
        .filter_map(|e| {
            let p = e.expect("entry").path();
            if !p
                .file_name()
                .expect("filename")
                .to_string_lossy()
                .starts_with("message-")
            {
                return None;
            }
            let bytes = std::fs::read(p).expect("message");
            bytes
                .windows(b"Subject: async-turnloop".len())
                .any(|b| b == b"Subject: async-turnloop")
                .then_some(bytes)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "async delivery must reach the actual sink"
    );
    assert!(
        matching[0]
            .windows(b"async delivery bytes".len())
            .any(|b| b == b"async delivery bytes")
    );
}
