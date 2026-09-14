//! TLS record gate over real AsyncIo sockets. Rustls 0.23.45 owns two allocations
//! per decrypted record; retain its existing four-allocation bidirectional baseline.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod native {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
        time::Duration,
    };
    use turnloop_io::{
        turnloop::{Config, LocalExecutor, Timeout, backend::Platform},
        *,
    };
    use turnloop_tls::{
        ClientConfig, ClientOptions, ServerConfig, TlsStream,
        rustls::pki_types::{PrivatePkcs8KeyDer, ServerName},
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

    fn finish<T>(task: &mut turnloop::executor::JoinHandle<T>) -> T {
        match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(Ok(v)) => v,
            _ => panic!("task incomplete"),
        }
    }
    pub fn run() {
        assert!(
            measure(|| {
                std::hint::black_box(Box::new([0u8; 128]));
            }) > 0
        );
        const NOW: u64 = 1_789_344_000;
        let cert =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("certificate");
        let tls_server = ServerConfig::new(
            vec![cert.cert.der().clone()],
            PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            vec![b"h2".to_vec()],
            NOW,
        )
        .expect("server config");
        let tls_client = ClientConfig::new(
            ClientOptions {
                ca: Some(vec![cert.cert.der().clone()]),
                alpn: vec![b"h2".to_vec()],
                ..Default::default()
            },
            NOW,
        )
        .expect("client config");
        let mut executor = LocalExecutor::<Platform>::new(Config::default()).expect("executor");
        let h = executor.handle();
        let client_h = h.clone();
        let listener =
            Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listener");
        let address = listener.local_addr().expect("address");
        let end = h.now() + Duration::from_secs(10);
        let mut server = executor
            .spawn_local(async move {
                let stream = listener.accept().await.expect("accept");
                let mut tls = TlsStream::accept(stream, &tls_server, &h, end, NOW)
                    .await
                    .expect("handshake");
                let mut total = 0;
                for _ in 0..110 {
                    let mut bytes = [0; 64];
                    let mut n = 0;
                    while n < bytes.len() {
                        let got = read(&mut tls, &mut bytes[n..]).await.expect("read");
                        assert!(got > 0);
                        n += got;
                    }
                    assert_eq!(bytes, [b'x'; 64]);
                    write_all(&mut tls, &bytes).await.expect("echo");
                    total += n;
                }
                total
            })
            .expect("spawn");
        let mut client = executor
            .spawn_local(async move {
                let stream = client_h
                    .connect(address, Default::default())
                    .await
                    .expect("connect");
                let mut tls = TlsStream::connect(
                    stream,
                    &tls_client,
                    ServerName::try_from("localhost").expect("name"),
                    &client_h,
                    end,
                    NOW,
                )
                .await
                .expect("handshake");
                let expected_leaf = cert.cert.der();
                let expected_digest = turnloop_tls::tls_server_end_point(expected_leaf)
                    .expect("ECDSA SHA-256 certificate binding");
                let mut bindings = 0;
                assert_eq!(
                    measure(|| {
                        for _ in 0..1000 {
                            let chain = tls.peer_certificates().expect("verified peer chain");
                            assert_eq!(chain, std::slice::from_ref(expected_leaf));
                            let digest = turnloop_tls::tls_server_end_point(chain[0].as_ref())
                                .expect("binding");
                            assert_eq!(digest.as_ref(), expected_digest.as_ref());
                            bindings += 1;
                        }
                    }),
                    0,
                    "peer chain access and binding must allocate zero"
                );
                assert_eq!(bindings, 1000);
                for round in 0..110 {
                    if round == 10 {
                        COUNT.store(0, Ordering::Relaxed);
                        ACTIVE.store(true, Ordering::Relaxed);
                    }
                    write_all(&mut tls, &[b'x'; 64]).await.expect("write");
                    let mut bytes = [0; 64];
                    let mut n = 0;
                    while n < bytes.len() {
                        let got = read(&mut tls, &mut bytes[n..]).await.expect("read");
                        assert!(got > 0);
                        n += got;
                    }
                    assert_eq!(bytes, [b'x'; 64]);
                }
                ACTIVE.store(false, Ordering::Relaxed);
                COUNT.load(Ordering::Relaxed)
            })
            .expect("spawn");
        while !server.is_finished() || !client.is_finished() {
            assert!(executor.driver().now() < end);
            executor.turn(Timeout::Until(end)).expect("turn");
        }
        let allocations = finish(&mut client);
        assert_eq!(finish(&mut server), 110 * 64);
        assert_eq!(
            allocations,
            4 * 100,
            "adapter must add zero allocations beyond rustls record ownership"
        );
        println!(
            "TLS: 100 bidirectional records, {allocations} rustls-owned allocations, zero adapter overhead"
        );
        println!("test result: ok. 1 passed; 0 failed; 0 ignored;");
    }
}
fn main() {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    native::run();
}
