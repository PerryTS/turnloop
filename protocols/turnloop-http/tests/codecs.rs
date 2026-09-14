use std::time::{Duration, Instant};
use turnloop_http::{client::*, hpack, http1::*, http2};
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|x| u8::from_str_radix(std::str::from_utf8(x).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn rfc_hpack_requests_and_huffman() {
    for cases in [
        [
            "828684410f7777772e6578616d706c652e636f6d",
            "828684be58086e6f2d6361636865",
            "828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565",
        ],
        [
            "828684418cf1e3c2e5f23a6ba0ab90f4ff",
            "828684be5886a8eb10649cbf",
            "828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf",
        ],
    ] {
        let mut decoder = hpack::Decoder::new(4096, 32768);
        let mut out = Vec::new();
        for (i, case) in cases.iter().enumerate() {
            decoder.decode(&hex(case), &mut out).unwrap();
            assert_eq!(out[0], Header::new(":method", "GET"));
            assert_eq!(out[3], Header::new(":authority", "www.example.com"));
            if i == 1 {
                assert_eq!(out[4], Header::new("cache-control", "no-cache"));
            }
            if i == 2 {
                assert_eq!(out[4], Header::new("custom-key", "custom-value"));
            }
        }
    }
    let mut encoded = Vec::new();
    hpack::huffman_encode(b"www.example.com", &mut encoded);
    assert_eq!(encoded, hex("f1e3c2e5f23a6ba0ab90f4ff"));
    for invalid in ["ff", "ffffffff", "00", "fffffffc"] {
        assert!(
            hpack::huffman_decode(&hex(invalid), &mut Vec::new(), 1024).is_err(),
            "{invalid}"
        );
    }
    let all: Vec<u8> = (0..=255).collect();
    encoded.clear();
    hpack::huffman_encode(&all, &mut encoded);
    let mut decoded = Vec::new();
    hpack::huffman_decode(&encoded, &mut decoded, 256).unwrap();
    assert_eq!(decoded, all);
    let mut ints = Vec::new();
    hpack::encode_integer(1337, 5, 0, &mut ints);
    assert_eq!(ints, hex("1f9a0a"));
}
#[test]
fn hpack_roundtrip_evictions_and_limits() {
    let mut encoder = hpack::Encoder::new(128);
    let mut decoder = hpack::Decoder::new(128, 1024);
    let mut wire = Vec::new();
    let mut out = Vec::new();
    for i in 0..100 {
        let fields = vec![
            Header::new(":method", "GET"),
            Header::new("custom", i.to_string()),
            Header::new("authorization", "secret"),
        ];
        wire.clear();
        encoder.encode(&fields, &mut wire);
        decoder.decode(&wire, &mut out).unwrap();
        assert_eq!(out, fields);
    }
    assert!(
        hpack::Decoder::new(4096, 10)
            .decode(&[0x82], &mut out)
            .is_err()
    );
    hpack::Decoder::new(4096, 32768)
        .decode(&hex("3fe11f"), &mut out)
        .unwrap();
    for invalid in [
        "80",
        "ff00",
        "3fe21f",
        "8220",
        "4081ff00",
        "ffffffffffffffffffffff7f",
    ] {
        assert!(
            hpack::Decoder::new(4096, 32768)
                .decode(&hex(invalid), &mut out)
                .is_err(),
            "{invalid}"
        );
    }
}
fn collect_fragmented(wire: &[u8], method: &str) -> (Vec<u8>, Vec<Header>, bool) {
    let mut decoder = Decoder::new(Mode::Response, Limits::default());
    decoder.response_to(method);
    let mut buffered = Vec::new();
    let mut body = Vec::new();
    let mut trailers = Vec::new();
    let mut ended = false;
    for b in wire {
        buffered.push(*b);
        loop {
            let step = decoder.receive(&buffered).unwrap();
            let progressed = step.consumed != 0 || step.event.is_some();
            match step.event {
                Some(Event::Body(b)) => body.extend_from_slice(b),
                Some(Event::Trailers(t)) => trailers = t,
                Some(Event::End) => ended = true,
                _ => {}
            }
            buffered.drain(..step.consumed);
            if !progressed {
                break;
            }
        }
    }
    assert!(ended);
    (body, trailers, decoder.reusable())
}
#[test]
fn chunked_trailers_fragmentation_and_lengths() {
    let(body,trailers,reuse)=collect_fragmented(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;x=y\r\nabc\r\n2\r\nde\r\n0\r\nx-digest: yes\r\n\r\n","GET");
    assert_eq!(body, b"abcde");
    assert_eq!(trailers, [Header::new("x-digest", "yes")]);
    assert!(reuse);
    let (body, _, reuse) = collect_fragmented(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc",
        "GET",
    );
    assert_eq!(body, b"abc");
    assert!(!reuse);
    let (body, _, _) =
        collect_fragmented(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n", "HEAD");
    assert!(body.is_empty());
}
#[test]
fn smuggling_rejected_and_failure_sticky() {
    let heads = [
        "Content-Length: 3\r\nTransfer-Encoding: chunked",
        "Content-Length: 3\r\nContent-Length: 3",
        "Content-Length: 3, 3",
        "Content-Length: +3",
        "Content-Length: 18446744073709551616",
        "Transfer-Encoding: chunked, chunked",
        "Transfer-Encoding: gzip, chunked",
        "Transfer-Encoding : chunked",
        "X: one\r\n folded",
        "X: a\nY: b",
    ];
    for headers in heads {
        let wire = format!("POST / HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n\r\n");
        let mut decoder = Decoder::new(Mode::Request, Limits::default());
        assert!(decoder.receive(wire.as_bytes()).is_err(), "{headers}");
        assert!(
            decoder
                .receive(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .is_err()
        );
    }
    for wire in [
        b"GET / HTTP/1.1\r\n\r\n".as_slice(),
        b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
    ] {
        assert!(
            Decoder::new(Mode::Request, Limits::default())
                .receive(wire)
                .is_err()
        );
    }
}
#[test]
fn informational_upgrade_and_eof() {
    let mut decoder = Decoder::new(Mode::Response, Limits::default());
    assert!(matches!(
        decoder
            .receive(b"HTTP/1.1 100 Continue\r\n\r\n")
            .unwrap()
            .event,
        Some(Event::Informational(_))
    ));
    decoder.response_to("CONNECT");
    let wire = b"HTTP/1.1 200 Connected\r\n\r\nTLS";
    let n = decoder.receive(wire).unwrap().consumed;
    assert!(matches!(
        decoder.receive(&wire[n..]).unwrap().event,
        Some(Event::Upgrade)
    ));
    assert_eq!(&wire[n..], b"TLS");
    let mut decoder = Decoder::new(Mode::Response, Limits::default());
    decoder
        .receive(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n")
        .unwrap();
    decoder.receive(b"x").unwrap();
    assert!(decoder.eof().is_err());
    let mut decoder = Decoder::new(Mode::Response, Limits::default());
    decoder.receive(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
    assert!(matches!(
        decoder.receive(b"abc").unwrap().event,
        Some(Event::Body(b"abc"))
    ));
    decoder.eof().unwrap();
    assert!(matches!(
        decoder.receive(&[]).unwrap().event,
        Some(Event::End)
    ));
    assert!(!decoder.reusable());
}
#[test]
fn serialization_validates_before_writing() {
    let request = Request::new("http://localhost/hello", "POST").unwrap();
    let head = request.head(false);
    let mut wire = Vec::new();
    let mut encoder = Encoder::start(&head, BodyLength::Chunked, &mut wire).unwrap();
    encoder.body(b"abc", &mut wire).unwrap();
    encoder
        .finish(&[Header::new("digest", "yes")], &mut wire)
        .unwrap();
    assert!(wire.ends_with(b"3\r\nabc\r\n0\r\ndigest: yes\r\n\r\n"));
    let mut head = head;
    head.headers.push(Header::new("x", "bad\r\nInjected: yes"));
    let before = wire.clone();
    assert!(Encoder::start(&head, BodyLength::Empty, &mut wire).is_err());
    assert_eq!(before, wire);
}
fn request_headers() -> Vec<Header> {
    vec![
        Header::new(":method", "GET"),
        Header::new(":scheme", "http"),
        Header::new(":path", "/"),
        Header::new(":authority", "localhost"),
    ]
}
fn transfer(from: &mut http2::Connection, to: &mut http2::Connection) -> usize {
    let wire = from.output().to_vec();
    from.consume_output(wire.len()).unwrap();
    let mut pos = 0;
    let mut events = 0;
    while pos < wire.len() {
        let step = to.receive(&wire[pos..]).unwrap();
        assert!(step.consumed > 0);
        pos += step.consumed;
        if step.event.is_some() {
            events += 1;
        }
    }
    events
}
#[test]
fn h2_hundred_streams_flow_control_abort_and_goaway() {
    let mut client = http2::Connection::new(http2::Role::Client, http2::Limits::default()).unwrap();
    let mut server = http2::Connection::new(http2::Role::Server, http2::Limits::default()).unwrap();
    transfer(&mut client, &mut server);
    transfer(&mut server, &mut client);
    transfer(&mut client, &mut server);
    let ids: Vec<_> = (0..100)
        .map(|_| client.open(&request_headers(), true).unwrap())
        .collect();
    assert_eq!(transfer(&mut client, &mut server), 100);
    assert!(client.open(&request_headers(), true).is_err());
    for id in &ids {
        server
            .send_headers(*id, &[Header::new(":status", "200")], false)
            .unwrap();
    }
    assert_eq!(transfer(&mut server, &mut client), 100);
    let data = vec![7; 100_000];
    let id = ids[0];
    let mut sent = 0;
    loop {
        let n = server.send_data(id, &data[sent..], false).unwrap();
        if n == 0 {
            break;
        }
        sent += n;
    }
    assert_eq!(sent, 65535);
    assert_eq!(server.send_data(ids[1], b"blocked", true).unwrap(), 0);
    transfer(&mut server, &mut client);
    client.release_capacity(id, 65535).unwrap();
    transfer(&mut client, &mut server);
    assert_eq!(server.send_data(ids[1], b"ready", true).unwrap(), 5);
    transfer(&mut server, &mut client);
    client.reset(id, 8).unwrap();
    assert_eq!(transfer(&mut client, &mut server), 1);
    server.shutdown().unwrap();
    assert_eq!(transfer(&mut server, &mut client), 1);
    assert!(client.open(&request_headers(), true).is_err());
}
#[test]
fn h2_continuation_flood_and_invalid_frames() {
    let mut server = http2::Connection::new(
        http2::Role::Server,
        http2::Limits {
            continuations: 1,
            ..Default::default()
        },
    )
    .unwrap();
    server.receive(http2::PREFACE).unwrap();
    let mut wire = Vec::new();
    http2::encode_frame(4, 0, 0, &[], &mut wire).unwrap();
    server.receive(&wire).unwrap();
    wire.clear();
    http2::encode_frame(1, 0, 1, &[], &mut wire).unwrap();
    server.receive(&wire).unwrap();
    wire.clear();
    http2::encode_frame(9, 0, 1, &[], &mut wire).unwrap();
    server.receive(&wire).unwrap();
    assert_eq!(
        server.receive(&wire).err().unwrap().code,
        "ENHANCE_YOUR_CALM"
    );
    for (kind, stream, payload) in [(6, 0, vec![0; 7]), (4, 1, vec![]), (8, 0, vec![0; 4])] {
        let mut s = http2::Connection::new(http2::Role::Server, Default::default()).unwrap();
        s.receive(http2::PREFACE).unwrap();
        let mut wire = Vec::new();
        http2::encode_frame(4, 0, 0, &[], &mut wire).unwrap();
        s.receive(&wire).unwrap();
        wire.clear();
        http2::encode_frame(kind, 0, stream, &payload, &mut wire).unwrap();
        assert!(s.receive(&wire).is_err());
    }
}
#[test]
fn redirect_pool_proxy_deadline_policy() {
    let mut request = Request::new("https://a.example/start", "POST").unwrap();
    request.body = b"data".to_vec();
    request.headers = vec![
        Header::new("authorization", "secret"),
        Header::new("content-type", "text/plain"),
    ];
    assert!(
        request
            .redirect(
                302,
                Some("https://b.example/next"),
                RedirectMode::Follow,
                20
            )
            .unwrap()
    );
    assert_eq!(request.method, "GET");
    assert!(request.body.is_empty());
    assert!(request.headers.is_empty());
    assert!(
        !request
            .redirect(307, Some("/x"), RedirectMode::Manual, 20)
            .unwrap()
    );
    let now = Instant::now();
    let key = PoolKey::new(&request.url, None);
    let mut pool = Pool::new(1, Duration::from_secs(2));
    let Acquire::Connect(id) = pool.acquire(&key, now) else {
        panic!()
    };
    assert_eq!(pool.acquire(&key, now), Acquire::Wait);
    pool.connected(id, Protocol::Http2, 100).unwrap();
    for _ in 1..100 {
        assert_eq!(pool.acquire(&key, now), Acquire::Reuse(id));
    }
    assert_eq!(pool.acquire(&key, now), Acquire::Wait);
    for _ in 0..100 {
        pool.release(id, true, now).unwrap();
    }
    assert_eq!(pool.next_timeout(), Some(now + Duration::from_secs(2)));
    assert_eq!(pool.handle_timeout(now + Duration::from_secs(2)), Some(id));
    assert_eq!(pool.handle_timeout(now + Duration::from_secs(2)), None);
    let env = ProxyEnvironment {
        http_proxy: Some("http://localhost:8080".into()),
        https_proxy: Some("http://localhost:8081".into()),
        no_proxy: ".example, localhost:8000".into(),
    };
    assert!(env.proxy_for(&request.url).unwrap().is_none());
    let target = Request::new("https://elsewhere.test/", "GET").unwrap().url;
    let mut route = Route::new(target.clone(), env.proxy_for(&target).unwrap());
    assert_eq!(
        route.connect_head(None).unwrap().target,
        "elsewhere.test:443"
    );
    assert!(matches!(
        route.tunnel_response(200).unwrap(),
        TransportRequest::UpgradeTls { .. }
    ));
    let mut op = Lifecycle::default();
    op.transition(Phase::Body, Some(now));
    op.abort();
    op.handle_timeout(now);
    assert_eq!(
        op.poll(),
        Some(Completion::Error(turnloop_http::Error::new(
            "UND_ERR_ABORTED",
            "Request aborted"
        )))
    );
    assert_eq!(op.poll(), None);
}
#[test]
fn gzip_deflate_br_zstd_decoding() {
    use std::io::Write;
    let body = b"hello compression hello compression";
    let mut encoded = flate2::write::GzEncoder::new(Vec::new(), Default::default());
    encoded.write_all(body).unwrap();
    let gzip = encoded.finish().unwrap();
    let mut out = Vec::new();
    turnloop_http::compression::decode("gzip", &gzip, &mut out, 100).unwrap();
    assert_eq!(out, body);
    assert!(turnloop_http::compression::decode("gzip", &gzip, &mut out, 2).is_err());
    let mut encoded = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
    encoded.write_all(body).unwrap();
    turnloop_http::compression::decode("deflate", &encoded.finish().unwrap(), &mut out, 100)
        .unwrap();
    assert_eq!(out, body);
    let mut encoded = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut encoded, 4096, 4, 22);
        encoder.write_all(body).unwrap();
    }
    turnloop_http::compression::decode("br", &encoded, &mut out, 100).unwrap();
    assert_eq!(out, body);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let encoded = zstd::stream::encode_all(body.as_slice(), 1).unwrap();
        turnloop_http::compression::decode("zstd", &encoded, &mut out, 100).unwrap();
        assert_eq!(out, body);
    }
}

#[test]
fn vendored_hpack_interop_corpus() {
    use std::io::Read;
    let mut corpus = String::new();
    flate2::read::GzDecoder::new(include_bytes!("hpack-corpus/stories.txt.gz").as_slice())
        .read_to_string(&mut corpus)
        .unwrap();
    let mut decoder = hpack::Decoder::new(4096, 1 << 20);
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    let mut story = "";
    let mut count = 0;
    for line in corpus.lines() {
        if let Some(name) = line.strip_prefix("S ") {
            story = name;
            decoder = hpack::Decoder::new(4096, 1 << 20);
        } else if let Some(wire) = line.strip_prefix("C ") {
            decoder
                .decode(&hex(wire), &mut actual)
                .unwrap_or_else(|e| panic!("{story} case {count}: {e}"));
            expected.clear();
        } else if let Some(header) = line.strip_prefix("H ") {
            let (n, v) = header.split_once(' ').unwrap();
            expected.push(Header {
                name: String::from_utf8(hex(n)).unwrap(),
                value: hex(v),
            });
        } else if line == "E" {
            assert_eq!(actual, expected, "{story}, case {count}");
            count += 1;
        } else {
            panic!("bad fixture")
        }
    }
    assert_eq!(count, 3754, "corpus cases must all run");
}

#[test]
fn expect_continue_streaming_abort_and_reuse() {
    let now = Instant::now();
    let mut connection = Http1Connection::new(Default::default());
    let mut head = Request::new("http://localhost/upload", "POST")
        .unwrap()
        .head(false);
    head.headers.push(Header::new("expect", "100-continue"));
    connection
        .start(
            &head,
            BodyLength::Known(4),
            Some(now + Duration::from_secs(5)),
            Some(now + Duration::from_secs(1)),
        )
        .unwrap();
    assert!(!connection.can_send_body());
    assert!(connection.send_body(b"body").is_err());
    let n = connection.output().len();
    connection.consume_output(n).unwrap();
    assert!(
        connection
            .start(&head, BodyLength::Empty, None, None)
            .is_err()
    );
    connection
        .receive(b"HTTP/1.1 100 Continue\r\n\r\n")
        .unwrap();
    assert!(connection.can_send_body());
    connection.send_body(b"bo").unwrap();
    connection.send_body(b"dy").unwrap();
    connection.finish_body(&[]).unwrap();
    assert_eq!(connection.output(), b"body");
    connection.consume_output(4).unwrap();
    connection
        .receive(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n")
        .unwrap();
    assert_eq!(connection.receive(b"OK").unwrap().consumed, 2);
    connection.receive(&[]).unwrap();
    assert_eq!(connection.poll_completion(), Some(Completion::Success));
    assert_eq!(connection.poll_completion(), None);
    assert!(connection.reusable());
    connection
        .start(&head, BodyLength::Known(4), None, Some(now))
        .unwrap();
    connection.handle_timeout(now);
    assert!(connection.can_send_body());
    connection.abort();
    assert_eq!(
        connection.poll_completion().unwrap(),
        Completion::Error(turnloop_http::Error::new(
            "UND_ERR_ABORTED",
            "Request aborted"
        ))
    );
    assert_eq!(connection.poll_completion(), None);
    assert!(!connection.reusable());
}

#[test]
fn streaming_compression_fragmented_bounded_and_truncated() {
    use std::io::Write;
    let body = b"streamed body streamed body streamed body";
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), Default::default());
    gzip.write_all(body).unwrap();
    let mut deflate = flate2::write::DeflateEncoder::new(Vec::new(), Default::default());
    deflate.write_all(body).unwrap();
    let mut br = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut br, 4096, 4, 22);
        writer.write_all(body).unwrap();
    }
    // zstd fixture generated by Node 26 zlib.zstdCompressSync, also run on WASI.
    let cases = vec![
        ("gzip", gzip.finish().unwrap()),
        ("deflate", deflate.finish().unwrap()),
        ("br", br),
        (
            "zstd",
            hex("28b52ffd2029a500007073747265616d656420626f6479200100114e25"),
        ),
    ];
    for (encoding, wire) in cases {
        let mut decoder = turnloop_http::compression::StreamingDecoder::new(encoding, 100).unwrap();
        let mut input = Vec::new();
        let mut result = Vec::new();
        let mut done = false;
        for (i, byte) in wire.iter().enumerate() {
            input.push(*byte);
            loop {
                let mut out = [0; 3];
                let step = decoder
                    .process(&input, &mut out, i + 1 == wire.len())
                    .unwrap();
                result.extend_from_slice(&out[..step.written]);
                input.drain(..step.consumed);
                done = step.finished;
                if done || step.consumed == 0 && step.written == 0 {
                    break;
                }
            }
        }
        if !done {
            let mut out = [0; 100];
            let step = decoder.process(&input, &mut out, true).unwrap();
            result.extend_from_slice(&out[..step.written]);
            done = step.finished;
        }
        assert!(done, "{encoding}");
        assert_eq!(result, body, "{encoding}");
        let mut decoder = turnloop_http::compression::StreamingDecoder::new(encoding, 100).unwrap();
        let mut pos = 0;
        let mut failure = false;
        for _ in 0..100 {
            let mut out = [0; 100];
            match decoder.process(&wire[pos..wire.len() - 1], &mut out, true) {
                Ok(step) => {
                    pos += step.consumed;
                    if step.finished {
                        break;
                    }
                }
                Err(_) => {
                    failure = true;
                    break;
                }
            }
        }
        assert!(failure, "truncated {encoding} must fail");
    }
}
