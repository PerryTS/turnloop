//! Real cryptography over fragmented memory transport: no threads or native sockets.
//! Standalone harness also avoids the pinned WASI 0.3 libtest allocator startup bug.
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    time::{Duration, Instant},
};
use turnloop_tls::{
    Client, ClientConfig, ClientOptions, ConnectionState, Server, ServerConfig, UnbufferedStatus,
    rustls::{self, HandshakeKind, pki_types::ServerName},
};

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
struct Counter;
// SAFETY: the allocator forwards each caller's layout and allocation unchanged.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
        // SAFETY: GlobalAlloc's caller guarantees a valid layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc's caller guarantees ownership and the original layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
        // SAFETY: GlobalAlloc's caller guarantees ownership, layout and new size.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;

const NOW: u64 = 1_789_344_000;
trait Endpoint {
    type Data;
    fn process<'c, 'i>(&'c mut self, input: &'i mut [u8]) -> UnbufferedStatus<'c, 'i, Self::Data>;
}
impl Endpoint for Client {
    type Data = rustls::client::ClientConnectionData;
    fn process<'c, 'i>(&'c mut self, input: &'i mut [u8]) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process(input, NOW)
    }
}
impl Endpoint for Server {
    type Data = rustls::server::ServerConnectionData;
    fn process<'c, 'i>(&'c mut self, input: &'i mut [u8]) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process(input, NOW)
    }
}
struct Peer<E> {
    tls: E,
    input: Vec<u8>,
    encoded: Vec<u8>,
    wire: Vec<u8>,
    plain: Vec<u8>,
    records: usize,
    transmissions: usize,
}
impl<E: Endpoint> Peer<E> {
    fn new(tls: E) -> Self {
        Self {
            tls,
            input: Vec::with_capacity(65536),
            encoded: Vec::with_capacity(131072),
            wire: Vec::with_capacity(65536),
            plain: Vec::with_capacity(65536),
            records: 0,
            transmissions: 0,
        }
    }
    fn step(&mut self, app: &mut Option<&[u8]>) -> Result<(), rustls::Error> {
        let status = self.tls.process(&mut self.input);
        let mut discard = status.discard;
        match status.state? {
            ConnectionState::EncodeTlsData(mut encode) => {
                let start = self.encoded.len();
                self.encoded.resize(start + 65536, 0);
                let n = encode
                    .encode(&mut self.encoded[start..])
                    .expect("handshake output capacity");
                self.encoded.truncate(start + n);
            }
            ConnectionState::TransmitTlsData(tx) => {
                self.wire.extend_from_slice(&self.encoded);
                self.encoded.clear();
                self.transmissions += 1;
                // Acknowledge only after the transport owns every ciphertext byte.
                tx.done();
            }
            ConnectionState::ReadTraffic(mut read) => {
                while let Some(record) = read.next_record() {
                    let record = record?;
                    discard += record.discard;
                    self.plain.extend_from_slice(record.payload);
                    self.records += 1;
                }
            }
            ConnectionState::WriteTraffic(mut write) => {
                if let Some(bytes) = app.take() {
                    self.encoded.resize(65536, 0);
                    let n = write
                        .encrypt(bytes, &mut self.encoded)
                        .expect("record output capacity");
                    assert!(
                        n > bytes.len(),
                        "a TLS record must include authenticated framing"
                    );
                    assert!(
                        !self.encoded[..n]
                            .windows(bytes.len())
                            .any(|window| window == bytes),
                        "transport must carry ciphertext, not plaintext"
                    );
                    self.wire.extend_from_slice(&self.encoded[..n]);
                    self.encoded.clear();
                }
            }
            ConnectionState::BlockedHandshake => {}
            state => panic!("unexpected state during live TLS exchange: {state:?}"),
        }
        self.input.drain(..discard);
        Ok(())
    }
    fn transfer(&mut self, receiver: &mut impl Extend<u8>) -> usize {
        let n = self.wire.len().min(137); // Splits record headers and handshake messages.
        receiver.extend(self.wire.drain(..n));
        n
    }
}
struct Pair {
    client: Peer<Client>,
    server: Peer<Server>,
}
impl Pair {
    fn new(client: &ClientConfig, server: &ServerConfig, name: &'static str) -> Self {
        Self {
            client: Peer::new(
                client
                    .connect(ServerName::try_from(name).expect("DNS name"))
                    .expect("client"),
            ),
            server: Peer::new(server.accept().expect("server")),
        }
    }
    fn exchange(&mut self, request: &[u8], response: &[u8]) -> Result<(), rustls::Error> {
        self.client.plain.clear();
        self.server.plain.clear();
        let mut request_pending = Some(request);
        let mut response_pending = Some(response);
        let mut wire_bytes = 0;
        let initial = self.client.records + self.server.records;
        for _ in 0..10000 {
            self.client.step(&mut request_pending)?;
            wire_bytes += self.client.transfer(&mut self.server.input);
            self.server.step(&mut response_pending)?;
            wire_bytes += self.server.transfer(&mut self.client.input);
            if self.server.plain == request && self.client.plain == response {
                assert!(request_pending.is_none() && response_pending.is_none());
                assert!(wire_bytes > request.len() + response.len());
                assert!(self.client.records + self.server.records >= initial + 2);
                return Ok(());
            }
        }
        panic!(
            "TLS exchange did not complete; client={} server={} ciphertext={wire_bytes}",
            self.client.plain.len(),
            self.server.plain.len()
        );
    }
}
fn certificate() -> rcgen::CertifiedKey {
    rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .expect("ring must generate a signing key")
}
fn server(cert: &rcgen::CertifiedKey) -> ServerConfig {
    ServerConfig::new(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        NOW,
    )
    .expect("server config")
}
fn options(cert: &rcgen::CertifiedKey) -> ClientOptions {
    ClientOptions {
        ca: Some(vec![cert.cert.der().clone()]),
        alpn: vec![b"h2".to_vec()],
        ..Default::default()
    }
}
fn handshake_records_alpn_and_resumption() {
    let cert = certificate();
    let server = server(&cert);
    let client = ClientConfig::new(options(&cert), NOW).expect("client config");
    for kind in [HandshakeKind::Full, HandshakeKind::Resumed] {
        let mut pair = Pair::new(&client, &server, "localhost");
        pair.exchange(b"verified client payload", b"verified server payload")
            .expect("TLS exchange");
        assert!(!pair.client.tls.is_handshaking() && !pair.server.tls.is_handshaking());
        assert_eq!(pair.client.tls.handshake_kind(), Some(kind));
        assert_eq!(pair.server.tls.handshake_kind(), Some(kind));
        assert_eq!(pair.client.tls.alpn_protocol(), Some(b"h2".as_slice()));
        assert_eq!(pair.server.tls.alpn_protocol(), Some(b"h2".as_slice()));
        assert!(pair.client.transmissions > 0 && pair.server.transmissions > 0);
    }
}
fn certificate_errors_and_explicit_insecure_option() {
    let cert = certificate();
    let mut cases = 0;
    for (options, name, expected) in [
        (
            ClientOptions::default(),
            "localhost",
            "UNABLE_TO_VERIFY_LEAF_SIGNATURE",
        ),
        (
            options(&cert),
            "wrong.invalid",
            "ERR_TLS_CERT_ALTNAME_INVALID",
        ),
    ] {
        let client = ClientConfig::new(options, NOW).expect("client config");
        let error = Pair::new(&client, &server(&cert), name)
            .exchange(b"request", b"response")
            .expect_err("certificate rejection");
        assert_eq!(turnloop_tls::node_error_code(&error), expected);
        cases += 1;
    }
    for (start, end, expected) in [
        (2010, 2020, "CERT_HAS_EXPIRED"),
        (2030, 2040, "CERT_NOT_YET_VALID"),
    ] {
        let mut params =
            rcgen::CertificateParams::new(vec!["localhost".into()]).expect("certificate params");
        params.not_before = rcgen::date_time_ymd(start, 1, 1);
        params.not_after = rcgen::date_time_ymd(end, 1, 1);
        let key_pair = rcgen::KeyPair::generate().expect("signing key");
        let cert = rcgen::CertifiedKey {
            cert: params.self_signed(&key_pair).expect("signed certificate"),
            key_pair,
        };
        let client = ClientConfig::new(options(&cert), NOW).expect("client config");
        let error = Pair::new(&client, &server(&cert), "localhost")
            .exchange(b"request", b"response")
            .expect_err("certificate validity rejection");
        assert_eq!(turnloop_tls::node_error_code(&error), expected);
        cases += 1;
    }
    let client = ClientConfig::new(
        ClientOptions {
            reject_unauthorized: false,
            ..Default::default()
        },
        NOW,
    )
    .expect("client config");
    Pair::new(&client, &server(&cert), "wrong.invalid")
        .exchange(b"request", b"response")
        .expect("explicit insecure option");
    assert_eq!(cases, 4);
}
fn injected_timeout_completes_once() {
    let mut client = ClientConfig::new(ClientOptions::default(), NOW)
        .expect("config")
        .connect(ServerName::try_from("localhost").expect("name"))
        .expect("client");
    let now = Instant::now();
    client.set_handshake_deadline(Some(now + Duration::from_micros(500)));
    assert_eq!(client.handle_timeout(now), None);
    assert_eq!(
        client.handle_timeout(now + Duration::from_micros(500)),
        Some("ERR_TLS_HANDSHAKE_TIMEOUT")
    );
    assert_eq!(client.handle_timeout(now + Duration::from_secs(1)), None);
    assert!(client.process(&mut [], NOW).state.is_err());
}
fn records_allocate_zero_after_warmup() {
    COUNTING.store(true, Relaxed);
    let calibration = std::hint::black_box(vec![42u8; 256]);
    COUNTING.store(false, Relaxed);
    assert!(
        ALLOCATIONS.load(Relaxed) > 0,
        "allocator calibration must execute"
    );
    drop(calibration);
    let cert = certificate();
    let client = ClientConfig::new(options(&cert), NOW).expect("config");
    let mut pair = Pair::new(&client, &server(&cert), "localhost");
    let request = [42; 1024];
    let response = [71; 1024];
    for _ in 0..3 {
        pair.exchange(&request, &response).expect("warmup exchange");
    }
    let initial = pair.client.records + pair.server.records;
    ALLOCATIONS.store(0, Relaxed);
    COUNTING.store(true, Relaxed);
    for _ in 0..1000 {
        pair.exchange(&request, &response)
            .expect("measured exchange");
    }
    COUNTING.store(false, Relaxed);
    assert_eq!(pair.client.records + pair.server.records - initial, 2000);
    assert_eq!(
        ALLOCATIONS.load(Relaxed),
        0,
        "steady-state TLS records allocated"
    );
}
fn main() {
    let tests: &[(&str, fn())] = &[
        (
            "handshake_records_alpn_and_resumption",
            handshake_records_alpn_and_resumption,
        ),
        (
            "certificate_errors_and_explicit_insecure_option",
            certificate_errors_and_explicit_insecure_option,
        ),
        (
            "injected_timeout_completes_once",
            injected_timeout_completes_once,
        ),
        (
            "records_allocate_zero_after_warmup",
            records_allocate_zero_after_warmup,
        ),
    ];
    for (name, test) in tests {
        test();
        println!("test {name} ... ok");
    }
    println!(
        "test result: ok. {} passed; 0 failed; 0 ignored;",
        tests.len()
    );
}
