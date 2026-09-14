use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use turnloop_http::{
    client::{RedirectMode, Request},
    http1::{self, Head, Header},
    http2,
};
#[path = "../../turnloop-tls/tests/support/mod.rs"]
mod tls_support;
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn node_server(mode: &str) -> (Process, u16) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/private-http-server.mjs"
    );
    let mut child = Command::new("node")
        .args([path, mode])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line.trim().parse().unwrap();
    (Process(child), port)
}
fn socket(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}
fn accept(listener: TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                return s;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < end, "client did not connect");
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("{e}"),
        }
    }
}
fn response(stream: &mut impl Read, decoder: &mut http1::Decoder) -> (Head, Vec<u8>, Vec<Header>) {
    let mut buffer = Vec::new();
    let mut body = Vec::new();
    let mut head = None;
    let mut trailers = Vec::new();
    loop {
        let step = decoder.receive(&buffer).unwrap();
        let progressed = step.consumed > 0 || step.event.is_some();
        let end = matches!(step.event, Some(http1::Event::End));
        match step.event {
            Some(http1::Event::Head(h)) => head = Some(h),
            Some(http1::Event::Body(bytes)) => body.extend_from_slice(bytes),
            Some(http1::Event::Trailers(t)) => trailers = t,
            _ => {}
        }
        buffer.drain(..step.consumed);
        if end {
            return (head.unwrap(), body, trailers);
        }
        if !progressed {
            let mut bytes = [0; 37];
            let n = stream.read(&mut bytes).unwrap();
            if n == 0 {
                decoder.eof().unwrap();
            } else {
                buffer.extend_from_slice(&bytes[..n]);
            }
        }
    }
}
fn send(stream: &mut impl Write, request: &Request) {
    let mut bytes = Vec::new();
    let mut encoder = http1::Encoder::start(
        &request.head(false),
        http1::BodyLength::Known(request.body.len() as u64),
        &mut bytes,
    )
    .unwrap();
    encoder.body(&request.body, &mut bytes).unwrap();
    encoder.finish(&[], &mut bytes).unwrap();
    stream.write_all(&bytes).unwrap();
}
#[test]
fn node_h1_redirect_compression_trailers_and_socket_reuse() {
    let (_server, port) = node_server("h1");
    let mut socket = socket(port);
    let mut decoder = http1::Decoder::new(http1::Mode::Response, Default::default());
    let mut request = Request::new(&format!("http://127.0.0.1:{port}/redirect"), "GET").unwrap();
    send(&mut socket, &request);
    let (head, body, _) = response(&mut socket, &mut decoder);
    assert!(body.is_empty());
    assert!(
        request
            .redirect(
                head.status,
                head.get("location")
                    .map(|x| std::str::from_utf8(x).unwrap()),
                RedirectMode::Follow,
                20
            )
            .unwrap()
    );
    decoder.reset().unwrap();
    send(&mut socket, &request);
    let (head, body, _) = response(&mut socket, &mut decoder);
    assert_eq!(head.status, 200);
    let mut decoded = Vec::new();
    turnloop_http::compression::decode("gzip", &body, &mut decoded, 100).unwrap();
    assert_eq!(decoded, b"compressed from node");
    decoder.reset().unwrap();
    request.url.set_path("/trailers");
    send(&mut socket, &request);
    let (_, body, trailers) = response(&mut socket, &mut decoder);
    assert_eq!(body, b"chunk-onechunk-two");
    assert_eq!(trailers, [Header::new("x-check", "verified")]);
    for _ in 0..2 {
        decoder.reset().unwrap();
        request.url.set_path("/reuse");
        send(&mut socket, &request);
        let (head, body, _) = response(&mut socket, &mut decoder);
        assert_eq!(head.get("x-socket"), Some(b"1".as_slice()));
        assert_eq!(body, b"GET /reuse ");
    }
}
fn serve_h1(mut stream: impl Read + Write) {
    let mut decoder = http1::Decoder::new(http1::Mode::Request, Default::default());
    let (head, body, _) = response(&mut stream, &mut decoder);
    assert_eq!(head.target, "/interop");
    assert!(body.is_empty());
    let reply = Head {
        method: String::new(),
        target: String::new(),
        status: 200,
        version: 1,
        headers: vec![Header::new("connection", "close")],
        keep_alive: false,
    };
    let mut wire = Vec::new();
    let mut encoder = http1::Encoder::start(&reply, http1::BodyLength::Chunked, &mut wire).unwrap();
    encoder.body(b"native-http", &mut wire).unwrap();
    encoder
        .finish(&[Header::new("x-check", "native")], &mut wire)
        .unwrap();
    stream.write_all(&wire).unwrap();
}
#[test]
fn curl_and_node_fetch_against_native_http1() {
    for node in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || serve_h1(accept(listener)));
        let url = format!("http://{address}/interop");
        let output = if node {
            Command::new("node").args(["--input-type=module","-e","const r=await fetch(process.argv[1]);if(r.status!==200)process.exit(2);console.log(await r.text());",&url]).output().unwrap()
        } else {
            Command::new("curl")
                .args([
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "10",
                    "--noproxy",
                    "*",
                    &url,
                ])
                .output()
                .unwrap()
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "native-http"
        );
        server.join().unwrap();
    }
}
#[test]
fn https_over_unbuffered_tls() {
    let cert = tls_support::certificate();
    let config = tls_support::server_config(&cert);
    let client = turnloop_tls::ClientConfig::new(
        turnloop_tls::ClientOptions {
            ca: Some(vec![cert.cert.der().clone()]),
            alpn: vec![b"http/1.1".to_vec()],
            ..Default::default()
        },
        tls_support::NOW,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        serve_h1(tls_support::Stream::new(
            config.accept().unwrap(),
            accept(listener),
        ))
    });
    let mut stream = tls_support::Stream::new(
        client
            .connect(turnloop_tls::rustls::pki_types::ServerName::try_from("localhost").unwrap())
            .unwrap(),
        socket(port),
    );
    send(
        &mut stream,
        &Request::new("https://localhost/interop", "GET").unwrap(),
    );
    let (head, body, trailers) = response(
        &mut stream,
        &mut http1::Decoder::new(http1::Mode::Response, Default::default()),
    );
    assert_eq!(head.status, 200);
    assert_eq!(body, b"native-http");
    assert_eq!(trailers, [Header::new("x-check", "native")]);
    server.join().unwrap();
}
fn flush_h2(engine: &mut http2::Connection, socket: &mut TcpStream) {
    socket.write_all(engine.output()).unwrap();
    let n = engine.output().len();
    engine.consume_output(n).unwrap();
}
fn serve_h2(mut socket: TcpStream, total: usize) {
    let mut engine = http2::Connection::new(http2::Role::Server, Default::default()).unwrap();
    let mut input = Vec::new();
    let mut count = 0;
    flush_h2(&mut engine, &mut socket);
    while count < total {
        let step = engine.receive(&input).unwrap();
        let consumed = step.consumed;
        if let Some(http2::Event::Headers {
            stream,
            headers,
            end_stream,
        }) = step.event
        {
            assert!(end_stream);
            assert!(
                headers
                    .iter()
                    .any(|h| h.name == ":method" && h.value == b"GET")
            );
            engine
                .send_headers(
                    stream,
                    &[
                        Header::new(":status", "200"),
                        Header::new("content-length", "9"),
                    ],
                    false,
                )
                .unwrap();
            assert_eq!(engine.send_data(stream, b"native-h2", true).unwrap(), 9);
            count += 1;
        }
        input.drain(..consumed);
        flush_h2(&mut engine, &mut socket);
        if consumed == 0 {
            let mut bytes = [0; 1024];
            let n = socket.read(&mut bytes).unwrap();
            assert!(n > 0, "EOF after {count}/{total}");
            input.extend_from_slice(&bytes[..n]);
        }
    }
    assert_eq!(count, total);
    engine.shutdown().unwrap();
    flush_h2(&mut engine, &mut socket);
    socket.shutdown(std::net::Shutdown::Write).unwrap();
    let mut tail = [0; 1024];
    loop {
        match socket.read(&mut tail) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("peer did not finish shutdown: {e}"),
        }
    }
}
#[test]
fn curl_and_node_h2_hundred_streams_against_native_server() {
    for node in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || serve_h2(accept(listener), if node { 100 } else { 1 }));
        let url = format!("http://{address}/interop");
        let output = if node {
            Command::new("node").args(["--input-type=module","-e",r#"import h2 from 'node:http2'; setTimeout(()=>{console.error('client timeout');process.exit(70);},10000).unref(); const c=h2.connect(process.argv[1]); await Promise.all(Array.from({length:100},()=>new Promise((resolve,reject)=>{const s=c.request({':path':'/interop'});let b='';s.on('response',h=>{if(h[':status']!==200)reject(Error('status'));});s.on('data',x=>b+=x);s.on('end',()=>b==='native-h2'?resolve():reject(Error(b)));s.on('error',reject);s.on('aborted',()=>reject(Error('stream aborted')));s.end();})));c.close();console.log('100 verified');"#,&url]).output().unwrap()
        } else {
            Command::new("curl")
                .args([
                    "--http2-prior-knowledge",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "10",
                    "--noproxy",
                    "*",
                    &url,
                ])
                .output()
                .unwrap()
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            if node { "100 verified" } else { "native-h2" }
        );
        server.join().unwrap();
    }
}
#[test]
fn native_h2_client_against_node_hundred_streams() {
    let (_server, port) = node_server("h2");
    let mut socket = socket(port);
    let mut engine = http2::Connection::new(http2::Role::Client, Default::default()).unwrap();
    for _ in 0..100 {
        engine
            .open(
                &[
                    Header::new(":method", "GET"),
                    Header::new(":scheme", "http"),
                    Header::new(":path", "/interop"),
                    Header::new(":authority", "localhost"),
                ],
                true,
            )
            .unwrap();
    }
    flush_h2(&mut engine, &mut socket);
    let mut input = Vec::new();
    let mut ended = 0;
    let mut bodies = vec![Vec::new(); 100];
    while ended < 100 {
        let step = engine.receive(&input).unwrap();
        let consumed = step.consumed;
        match step.event {
            Some(http2::Event::Headers { headers, .. }) => assert!(
                headers
                    .iter()
                    .any(|h| h.name == ":status" && h.value == b"200")
            ),
            Some(http2::Event::Data {
                stream,
                bytes,
                end_stream,
            }) => {
                bodies[(stream / 2) as usize].extend_from_slice(bytes);
                engine.release_capacity(stream, bytes.len() as u32).unwrap();
                if end_stream {
                    ended += 1;
                }
            }
            _ => {}
        }
        input.drain(..consumed);
        flush_h2(&mut engine, &mut socket);
        if consumed == 0 {
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).unwrap();
            assert!(n > 0);
            input.extend_from_slice(&bytes[..n]);
        }
    }
    assert_eq!(ended, 100);
    for b in bodies {
        assert_eq!(b, b"GET /interop ");
    }
}

#[test]
fn proxy_connect_then_tls_then_http() {
    use turnloop_http::client::{Route, TransportRequest};
    let cert = tls_support::certificate();
    let server_config = tls_support::server_config(&cert);
    let client = turnloop_tls::ClientConfig::new(
        turnloop_tls::ClientOptions {
            ca: Some(vec![cert.cert.der().clone()]),
            alpn: vec![b"http/1.1".to_vec()],
            ..Default::default()
        },
        tls_support::NOW,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut socket = accept(listener);
        let mut decoder = http1::Decoder::new(http1::Mode::Request, Default::default());
        let (head, body, _) = response(&mut socket, &mut decoder);
        assert_eq!(head.method, "CONNECT");
        assert_eq!(head.target, "localhost:443");
        assert_eq!(
            head.get("proxy-authorization"),
            Some(b"Basic dXNlcjpwYXNz".as_slice())
        );
        assert!(body.is_empty());
        socket
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .unwrap();
        serve_h1(tls_support::Stream::new(
            server_config.accept().unwrap(),
            socket,
        ));
    });
    let request = Request::new("https://localhost/interop", "GET").unwrap();
    let mut route = Route::new(
        request.url.clone(),
        Some(url::Url::parse(&format!("http://127.0.0.1:{port}")).unwrap()),
    );
    let mut socket = socket(port);
    let mut wire = Vec::new();
    let mut encoder = http1::Encoder::start(
        &route.connect_head(Some(b"Basic dXNlcjpwYXNz")).unwrap(),
        http1::BodyLength::Empty,
        &mut wire,
    )
    .unwrap();
    encoder.finish(&[], &mut wire).unwrap();
    socket.write_all(&wire).unwrap();
    let mut decoder = http1::Decoder::new(http1::Mode::Response, Default::default());
    decoder.response_to("CONNECT");
    let mut input = Vec::new();
    let status = loop {
        let step = decoder.receive(&input).unwrap();
        if let Some(http1::Event::Head(head)) = step.event {
            assert_eq!(step.consumed, input.len());
            break head.status;
        }
        let mut bytes = [0; 64];
        let n = socket.read(&mut bytes).unwrap();
        assert!(n > 0);
        input.extend_from_slice(&bytes[..n]);
    };
    assert!(matches!(
        route.tunnel_response(status).unwrap(),
        TransportRequest::UpgradeTls { .. }
    ));
    assert!(matches!(
        decoder.receive(&[]).unwrap().event,
        Some(http1::Event::Upgrade)
    ));
    let mut stream = tls_support::Stream::new(
        client
            .connect(turnloop_tls::rustls::pki_types::ServerName::try_from("localhost").unwrap())
            .unwrap(),
        socket,
    );
    send(&mut stream, &request);
    let (_, body, _) = response(
        &mut stream,
        &mut http1::Decoder::new(http1::Mode::Response, Default::default()),
    );
    assert_eq!(body, b"native-http");
    server.join().unwrap();
}
#[test]
fn abort_mid_body_closes_socket_and_completes_once() {
    use turnloop_http::client::{Completion, Http1Connection};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut socket = accept(listener);
        let mut decoder = http1::Decoder::new(http1::Mode::Request, Default::default());
        let (head, _, _) = response(&mut socket, &mut decoder);
        assert_eq!(head.target, "/abort");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
            .unwrap();
        let mut b = [0; 1];
        assert_eq!(
            socket.read(&mut b).unwrap(),
            0,
            "host must close aborted HTTP/1 transport"
        );
    });
    let mut socket = socket(port);
    let mut engine = Http1Connection::new(Default::default());
    engine
        .start(
            &Request::new("http://localhost/abort", "GET")
                .unwrap()
                .head(false),
            http1::BodyLength::Empty,
            None,
            None,
        )
        .unwrap();
    engine.finish_body(&[]).unwrap();
    socket.write_all(engine.output()).unwrap();
    engine.consume_output(engine.output().len()).unwrap();
    let mut input = Vec::new();
    let mut observed = false;
    while !observed {
        let step = engine.receive(&input).unwrap();
        let n = step.consumed;
        if let Some(http1::Event::Body(bytes)) = step.event {
            assert_eq!(bytes, b"partial");
            observed = true;
        }
        input.drain(..n);
        if n == 0 {
            let mut bytes = [0; 1024];
            let n = socket.read(&mut bytes).unwrap();
            assert!(n > 0);
            input.extend_from_slice(&bytes[..n]);
        }
    }
    engine.abort();
    assert!(
        matches!(engine.poll_completion(),Some(Completion::Error(e))if e.code=="UND_ERR_ABORTED")
    );
    assert_eq!(engine.poll_completion(), None);
    assert!(!engine.reusable());
    drop(socket);
    server.join().unwrap();
}
