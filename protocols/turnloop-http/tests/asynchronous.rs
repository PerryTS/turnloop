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
