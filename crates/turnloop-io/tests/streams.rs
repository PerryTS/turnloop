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

async fn read_to_eof<S: AsyncRead + Unpin>(stream: &mut S) -> Vec<u8> {
    let mut received = Vec::new();
    let mut bytes = [0; 256];
    loop {
        // A reset here (instead of EOF) is the failure lingering close prevents.
        let n = read(stream, &mut bytes)
            .await
            .expect("clean EOF, not a reset");
        if n == 0 {
            return received;
        }
        received.extend_from_slice(&bytes[..n]);
    }
}
#[test]
fn half_close_keeps_reading_until_peer_eof() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            write_all(&mut s, b"response").await.expect("write");
            assert!(!s.is_write_shut());
            shutdown(&mut s).await.expect("half-close");
            assert!(s.is_write_shut());
            shutdown(&mut s)
                .await
                .expect("a completed half-close stays complete");
            assert!(
                write_all(&mut s, b"x").await.is_err(),
                "writes end at the half-close"
            );
            let late = read_to_eof(&mut s).await;
            close(&mut s).await.expect("close");
            late
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let mut s = h
                .connect(address, Default::default())
                .await
                .expect("connect");
            let response = read_to_eof(&mut s).await;
            // The peer ended only its write direction: this one still delivers.
            write_all(&mut s, b"late request")
                .await
                .expect("write after the peer's half-close");
            shutdown(&mut s).await.expect("client half-close");
            response
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut client), b"response");
    assert_eq!(finish(&mut server), b"late request");
}

#[test]
fn lingering_close_discards_peer_input_until_eof() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let client_h = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            write_all(&mut s, b"final response").await.expect("write");
            let mut scratch = [0; 512];
            linger_close(&mut s, &mut scratch, Some(h.now() + Duration::from_secs(5)))
                .await
                .expect("lingering close")
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let mut s = client_h
                .connect(address, Default::default())
                .await
                .expect("connect");
            let response = read_to_eof(&mut s).await;
            // Bytes sent after the server's final response stay unread by its
            // protocol; lingering close must drain them instead of resetting.
            for _ in 0..3 {
                write_all(&mut s, &[7; 1024]).await.expect("late bytes");
            }
            close(&mut s).await.expect("client close");
            response
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut client), b"final response");
    let lingered = finish(&mut server);
    assert_eq!(lingered.end, LingerEnd::Eof);
    assert_eq!(lingered.discarded, 3 * 1024);
    assert!(lingered.reads > 0);
}

#[test]
fn lingering_close_deadline_closes_silent_peer_without_spinning() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let client_h = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    const LINGER: Duration = Duration::from_millis(200);
    let mut server = executor
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            write_all(&mut s, b"final").await.expect("write");
            let at = h.now() + LINGER;
            let mut scratch = [0; 64];
            let lingered = linger_close(&mut s, &mut scratch, Some(at))
                .await
                .expect("lingering close");
            assert!(h.now() >= at, "closed before the linger deadline");
            lingered
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let mut s = client_h
                .connect(address, Default::default())
                .await
                .expect("connect");
            assert_eq!(read_to_eof(&mut s).await, b"final");
            // Never close: the server's deadline alone must end its linger.
            s
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    let _silent_peer = finish(&mut client);
    // The client saw the half-close, so the server is now waiting on its idle
    // read and the linger timer. Settle residual discovery from the exchange, as
    // the keep-alive no-spin tests do; the remaining wait must block until the
    // deadline: at most two turns and one zero-event wait for the one expiry.
    for _ in 0..3 {
        executor.turn(Timeout::Now).expect("settle queued events");
    }
    assert!(!server.is_finished(), "linger ended before its deadline");
    let (mut turns, mut empty) = (0, 0);
    while !server.is_finished() {
        assert!(turns < 2, "lingering close spun");
        let info = executor.turn(Timeout::Until(end)).expect("turn");
        turns += 1;
        empty += info.zero_event_waits;
    }
    assert!(empty <= 1, "zero-event waits while lingering: {empty}");
    let lingered = finish(&mut server);
    assert_eq!(lingered.end, LingerEnd::Deadline);
    assert_eq!((lingered.reads, lingered.discarded), (0, 0));
}

#[test]
fn lingering_close_deadline_bounds_a_stalled_half_close() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let client_h = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            // Fill the peer's receive window and our send buffer: the peer never
            // reads, so the last accepted write can never be flushed.
            let chunk = [9; 16384];
            let mut written = 0;
            loop {
                let write = std::future::poll_fn(|cx| Pin::new(&mut s).poll_write(cx, &chunk));
                match h.timeout(Duration::from_millis(200), write).await {
                    Ok(result) => written += result.expect("write"),
                    Err(e) => {
                        assert_eq!(e.kind, turnloop::ErrorKind::TimedOut);
                        break;
                    }
                }
                assert!(written < 1 << 30, "socket buffers never filled");
            }
            let at = h.now() + Duration::from_millis(50);
            let mut scratch = [0; 64];
            let error = linger_close(&mut s, &mut scratch, Some(at))
                .await
                .expect_err("a half-close that cannot flush must not wait forever");
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(h.now() >= at);
            // Dropping the unclosed stream releases its handle.
            drop(s);
            written
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            client_h
                .connect(address, Default::default())
                .await
                .expect("connect")
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(20);
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert!(finish(&mut server) > 0);
    drop(finish(&mut client));
}

#[test]
fn half_close_is_unsupported_on_datagram_adapters() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let mut task = executor
        .spawn_local(async move {
            let handle = h
                .driver()
                .udp_bind("127.0.0.1:0".parse().expect("address"), &Default::default())
                .expect("udp");
            let peer = h.driver().local_addr(handle).expect("address");
            let mut udp = h.udp(handle, peer);
            let error = shutdown(&mut udp)
                .await
                .expect_err("datagrams cannot half-close");
            assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
            let mut scratch = [0; 16];
            let lingered = linger_close(&mut udp, &mut scratch, None)
                .await
                .expect("closes without lingering");
            assert_eq!(lingered.end, LingerEnd::Unsupported);
            1
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 1);
}

#[cfg(any(
    not(target_arch = "wasm32"),
    all(target_os = "wasi", target_env = "p2")
))]
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

#[cfg(all(target_os = "wasi", target_env = "p3"))]
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

#[cfg(all(unix, not(target_arch = "wasm32")))]
#[test]
fn blocking_worker_and_dns_validation_deliver_results() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let main_thread = std::thread::current().id();
    let mut task = executor
        .spawn_local(async move {
            let result = h
                .blocking(move || {
                    assert_ne!(
                        std::thread::current().id(),
                        main_thread,
                        "must execute off loop"
                    );
                    Ok(turnloop::Payload::Boxed(Box::new(42u32)))
                })
                .await
                .expect("worker completion");
            match result {
                turnloop::Payload::Boxed(v) => {
                    assert_eq!(*v.downcast::<u32>().expect("payload"), 42)
                }
                _ => panic!("wrong completion"),
            }
            // The native resolver validates this on its worker without network I/O.
            let error = dns::query(
                &h,
                "invalid..name".into(),
                dns::Query::Srv,
                h.now() + Duration::from_secs(3),
            )
            .await
            .expect_err("invalid DNS label");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            2
        })
        .expect("spawn");
    let at = executor.handle().now() + Duration::from_secs(5);
    while !task.is_finished() {
        assert!(executor.handle().now() < at);
        executor.turn(Timeout::Until(at)).expect("turn");
    }
    assert_eq!(finish(&mut task), 2);
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
#[test]
#[ignore = "requires external DNS; deterministic wire/worker tests run by default"]
fn native_srv_txt_records_through_blocking_pool() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let mut task = executor
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(15);
            let records = dns::query(
                &h,
                "_xmpp-server._tcp.jabber.org".into(),
                dns::Query::Srv,
                at,
            )
            .await
            .expect("native SRV query");
            assert!(
                records
                    .iter()
                    .any(|r| matches!(r, dns::Record::Srv { target, port: 5269, .. }
            if target.ends_with(".jabber.org"))),
                "actual XMPP service record: {records:?}"
            );
            let records = dns::query(&h, "example.com".into(), dns::Query::Txt, at)
                .await
                .expect("native TXT query");
            assert!(
                records
                    .iter()
                    .any(|r| matches!(r, dns::Record::Txt { text, .. }
            if text.starts_with("v=spf1"))),
                "actual SPF record: {records:?}"
            );
            2
        })
        .expect("spawn");
    let at = executor.handle().now() + Duration::from_secs(20);
    while !task.is_finished() {
        assert!(executor.handle().now() < at);
        executor.turn(Timeout::Until(at)).expect("turn");
    }
    assert_eq!(finish(&mut task), 2);
}
