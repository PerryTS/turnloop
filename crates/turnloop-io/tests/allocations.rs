#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod native {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        future::Future,
        io,
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
        time::Duration,
    };
    use turnloop_io::{
        turnloop::{Config, LocalExecutor, Timeout, backend::Platform},
        *,
    };
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

    struct Codec {
        pending: bool,
        ready: bool,
        bytes: [u8; 4],
        received: usize,
        total: usize,
    }
    impl Output for Codec {
        fn output(&self) -> &[u8] {
            if self.pending { b"ping" } else { &[] }
        }
        fn consume_output(&mut self, n: usize) -> io::Result<()> {
            assert_eq!(n, 4);
            self.pending = false;
            Ok(())
        }
    }
    impl SansIo for Codec {
        type Event<'a> = &'a [u8];
        fn event(
            &mut self,
            mut receive: impl FnMut(Self::Event<'_>) -> io::Result<()>,
        ) -> io::Result<bool> {
            if self.ready {
                receive(&self.bytes)?;
                self.ready = false;
                self.received = 0;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        fn ingest(&mut self, bytes: &[u8], _: Instant) -> io::Result<usize> {
            let n = bytes.len().min(4 - self.received);
            self.bytes[self.received..self.received + n].copy_from_slice(&bytes[..n]);
            self.received += n;
            self.total += n;
            self.ready = self.received == 4;
            Ok(n)
        }
        fn disconnected(&mut self) {}
    }
    pub fn real_tcp_shared_driver_zero_allocations_after_warmup() {
        assert!(
            measure(|| {
                std::hint::black_box(Box::new([0u8; 128]));
            }) > 0
        );
        let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
        let h = executor.handle();
        let listener =
            Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
        let address = listener.local_addr().expect("address");
        let server = executor
            .spawn_local(async move {
                let mut stream = listener.accept().await.expect("accept");
                let mut rounds = 0;
                for _ in 0..1001 {
                    let mut bytes = [0; 4];
                    let mut n = 0;
                    while n < 4 {
                        let got = read(&mut stream, &mut bytes[n..]).await.expect("read");
                        assert!(got > 0);
                        n += got;
                    }
                    assert_eq!(&bytes, b"ping");
                    write_all(&mut stream, b"pong").await.expect("write");
                    rounds += 1;
                }
                assert_eq!(rounds, 1001);
            })
            .expect("spawn");
        let mut client = executor
            .spawn_local(async move {
                let stream = h
                    .connect(address, Default::default())
                    .await
                    .expect("connect");
                let mut driver = Driver::new(
                    stream,
                    Codec {
                        pending: true,
                        ready: false,
                        bytes: [0; 4],
                        received: 0,
                        total: 0,
                    },
                );
                for i in 0..1001 {
                    driver.core_mut().pending = true;
                    driver
                        .next(&h, |bytes| {
                            assert_eq!(bytes, b"pong");
                            Ok(())
                        })
                        .await
                        .expect("driver");
                    if i == 0 {
                        COUNT.store(0, Ordering::Relaxed);
                        ACTIVE.store(true, Ordering::Relaxed);
                    }
                }
                ACTIVE.store(false, Ordering::Relaxed);
                assert_eq!(driver.core().total, 4004);
                COUNT.load(Ordering::Relaxed)
            })
            .expect("spawn");
        let end = executor.driver().now() + Duration::from_secs(10);
        while !client.is_finished() || !server.is_finished() {
            assert!(executor.driver().now() < end);
            executor.turn(Timeout::Until(end)).expect("turn");
        }
        match Pin::new(&mut client).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(Ok(n)) => assert_eq!(n, 0, "shared driver allocates per operation"),
            _ => panic!("client incomplete"),
        }
    }
    /// Server-side progress the peer lock-steps on, so the gate never depends on
    /// how a kernel coalesces paced writes into reads.
    #[derive(Default)]
    struct Progress {
        shutdowns: std::cell::Cell<u32>,
        read: std::cell::Cell<u64>,
        peer: std::cell::RefCell<Option<Waker>>,
    }
    impl Progress {
        fn wake_peer(&self) {
            if let Some(waker) = self.peer.borrow_mut().take() {
                waker.wake();
            }
        }
        async fn until(&self, ready: impl Fn(&Self) -> bool) {
            std::future::poll_fn(|cx| {
                if ready(self) {
                    Poll::Ready(())
                } else {
                    *self.peer.borrow_mut() = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
            .await
        }
    }
    /// Transparent server transport that publishes half-close and read progress.
    struct Observed {
        inner: AsyncIo<Platform>,
        progress: std::rc::Rc<Progress>,
        shut: bool,
    }
    impl AsyncRead for Observed {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            let this = self.get_mut();
            let result = Pin::new(&mut this.inner).poll_read(cx, bytes);
            if let Poll::Ready(Ok(n @ 1..)) = &result {
                this.progress.read.set(this.progress.read.get() + *n as u64);
                this.progress.wake_peer();
            }
            result
        }
    }
    impl AsyncWrite for Observed {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            Pin::new(&mut self.get_mut().inner).poll_write(cx, bytes)
        }
        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_flush(cx)
        }
        fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_close(cx)
        }
    }
    impl HalfClose for Observed {
        type Backend = Platform;
        fn executor(&self) -> Option<ExecutorHandle<Platform>> {
            HalfClose::executor(&self.inner)
        }
        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            std::task::ready!(HalfClose::poll_shutdown(Pin::new(&mut this.inner), cx))?;
            if !std::mem::replace(&mut this.shut, true) {
                // Once per connection: its discard count starts here.
                this.progress.read.set(0);
                this.progress
                    .shutdowns
                    .set(this.progress.shutdowns.get() + 1);
                this.progress.wake_peer();
            }
            Poll::Ready(Ok(()))
        }
    }
    /// Lingering close over real TCP: the half-close, every discard read of late
    /// peer bytes, the deadline timer and the final close allocate nothing once a
    /// first identical connection has warmed the executor and driver tables.
    pub fn lingering_close_discards_without_allocating() {
        const LATE: u64 = 256;
        let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
        let h = executor.handle();
        let client_h = h.clone();
        let listener =
            Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
        let address = listener.local_addr().expect("address");
        let progress = std::rc::Rc::new(Progress::default());
        let server_progress = progress.clone();
        let server = executor
            .spawn_local(async move {
                let mut scratch = [0; 64];
                let mut measured = None;
                for round in 0..2 {
                    let mut stream = Observed {
                        inner: listener.accept().await.expect("accept"),
                        progress: server_progress.clone(),
                        shut: false,
                    };
                    write_all(&mut stream, b"final")
                        .await
                        .expect("final response");
                    if round == 1 {
                        COUNT.store(0, Ordering::Relaxed);
                        ACTIVE.store(true, Ordering::Relaxed);
                    }
                    let lingered = linger_close(
                        &mut stream,
                        &mut scratch,
                        Some(h.now() + Duration::from_secs(10)),
                    )
                    .await
                    .expect("lingering close");
                    if round == 1 {
                        ACTIVE.store(false, Ordering::Relaxed);
                        measured = Some((lingered, COUNT.load(Ordering::Relaxed)));
                    }
                    assert_eq!(lingered.end, LingerEnd::Eof);
                    assert_eq!(lingered.discarded, 4 * LATE);
                }
                measured.expect("measured round")
            })
            .expect("spawn");
        let client = executor
            .spawn_local(async move {
                for round in 0..2 {
                    let mut stream = client_h
                        .connect(address, Default::default())
                        .await
                        .expect("connect");
                    let mut bytes = [0; 8];
                    let mut n = 0;
                    loop {
                        let got = read(&mut stream, &mut bytes[n..]).await.expect("read");
                        if got == 0 {
                            break;
                        }
                        n += got;
                    }
                    assert_eq!(&bytes[..n], b"final");
                    // The server may observe its half-close after our EOF.
                    progress.until(|p| p.shutdowns.get() > round).await;
                    // Lock-step: the next chunk goes out only after the server
                    // discarded the previous one, so each chunk is its own read.
                    for chunk in 1..=LATE {
                        write_all(&mut stream, b"late").await.expect("late bytes");
                        progress.until(|p| p.read.get() >= 4 * chunk).await;
                    }
                    close(&mut stream).await.expect("close");
                }
            })
            .expect("spawn");
        let end = executor.driver().now() + Duration::from_secs(20);
        let mut server = server;
        let mut client = client;
        while !client.is_finished() || !server.is_finished() {
            assert!(executor.driver().now() < end);
            executor.turn(Timeout::Until(end)).expect("turn");
        }
        match Pin::new(&mut client).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(Ok(())) => {}
            _ => panic!("client incomplete"),
        }
        let (lingered, allocations) =
            match Pin::new(&mut server).poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Ready(Ok(v)) => v,
                _ => panic!("server incomplete"),
            };
        // The subject ran: at least one discard read per lock-stepped chunk, every
        // byte accounted for, all inside the measured window.
        assert!(
            lingered.reads >= LATE,
            "one discard read per chunk: {lingered:?}"
        );
        assert_eq!(allocations, 0, "lingering close allocates: {lingered:?}");
        println!(
            "lingering close: {} discard reads, {} bytes, {allocations} allocations",
            lingered.reads, lingered.discarded
        );
    }
}
fn main() {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        native::real_tcp_shared_driver_zero_allocations_after_warmup();
        println!("test real_tcp_shared_driver_zero_allocations_after_warmup ... ok");
        native::lingering_close_discards_without_allocating();
        println!("test lingering_close_discards_without_allocating ... ok");
        println!("test result: ok. 2 passed; 0 failed; 0 ignored;");
    }
}
