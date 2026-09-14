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
    idle_keepalive(false);
}
#[test]
fn idle_http2_keepalive_obeys_no_spin() {
    idle_keepalive(true);
}
fn idle_keepalive(h2: bool) {
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
                .run(move |stream, signal| async move {
                    if h2 {
                        server::http2(stream, signal, |core, event| {
                            if let http2::Event::Headers { stream, .. } = event {
                                core.send_headers(stream, &[Header::new(":status", "200")], true)
                                    .map_err(std::io::Error::other)?;
                            }
                            Ok(())
                        })
                        .await
                    } else {
                        server::http1(stream, signal, |event, out| {
                            match event {
                                Event::Head(_) => out.start(&response(), BodyLength::Empty)?,
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
            let tls = turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000)
                .expect("TLS config");
            let mut client = Client::new(
                h,
                tls,
                1_789_344_000,
                Options {
                    http2_prior_knowledge: h2,
                    ..Default::default()
                },
            );
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
            let mut request = Request::new(&format!("http://127.0.0.1:{port2}/echo"), "POST")
                .expect("large request");
            request.body = vec![b'x'; 262144];
            let mut received = Vec::new();
            assert_eq!(
                client
                    .request(&mut request, |bytes| {
                        received.extend_from_slice(bytes);
                        Ok(())
                    })
                    .await
                    .expect("flow-controlled upload and response")
                    .status,
                200
            );
            assert_eq!(received.len(), 262144 + b"POST /echo ".len());
            assert!(received.starts_with(b"POST /echo "));
            assert!(received[b"POST /echo ".len()..].iter().all(|&b| b == b'x'));
            count + 1
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(15);
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 101);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn node_https_and_http2_against_async_tls_server() {
    use std::process::Command;
    use turnloop_tls::{ServerConfig, TlsStream, rustls::pki_types::PrivatePkcs8KeyDer};
    for h2 in [false, true] {
        let cert =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("certificate");
        let ca = cert.cert.pem();
        let tls = ServerConfig::new(
            vec![cert.cert.der().clone()],
            PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            vec![if h2 {
                b"h2".to_vec()
            } else {
                b"http/1.1".to_vec()
            }],
            1_789_344_000,
        )
        .expect("TLS config");
        let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
        let h = executor.handle();
        let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
        let address = listener.local_addr().expect("address");
        let end = h.now() + Duration::from_secs(10);
        let mut task = executor
            .spawn_local(async move {
                let stream = listener.accept().await.expect("accept");
                let stream = TlsStream::accept(stream, &tls, &h, end, 1_789_344_000)
                    .await
                    .expect("TLS handshake");
                assert_eq!(
                    stream.alpn_protocol(),
                    Some(if h2 {
                        b"h2".as_slice()
                    } else {
                        b"http/1.1".as_slice()
                    })
                );
                let signal = server::Shutdown::default();
                let stop = signal.clone();
                let mut count = 0;
                if h2 {
                    server::http2(stream, signal, |core, event| {
                        if let http2::Event::Headers {
                            stream, end_stream, ..
                        } = event
                        {
                            assert!(end_stream);
                            core.send_headers(
                                stream,
                                &[
                                    Header::new(":status", "200"),
                                    Header::new("content-length", "4"),
                                ],
                                false,
                            )
                            .map_err(std::io::Error::other)?;
                            assert_eq!(
                                core.send_data(stream, b"node", true)
                                    .map_err(std::io::Error::other)?,
                                4
                            );
                            count += 1;
                            stop.stop();
                        }
                        Ok(())
                    })
                    .await
                    .expect("h2 drain");
                } else {
                    server::http1(stream, signal, |event, out| {
                        match event {
                            Event::Head(head) => {
                                assert_eq!(head.method, "GET");
                                out.start(&response(), BodyLength::Known(4))?;
                                out.body(b"node")?;
                            }
                            Event::End => {
                                out.finish(&[])?;
                                count += 1;
                                stop.stop();
                            }
                            _ => {}
                        }
                        Ok(())
                    })
                    .await
                    .expect("h1 drain");
                }
                count
            })
            .expect("spawn");
        let script = if h2 {
            format!(
                "const c=require('node:http2').connect('https://{address}',{{ca:process.env.TURNLOOP_CA,servername:'localhost'}});c.on('error',()=>process.exit(2));const r=c.request({{':path':'/'}});let body='';r.on('data',b=>body+=b);r.on('end',()=>{{if(body!=='node')process.exit(3);c.close()}});r.end();"
            )
        } else {
            format!(
                "require('node:https').get('https://{address}/',{{ca:process.env.TURNLOOP_CA,servername:'localhost',ALPNProtocols:['http/1.1']}},r=>{{let body='';r.on('data',b=>body+=b);r.on('end',()=>{{if(r.statusCode!==200||body!=='node')process.exit(3)}})}}).on('error',()=>process.exit(2));"
            )
        };
        let mut child = Command::new("node")
            .args(["-e", &script])
            .env("TURNLOOP_CA", ca)
            .spawn()
            .expect("Node required");
        while !task.is_finished() {
            assert!(executor.driver().now() < end);
            executor.turn(Timeout::Until(end)).expect("turn");
        }
        assert_eq!(finish(&mut task), 1);
        executor.turn(Timeout::Now).expect("close delivery");
        assert!(child.wait().expect("Node exit").success());
    }
}

#[test]
fn drop_server_cancels_inflight_request_once() {
    use std::{cell::Cell, rc::Rc};
    use turnloop_http::asynchronous::Http1;
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let mut server =
        Server::bind(h.clone(), "127.0.0.1:0".parse().expect("address")).expect("server");
    let address = server.local_addr().expect("address");
    let seen = Rc::new(Cell::new(0));
    let received = seen.clone();
    let mut service = executor
        .spawn_local(async move {
            server
                .run(move |stream, signal| {
                    let received = received.clone();
                    async move {
                        server::http1(stream, signal, |event, _| {
                            if let Event::Body(bytes) = event {
                                assert_eq!(bytes, b"partial");
                                received.set(received.get() + bytes.len());
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
            let stream = h
                .connect(address, Default::default())
                .await
                .expect("connect");
            let mut http = Http1::new(stream, turnloop_http::http1::Mode::Response);
            let request = Request::new(&format!("http://{address}/"), "POST").expect("request");
            http.send_head(&request.head(false), BodyLength::Known(100))
                .await
                .expect("head");
            http.send_body(b"partial").await.expect("partial body");
            assert!(http.head().await.is_err());
            1
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while seen.get() == 0 {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(seen.get(), 7);
    service.cancel();
    while !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut client), 1);
    assert!(matches!(
        Pin::new(&mut service).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(turnloop::JoinError::Cancelled))
    ));
}

#[test]
fn expect_continue_timeout_and_early_response() {
    use turnloop_http::{asynchronous::Http1, http1::Mode};
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let mut uploaded = 0;
            for case in 0..3 {
                let stream = listener.accept().await.expect("accept");
                let mut conn = Http1::new(stream, Mode::Request);
                let head = conn.head().await.expect("request head");
                assert!(head.token("expect", "100-continue"));
                if case == 0 {
                    for status in [103, 100] {
                        conn.send_head(
                            &Head {
                                status,
                                ..response()
                            },
                            BodyLength::Empty,
                        )
                        .await
                        .expect("informational");
                    }
                }
                if case < 2 {
                    loop {
                        let mut end = false;
                        assert!(
                            conn.event(|event| {
                                match event {
                                    Event::Body(bytes) => {
                                        assert!(bytes.iter().all(|&b| b == b'x'));
                                        uploaded += bytes.len();
                                    }
                                    Event::End => end = true,
                                    _ => {}
                                }
                                Ok(())
                            })
                            .await
                            .expect("upload")
                        );
                        if end {
                            break;
                        }
                    }
                    // A finished decoder must return immediately without another read.
                    for _ in 0..3 {
                        assert!(
                            !conn
                                .event(|_| panic!("duplicate completion"))
                                .await
                                .expect("terminal")
                        );
                    }
                }
                conn.send_head(
                    &Head {
                        status: if case == 2 { 417 } else { 200 },
                        keep_alive: false,
                        ..response()
                    },
                    BodyLength::Empty,
                )
                .await
                .expect("response");
                conn.finish_body(&[]).await.expect("finish");
                if case == 2 {
                    let mut stream = conn.into_inner().expect("no early upload");
                    assert_eq!(read(&mut stream, &mut [0; 1]).await.expect("peer EOF"), 0);
                }
            }
            uploaded
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let tls = turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000)
                .expect("TLS config");
            let mut client = Client::new(
                h,
                tls,
                1_789_344_000,
                Options {
                    continue_timeout: Duration::from_millis(2),
                    ..Default::default()
                },
            );
            let mut request = Request::new(&format!("http://{address}/"), "POST").expect("request");
            request.headers.push(Header::new("expect", "100-continue"));
            request.body = vec![b'x'; 16384];
            for case in 0..3 {
                let head = client
                    .request(&mut request, |_| panic!("empty response"))
                    .await
                    .expect("response");
                assert_eq!(head.status, if case == 2 { 417 } else { 200 });
            }
            3
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut client), 3);
    assert_eq!(finish(&mut server), 32768);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn curl_against_async_http1_server() {
    use std::process::{Command, Stdio};
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut task = executor
        .spawn_local(async move {
            let stream = listener.accept().await.expect("accept");
            let signal = server::Shutdown::default();
            let stop = signal.clone();
            let mut received = 0;
            server::http1(stream, signal, |event, out| {
                match event {
                    Event::Head(head) => {
                        assert_eq!(head.method, "POST");
                        out.start(&response(), BodyLength::Chunked)?;
                    }
                    Event::Body(bytes) => {
                        assert_eq!(bytes, b"curl async echo");
                        received += bytes.len();
                        out.body(bytes)?;
                    }
                    Event::End => {
                        out.finish(&[])?;
                        stop.stop();
                    }
                    _ => {}
                }
                Ok(())
            })
            .await
            .expect("server drain");
            received
        })
        .expect("spawn");
    let curl = std::env::var_os("TURNLOOP_TEST_CURL").unwrap_or_else(|| "curl".into());
    let mut child = Command::new(curl)
        .args([
            "--http1.1",
            "--silent",
            "--show-error",
            "--fail",
            "--noproxy",
            "*",
            "--max-time",
            "5",
            "--data-binary",
            "curl async echo",
            &format!("http://{address}/"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("curl required for HTTP/1.1");
    let end = executor.driver().now() + Duration::from_secs(10);
    while !task.is_finished() {
        if executor.driver().now() >= end {
            // Tell a hung server (curl exited, its close never delivered) from a hung curl.
            let exited = child.try_wait();
            let _ = child.kill();
            panic!(
                "server task pending after 10 s; curl before kill: {exited:?}; curl output: {:?}",
                child.wait_with_output()
            );
        }
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 15);
    executor.turn(Timeout::Now).expect("close delivery");
    let output = child.wait_with_output().expect("curl exit");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"curl async echo");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn node_https_via_authenticated_connect_proxy() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    use turnloop_http::client::ProxyEnvironment;
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("certificate");
    let script = r#"
const assert = require('node:assert/strict');
const https = require('node:https'), http = require('node:http'), net = require('node:net');
let connects = 0, requests = 0;
const watchdog = setTimeout(() => process.exit(7), 10000);
const origin = https.createServer({key: process.env.KEY, cert: process.env.CERT}, (req, res) => {
  assert.equal(req.method, 'POST'); assert.equal(req.url, '/through-proxy');
  let body = ''; req.on('data', b => body += b); req.on('end', () => {
    assert.equal(body, 'verified TLS upload'); requests++;
    res.setHeader('connection', 'close'); res.end('verified TLS response');
  });
});
const proxy = http.createServer();
proxy.on('connect', (req, client, head) => {
  assert.equal(req.url, 'localhost:' + origin.address().port);
  assert.equal(req.headers['proxy-authorization'], 'Basic dXNlcjpwYXNz'); connects++;
  const upstream = net.connect(origin.address().port, '127.0.0.1', () => {
    client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
    if (head.length) upstream.write(head);
    client.pipe(upstream); upstream.pipe(client);
  });
  client.on('error', () => upstream.destroy()); upstream.on('error', () => client.destroy());
  client.on('close', () => {
    upstream.destroy(); assert.equal(connects, 1); assert.equal(requests, 1);
    proxy.close(); origin.close(); clearTimeout(watchdog);
  });
});
origin.listen(0, '127.0.0.1', () => proxy.listen(0, '127.0.0.1', () => {
  console.log(origin.address().port + ' ' + proxy.address().port);
}));
"#;
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut node = OwnedChild(
        Command::new("node")
            .args(["-e", script])
            .env("KEY", cert.key_pair.serialize_pem())
            .env("CERT", cert.cert.pem())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Node required"),
    );
    let mut ports = String::new();
    BufReader::new(node.0.stdout.take().expect("stdout"))
        .read_line(&mut ports)
        .expect("listener ports");
    let ports: Vec<u16> = ports
        .split_whitespace()
        .map(|p| p.parse().expect("port"))
        .collect();
    assert_eq!(ports.len(), 2);
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let mut task = executor
        .spawn_local(async move {
            let tls = turnloop_tls::ClientConfig::new(
                turnloop_tls::ClientOptions {
                    ca: Some(vec![cert.cert.der().clone()]),
                    alpn: vec![b"http/1.1".to_vec()],
                    ..Default::default()
                },
                1_789_344_000,
            )
            .expect("trusted TLS config");
            let mut client = Client::new(
                h,
                tls,
                1_789_344_000,
                Options {
                    proxy: ProxyEnvironment {
                        https_proxy: Some(format!("http://user:pass@127.0.0.1:{}", ports[1])),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
            let mut req = Request::new(
                &format!("https://localhost:{}/through-proxy", ports[0]),
                "POST",
            )
            .expect("request");
            req.body = b"verified TLS upload".to_vec();
            let mut body = Vec::new();
            let head = client
                .request(&mut req, |bytes| {
                    body.extend_from_slice(bytes);
                    Ok(())
                })
                .await
                .expect("CONNECT and TLS request");
            assert_eq!(head.status, 200);
            assert_eq!(body, b"verified TLS response");
            body.len()
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(10);
    while !task.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut task), 21);
    executor.turn(Timeout::Now).expect("close delivery");
    assert!(node.0.wait().expect("Node exit").success());
}

#[test]
fn cancelled_pooled_request_never_reuses_partial_response() {
    use std::cell::Cell;
    use turnloop_http::{
        asynchronous::{Http1, Http2},
        http1::Mode,
    };
    for h2 in [false, true] {
        let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
        let h = executor.handle();
        let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
        let address = listener.local_addr().expect("address");
        let mut server = executor
            .spawn_local(async move {
                for request in 0..2 {
                    let stream = listener.accept().await.expect("fresh accept");
                    if h2 {
                        let mut conn = Http2::new(stream, http2::Role::Server).expect("h2");
                        let mut heads = 0;
                        while conn
                            .event(|core, event| {
                                if let http2::Event::Headers { stream, .. } = event {
                                    heads += 1;
                                    core.send_headers(
                                        stream,
                                        &[Header::new(":status", "200")],
                                        request == 1,
                                    )
                                    .map_err(std::io::Error::other)?;
                                    if request == 0 {
                                        assert_eq!(
                                            core.send_data(stream, b"partial", false)
                                                .map_err(std::io::Error::other)?,
                                            7
                                        );
                                    }
                                }
                                Ok(())
                            })
                            .await
                            .expect("receive and peer EOF")
                        {}
                        assert_eq!(heads, 1);
                    } else {
                        let mut conn = Http1::new(stream, Mode::Request);
                        assert_eq!(conn.head().await.expect("head").method, "GET");
                        conn.event(|e| {
                            assert!(matches!(e, Event::End));
                            Ok(())
                        })
                        .await
                        .expect("request end");
                        conn.send_head(
                            &response(),
                            BodyLength::Known(if request == 0 { 100 } else { 0 }),
                        )
                        .await
                        .expect("response head");
                        if request == 0 {
                            conn.send_body(b"partial").await.expect("partial response");
                        } else {
                            conn.finish_body(&[]).await.expect("complete response");
                        }
                        let mut stream = conn.into_inner().expect("transport");
                        assert_eq!(read(&mut stream, &mut [0; 1]).await.expect("peer EOF"), 0);
                    }
                }
                2
            })
            .expect("spawn");
        let mut client = executor
            .spawn_local(async move {
                let tls = turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000)
                    .expect("TLS config");
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
                    Request::new(&format!("http://{address}/"), "GET").expect("request");
                let bytes_seen = Cell::new(0);
                {
                    let mut pending = std::pin::pin!(client.request(&mut request, |bytes| {
                        assert_eq!(bytes, b"partial");
                        bytes_seen.set(bytes_seen.get() + bytes.len());
                        Ok(())
                    }));
                    std::future::poll_fn(|cx| {
                        assert!(
                            pending.as_mut().poll(cx).is_pending(),
                            "first response is deliberately unfinished"
                        );
                        if bytes_seen.get() == 7 {
                            Poll::Ready(())
                        } else {
                            Poll::Pending
                        }
                    })
                    .await;
                }
                assert_eq!(bytes_seen.get(), 7);
                assert!(
                    client.next_deadline().is_none(),
                    "cancelled lease must leave the pool"
                );
                assert_eq!(
                    client
                        .request(&mut request, |_| panic!("empty second body"))
                        .await
                        .expect("fresh connection")
                        .status,
                    200
                );
                1
            })
            .expect("spawn");
        let end = executor.driver().now() + Duration::from_secs(5);
        while !server.is_finished() || !client.is_finished() {
            assert!(executor.driver().now() < end);
            executor.turn(Timeout::Until(end)).expect("turn");
        }
        assert_eq!(finish(&mut client), 1);
        assert_eq!(finish(&mut server), 2);
    }
}

#[test]
fn graceful_goaway_drains_existing_pooled_request() {
    use turnloop_http::asynchronous::Http2;
    let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
    let h = executor.handle();
    let sh = h.clone();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = executor
        .spawn_local(async move {
            let mut conn = Http2::new(
                listener.accept().await.expect("accept"),
                http2::Role::Server,
            )
            .expect("h2");
            let mut stream = None;
            while stream.is_none() {
                assert!(
                    conn.event(|_, event| {
                        if let http2::Event::Headers { stream: id, .. } = event {
                            stream = Some(id);
                        }
                        Ok(())
                    })
                    .await
                    .expect("headers")
                );
            }
            let stream = stream.expect("request stream");
            conn.core
                .send_headers(stream, &[Header::new(":status", "200")], false)
                .expect("response headers");
            assert_eq!(
                conn.core
                    .send_data(stream, b"before", false)
                    .expect("partial"),
                6
            );
            conn.shutdown().await.expect("GOAWAY flush");
            sh.sleep(Duration::from_millis(2))
                .await
                .expect("in-flight response delay");
            assert_eq!(
                conn.core
                    .send_data(stream, b"after", true)
                    .expect("finish existing stream"),
                5
            );
            conn.flush().await.expect("response flush");
            while !conn.core.is_drained() {
                assert!(
                    conn.event(|_, _| Ok(()))
                        .await
                        .expect("drain request END_STREAM")
                );
            }
            11
        })
        .expect("spawn");
    let mut client = executor
        .spawn_local(async move {
            let tls = turnloop_tls::ClientConfig::new(Default::default(), 1_789_344_000)
                .expect("TLS config");
            let mut client = Client::new(
                h,
                tls,
                1_789_344_000,
                Options {
                    http2_prior_knowledge: true,
                    ..Default::default()
                },
            );
            let mut request = Request::new(&format!("http://{address}/"), "GET").expect("request");
            let mut bytes = Vec::new();
            assert_eq!(
                client
                    .request(&mut request, |chunk| {
                        bytes.extend_from_slice(chunk);
                        Ok(())
                    })
                    .await
                    .expect("response survives GOAWAY")
                    .status,
                200
            );
            assert_eq!(bytes, b"beforeafter");
            assert!(
                client.next_deadline().is_none(),
                "drained connection cannot be reused"
            );
            bytes.len()
        })
        .expect("spawn");
    let end = executor.driver().now() + Duration::from_secs(5);
    while !server.is_finished() || !client.is_finished() {
        assert!(executor.driver().now() < end);
        executor.turn(Timeout::Until(end)).expect("turn");
    }
    assert_eq!(finish(&mut server), 11);
    assert_eq!(finish(&mut client), 11);
}
