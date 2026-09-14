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
use turnloop_http::{
    asynchronous::{
        client::{Client, Options},
        server::{self, Server},
    },
    client::Request,
    http1::{BodyLength, Event, Head, Header},
    http2,
};
use turnloop_io::{
    turnloop::{Config, LocalExecutor, Timeout, backend::Platform},
    *,
};
fn finish<T>(task: &mut turnloop::executor::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task incomplete"),
    }
}
fn response() -> Head {
    Head {
        method: String::new(),
        target: String::new(),
        status: 200,
        version: 1,
        headers: vec![Header::new("content-type", "text/plain")],
        keep_alive: true,
    }
}
fn run_echo(h2: bool) {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let mut server =
        Server::bind(h.clone(), "127.0.0.1:0".parse().expect("address")).expect("server");
    let address = server.local_addr().expect("address");
    let shutdown = server.shutdown();
    let mut task = executor
        .spawn_local(async move {
            server
                .run(move |stream, signal| async move {
                    if h2 {
                        server::http2(stream, signal, |core, event| {
                            match event {
                                http2::Event::Headers {
                                    stream, end_stream, ..
                                } => {
                                    core.send_headers(
                                        stream,
                                        &[Header::new(":status", "200")],
                                        end_stream,
                                    )
                                    .map_err(std::io::Error::other)?;
                                }
                                http2::Event::Data {
                                    stream,
                                    bytes,
                                    end_stream,
                                } => {
                                    core.release_capacity(stream, bytes.len() as u32)
                                        .map_err(std::io::Error::other)?;
                                    assert_eq!(
                                        core.send_data(stream, bytes, end_stream)
                                            .map_err(std::io::Error::other)?,
                                        bytes.len()
                                    );
                                }
                                _ => {}
                            }
                            Ok(())
                        })
                        .await
                    } else {
                        server::http1(stream, signal, |event, out| {
                            match event {
                                Event::Head(_) => out.start(&response(), BodyLength::Chunked)?,
                                Event::Body(bytes) => out.body(bytes)?,
                                Event::End => out.finish(&[])?,
                                _ => {}
                            }
                            Ok(())
                        })
                        .await
                    }
                })
                .await
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let tls =
                turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000).expect("config");
            let mut client = Client::new(
                h,
                tls,
                1_789_344_000,
                Options {
                    http2_prior_knowledge: h2,
                    ..Default::default()
                },
            );
            let mut request =
                Request::new(&format!("http://{address}/echo"), "POST").expect("request");
            request.body = vec![b'x'; 32768];
            let mut total = 0;
            for _ in 0..4 {
                let head = client
                    .request(&mut request, |bytes| {
                        assert!(bytes.iter().all(|&b| b == b'x'));
                        total += bytes.len();
                        Ok(())
                    })
                    .await
                    .expect("request");
                assert_eq!(head.status, 200);
            }
            assert_eq!(total, 4 * 32768);
            assert!(client.next_deadline().is_some());
            shutdown.stop();
            total
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !client.is_finished() || !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut client), 4 * 32768);
    finish(&mut task).expect("server drain");
}
#[test]
fn http1_streaming_pool_and_shutdown() {
    run_echo(false);
}
#[test]
fn http2_streaming_pool_and_shutdown() {
    run_echo(true);
}

#[test]
fn idle_http_keepalive_obeys_no_spin() {
    use std::{cell::Cell, rc::Rc};
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let mut server =
        Server::bind(h.clone(), "127.0.0.1:0".parse().expect("address")).expect("server");
    let address = server.local_addr().expect("address");
    let signal = server.shutdown();
    let service = executor
        .spawn_local(async move {
            server
                .run(|stream, signal| {
                    server::http1(stream, signal, |event, out| {
                        match event {
                            Event::Head(_) => out.start(&response(), BodyLength::Empty)?,
                            Event::End => out.finish(&[])?,
                            _ => {}
                        }
                        Ok(())
                    })
                })
                .await
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let tls = turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000)
                .expect("TLS config");
            let mut client = Client::new(h, tls, 1_789_344_000, Options::default());
            let mut request = Request::new(&format!("http://{address}/"), "GET").expect("request");
            assert_eq!(
                client
                    .request(&mut request, |_| panic!("empty body"))
                    .await
                    .expect("response")
                    .status,
                200
            );
            client
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    let _client = finish(&mut client);
    for _ in 0..3 {
        executor.turn(Timeout::Now).expect("settle queued events");
    }
    let mut expiries = 0;
    for delay in [
        Duration::from_micros(500),
        Duration::from_millis(2),
        Duration::from_millis(10),
    ] {
        for _ in 0..20 {
            let h = executor.handle();
            let at = h.now() + delay;
            let done = Rc::new(Cell::new(false));
            let task_done = done.clone();
            let timer = executor
                .spawn_local(async move {
                    h.sleep_until(at).await.expect("timer");
                    assert!(h.now() >= at);
                    task_done.set(true);
                })
                .expect("spawn");
            executor.run_ready();
            let (mut turns, mut empty, mut waits) = (0, 0, 0);
            while !done.get() {
                assert!(turns < 2, "idle keepalive spun");
                let info = executor.turn(Timeout::Until(at)).expect("turn");
                turns += 1;
                empty += info.zero_event_waits;
                waits += info.os_waits;
            }
            assert!(empty <= 1);
            assert!(waits > 0);
            assert!(timer.is_finished());
            expiries += 1;
            executor.turn(Timeout::Now).expect("drain timer close");
        }
    }
    assert_eq!(expiries, 60);
    signal.stop();
    drop(service);
    executor.turn(Timeout::Now).expect("cancel service");
}

#[test]
fn drop_response_future_mid_body_closes_once() {
    use turnloop_http::asynchronous::Http1;
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let stream = listener.accept().await.expect("accept");
            let mut http = Http1::new(stream, turnloop_http::http1::Mode::Request);
            assert_eq!(http.head().await.expect("head").method, "GET");
            http.event(|event| {
                assert!(matches!(event, Event::End));
                Ok(())
            })
            .await
            .expect("request end");
            http.send_head(&response(), BodyLength::Known(100))
                .await
                .expect("head");
            http.send_body(b"partial").await.expect("partial body");
            let (mut stream, extra) = http.into_parts().expect("parts");
            assert!(extra.is_empty());
            let mut b = [0];
            assert_eq!(
                read(&mut stream, &mut b).await.expect("cancellation EOF"),
                0
            );
            assert_eq!(read(&mut stream, &mut b).await.expect("repeat EOF"), 0);
            1
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let stream = h
                .connect(address, Default::default())
                .await
                .expect("connect");
            let mut http = Http1::new(stream, turnloop_http::http1::Mode::Response);
            http.response_to("GET");
            let req = Request::new(&format!("http://{address}/"), "GET").expect("request");
            http.send_head(&req.head(false), BodyLength::Empty)
                .await
                .expect("head");
            http.finish_body(&[]).await.expect("finish");
            assert_eq!(http.head().await.expect("response").status, 200);
            let mut bytes = 0;
            http.event(|event| {
                if let Event::Body(b) = event {
                    assert_eq!(b, b"partial");
                    bytes += b.len();
                }
                Ok(())
            })
            .await
            .expect("body");
            assert_eq!(bytes, 7);
            {
                let mut future =
                    std::pin::pin!(http.event(|_| panic!("cancelled event delivered")));
                std::future::poll_fn(|cx| {
                    assert!(future.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            assert!(!http.reusable());
            assert!(http.event(|_| Ok(())).await.is_err());
            bytes
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !client.is_finished() || !server.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut client), 7);
    assert_eq!(finish(&mut server), 1);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "requires private Node HTTP fixtures"]
fn node_async_client_redirect_decompression_and_h2() {
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let port = std::env::var("TURNLOOP_TEST_HTTP_PORT").expect("HTTP fixture");
    let port2 = std::env::var("TURNLOOP_TEST_HTTP2_PORT").expect("HTTP2 fixture");
    let mut task = executor
        .spawn_local(async move {
            let tls =
                turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000).expect("config");
            let mut client = Client::new(h.clone(), tls.clone(), 1_789_344_000, Options::default());
            let mut req =
                Request::new(&format!("http://127.0.0.1:{port}/redirect"), "GET").expect("request");
            let mut result = Vec::new();
            let response = client
                .request(&mut req, |bytes| {
                    result.extend_from_slice(bytes);
                    Ok(())
                })
                .await
                .expect("redirect");
            assert_eq!(response.status, 200);
            assert_eq!(result, b"compressed from node");
            assert_eq!(req.redirects, 1);
            let mut client = Client::new(
                h,
                tls,
                1_789_344_000,
                Options {
                    http2_prior_knowledge: true,
                    ..Default::default()
                },
            );
            let mut count = 0;
            for _ in 0..100 {
                let mut request = Request::new(&format!("http://127.0.0.1:{port2}/echo"), "POST")
                    .expect("request");
                request.body = b"async node h2".to_vec();
                let mut received = Vec::new();
                let head = client
                    .request(&mut request, |bytes| {
                        received.extend_from_slice(bytes);
                        Ok(())
                    })
                    .await
                    .expect("h2 response");
                assert_eq!(head.status, 200);
                assert_eq!(received, b"POST /echo async node h2");
                count += 1;
            }
            count
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(15);
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 100);
}
