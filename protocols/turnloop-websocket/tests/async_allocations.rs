//! Compare transport adapters with the identical sans-I/O owned-result workload.
//! The zero-extra-allocation threshold excludes only measured core result storage.
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    future::Future,
    io,
    pin::pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use turnloop_http::{
    asynchronous::Http1,
    http1::{self, BodyLength, Event, Head},
};
use turnloop_io::{AsyncRead, AsyncWrite};
use turnloop_websocket::{Connection, Message, Role, WebSocketStream};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
struct Counter;
fn record() {
    if ACTIVE.load(Ordering::Relaxed) {
        COUNT.fetch_add(1, Ordering::Relaxed);
    }
}
// SAFETY: the allocator forwards every layout and pointer unchanged to System.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        record();
        // SAFETY: caller-provided allocation layout is forwarded unchanged.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        record();
        // SAFETY: caller-provided allocation layout is forwarded unchanged.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        record();
        // SAFETY: pointer, original layout and new size satisfy GlobalAlloc.
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: pointer/layout belong to a prior System allocation.
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;
fn measure(f: impl FnOnce()) -> usize {
    COUNT.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    f();
    ACTIVE.store(false, Ordering::Relaxed);
    COUNT.load(Ordering::Relaxed)
}
fn run<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..10000 {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
    }
    panic!("future failed to progress");
}
struct Wire {
    bytes: &'static [u8],
    offset: usize,
    written: usize,
    pending: bool,
}
impl Wire {
    fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            written: 0,
            pending: false,
        }
    }
}
impl AsyncRead for Wire {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if !self.pending {
            self.pending = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.pending = false;
        if self.offset == self.bytes.len() {
            self.offset = 0;
        }
        let n = out.len().min(self.bytes.len() - self.offset);
        out[..n].copy_from_slice(&self.bytes[self.offset..self.offset + n]);
        self.offset += n;
        Poll::Ready(Ok(n))
    }
}
impl AsyncWrite for Wire {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let n = bytes.len().min(7);
        self.written += n;
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\nbody";
fn http() {
    let head = Head {
        method: "GET".into(),
        target: "/".into(),
        status: 0,
        version: 1,
        headers: vec![],
        keep_alive: true,
    };
    let mut core = http1::Decoder::new(http1::Mode::Response, Default::default());
    let mut output = Vec::with_capacity(65536);
    let mut baseline = || {
        let mut encoder =
            http1::Encoder::start(&head, BodyLength::Empty, &mut output).expect("encode");
        encoder.finish(&[], &mut output).expect("end");
        assert!(!output.is_empty());
        output.clear();
        let mut offset = 0;
        let mut bytes = 0;
        loop {
            let step = core.receive(&RESPONSE[offset..]).expect("decode");
            offset += step.consumed;
            let end = matches!(step.event, Some(Event::End));
            if let Some(Event::Body(body)) = step.event {
                assert_eq!(body, b"body");
                bytes += body.len();
            }
            if end {
                break;
            }
        }
        assert_eq!(bytes, 4);
        core.reset().expect("reset");
    };
    baseline();
    let reference = measure(|| {
        for _ in 0..100 {
            baseline();
        }
    });
    let mut adapter = Http1::new(Wire::new(RESPONSE), http1::Mode::Response);
    let mut round = || {
        run(async {
            adapter
                .send_head(&head, BodyLength::Empty)
                .await
                .expect("send");
            adapter.finish_body(&[]).await.expect("flush");
            let mut bytes = 0;
            loop {
                let mut end = false;
                adapter
                    .event(|event| {
                        if let Event::Body(body) = &event {
                            assert_eq!(*body, b"body");
                            bytes += body.len();
                        }
                        end = matches!(event, Event::End);
                        Ok(())
                    })
                    .await
                    .expect("event");
                if end {
                    break;
                }
            }
            assert_eq!(bytes, 4);
            adapter.reset().expect("reset");
        });
    };
    round();
    let actual = measure(|| {
        for _ in 0..100 {
            round();
        }
    });
    assert_eq!(actual, reference, "HTTP adapter adds scratch allocations");
    let (wire, _) = adapter.into_parts().expect("parts");
    assert!(wire.written > 1000);
    println!("HTTP: 100 exchanges, {reference} core-owned head allocations, 0 adapter allocations");
}
fn websocket() {
    static FRAME: &[u8] = b"\x82\x04body";
    let mut core = Connection::new(Role::Client, Default::default());
    let mut output = Vec::with_capacity(65536);
    let mut baseline = || {
        let step = core.receive(FRAME, &mut output).expect("frame");
        assert_eq!(step.consumed, FRAME.len());
        assert_eq!(
            step.message,
            Some(Message::Binary(b"body".as_slice().into()))
        );
    };
    baseline();
    let reference = measure(|| {
        for _ in 0..100 {
            baseline();
        }
    });
    assert_eq!(reference, 0, "core frame reads allocate");
    let mut adapter = WebSocketStream::from_upgrade(
        Wire::new(FRAME),
        Vec::with_capacity(65536),
        Role::Client,
        Default::default(),
    );
    let mut round = || {
        assert_eq!(
            run(adapter.receive()).expect("receive"),
            Some(Message::Binary(b"body".as_slice().into()))
        );
    };
    round();
    let actual = measure(|| {
        for _ in 0..100 {
            round();
        }
    });
    assert_eq!(
        actual, reference,
        "WebSocket adapter adds receive allocations"
    );
    let mut writer = WebSocketStream::from_upgrade(
        Wire::new(FRAME),
        Vec::with_capacity(65536),
        Role::Server,
        Default::default(),
    );
    run(writer.send(Message::Binary(b"body".as_slice().into()))).expect("warm send");
    assert_eq!(
        measure(|| for _ in 0..100 {
            run(writer.send(Message::Binary(b"body".as_slice().into()))).expect("send");
        }),
        0,
        "server frame writes allocate"
    );
    println!(
        "WebSocket: 100 reads, {reference} core-owned message allocations, 0 adapter allocations; 100 writes, 0 allocations"
    );
}
fn http2_data_flow() {
    use turnloop_http::{
        asynchronous::Http2,
        http1::Header,
        http2::{Event, Role},
    };
    // A real open stream and peer settings precede the measured DATA frames.
    let mut conn = Http2::new(
        Wire::new(b"\x00\x00\x04\x00\x00\x00\x00\x00\x01body"),
        Role::Client,
    )
    .expect("h2");
    assert_eq!(
        conn.core
            .open(
                &[
                    Header::new(":method", "POST"),
                    Header::new(":scheme", "http"),
                    Header::new(":authority", "example.test"),
                    Header::new(":path", "/"),
                ],
                false
            )
            .expect("stream"),
        1
    );
    assert_eq!(
        conn.core
            .receive(b"\x00\x00\x00\x04\x00\x00\x00\x00\x00")
            .expect("settings")
            .consumed,
        9
    );
    let head = conn
        .core
        .receive(b"\x00\x00\x01\x01\x04\x00\x00\x00\x01\x88")
        .expect("response headers");
    assert!(matches!(head.event, Some(Event::Headers { stream: 1, .. })));
    let mut bytes_seen = 0;
    let mut round = || {
        assert!(
            run(conn.event(|core, event| {
                let Event::Data {
                    stream,
                    bytes,
                    end_stream,
                } = event
                else {
                    panic!("DATA required")
                };
                assert_eq!(stream, 1);
                assert!(!end_stream);
                assert_eq!(bytes, b"body");
                bytes_seen += bytes.len();
                core.release_capacity(stream, bytes.len() as u32)
                    .map_err(io::Error::other)
            }))
            .expect("DATA and flow credit")
        );
    };
    round();
    assert_eq!(
        measure(|| {
            for _ in 0..100 {
                round();
            }
        }),
        0,
        "HTTP/2 DATA/credit adapter allocates"
    );
    assert_eq!(bytes_seen, 404);
    println!("HTTP/2: 100 DATA deliveries and flow-credit writes, zero allocations");
}

fn main() {
    assert!(
        measure(|| {
            std::hint::black_box(Box::new([0u8; 128]));
        }) > 0,
        "allocator must observe allocation"
    );
    http();
    http2_data_flow();
    websocket();
    println!("test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;");
}
