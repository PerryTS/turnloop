//! Count protocol work after warm-up; result head construction is outside the body hot path.
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {static ENABLED:Cell<bool>=const{Cell::new(false)};static ALLOCATIONS:Cell<usize>=const{Cell::new(0)};}
struct Counter;
fn count() {
    ENABLED.with(|enabled| {
        if enabled.get() {
            ALLOCATIONS.with(|n| n.set(n.get() + 1));
        }
    });
}
// SAFETY: all allocation operations delegate unchanged to the system allocator.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: caller provides GlobalAlloc's layout contract.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: allocation ownership and layout are forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: caller supplies the original allocation and valid new size.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static GLOBAL: Counter = Counter;
fn measured(run: impl FnOnce()) -> usize {
    ALLOCATIONS.with(|n| n.set(0));
    ENABLED.with(|b| b.set(true));
    run();
    ENABLED.with(|b| b.set(false));
    ALLOCATIONS.with(Cell::get)
}
#[test]
fn reusable_serialization_and_hpack_encoder_allocate_zero() {
    use turnloop_http::{hpack, http1::*};
    let head = Head {
        method: "POST".into(),
        target: "/".into(),
        status: 0,
        version: 1,
        headers: vec![
            Header::new("host", "localhost"),
            Header::new("x-custom-header", "custom-value"),
        ],
        keep_alive: true,
    };
    let mut output = Vec::with_capacity(4096);
    let allocations = measured(|| {
        for _ in 0..1000 {
            output.clear();
            let mut encoder = Encoder::start(&head, BodyLength::Known(4), &mut output).unwrap();
            encoder.body(b"body", &mut output).unwrap();
            encoder.finish(&[], &mut output).unwrap();
        }
    });
    assert_eq!(allocations, 0);
    assert!(output.ends_with(b"body"));
    let mut encoder = hpack::Encoder::new(4096);
    encoder.encode(&head.headers, &mut output);
    output.clear();
    let allocations = measured(|| {
        for _ in 0..1000 {
            output.clear();
            encoder.encode(&head.headers, &mut output);
        }
    });
    assert_eq!(allocations, 0);
    assert!(!output.is_empty());
}
#[test]
fn http1_body_and_h2_flow_control_allocate_zero() {
    use turnloop_http::{http1::*, http2};
    let mut decoder = Decoder::new(Mode::Response, Default::default());
    decoder
        .receive(b"HTTP/1.1 200 OK\r\nContent-Length: 4000\r\n\r\n")
        .unwrap();
    let mut bytes = 0;
    let allocations = measured(|| {
        for _ in 0..1000 {
            let step = decoder.receive(b"body").unwrap();
            assert!(matches!(step.event, Some(Event::Body(b"body"))));
            bytes += step.consumed;
        }
    });
    assert_eq!(allocations, 0);
    assert_eq!(bytes, 4000);
    let mut client = http2::Connection::new(http2::Role::Client, Default::default()).unwrap();
    let mut server = http2::Connection::new(http2::Role::Server, Default::default()).unwrap();
    fn transfer(from: &mut http2::Connection, to: &mut http2::Connection) {
        let mut pos = 0;
        while pos < from.output().len() {
            pos += to.receive(&from.output()[pos..]).unwrap().consumed;
        }
        from.consume_output(pos).unwrap();
    }
    transfer(&mut client, &mut server);
    transfer(&mut server, &mut client);
    transfer(&mut client, &mut server);
    let id = client
        .open(
            &[
                Header::new(":method", "POST"),
                Header::new(":scheme", "http"),
                Header::new(":path", "/"),
                Header::new(":authority", "localhost"),
            ],
            false,
        )
        .unwrap();
    transfer(&mut client, &mut server);
    client.send_data(id, b"warm", false).unwrap();
    transfer(&mut client, &mut server);
    server.release_capacity(id, 4).unwrap();
    transfer(&mut server, &mut client);
    let mut accepted = 0;
    let allocations = measured(|| {
        for _ in 0..1000 {
            accepted += client.send_data(id, b"data", false).unwrap();
            transfer(&mut client, &mut server);
            server.release_capacity(id, 4).unwrap();
            transfer(&mut server, &mut client);
        }
    });
    assert_eq!(accepted, 4000);
    assert_eq!(allocations, 0);
}

#[test]
fn streaming_decompressors_reuse_scratch() {
    use std::io::Write;
    use turnloop_http::compression::StreamingDecoder;
    let body = b"allocation profile allocation profile allocation profile";
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), Default::default());
    gzip.write_all(body).unwrap();
    let mut deflate = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
    deflate.write_all(body).unwrap();
    let mut brotli = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut brotli, 4096, 4, 22);
        writer.write_all(body).unwrap();
    }
    let cases = [
        ("gzip", gzip.finish().unwrap()),
        ("deflate", deflate.finish().unwrap()),
        ("br", brotli),
        (
            "zstd",
            "28b52ffd2038cd000098616c6c6f636174696f6e2070726f66696c65200100d933c3"
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap())
                .collect(),
        ),
    ];
    for (name, wire) in cases {
        let mut decoder = StreamingDecoder::new(name, 4096).unwrap();
        let mut count = 0;
        let mut run = || {
            decoder.reset(4096).unwrap();
            let mut pos = 0;
            loop {
                let mut output = [0; 4096];
                let step = decoder.process(&wire[pos..], &mut output, true).unwrap();
                pos += step.consumed;
                count += step.written;
                if step.finished {
                    break;
                }
                assert!(step.consumed > 0 || step.written > 0);
            }
        };
        for _ in 0..5 {
            run();
        }
        let allocations = measured(|| {
            for _ in 0..100 {
                run();
            }
        });
        assert_eq!(allocations, 0, "{name}");
        assert_eq!(count, 105 * body.len());
    }
}
