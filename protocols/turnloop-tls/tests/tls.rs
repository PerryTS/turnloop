mod support;
use std::{io::{Read, Write}, net::{TcpListener, TcpStream}, thread, time::{Duration, Instant}};
use support::*;
use turnloop_tls::{ClientConfig, ClientOptions, rustls::{self, pki_types::ServerName}};

#[test]
fn sockets_alpn_sni_payload_and_resumption() {
    let cert = certificate();
    let server = server_config(&cert);
    let client = ClientConfig::new(ClientOptions { extra_ca_pem: cert.cert.pem().into_bytes(),
        alpn: vec![b"h2".to_vec(), b"http/1.1".to_vec()], ..Default::default() }, NOW).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        for _ in 0..2 {
            let mut stream = Stream::new(server.accept().unwrap(), listener.accept().unwrap().0);
            let mut request = [0; 4]; stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"ping");
            assert_eq!(stream.engine.alpn_protocol(), Some(b"h2".as_slice()));
            stream.write_all(b"pong").unwrap();
        }
    });
    for index in 0..2 {
        let mut stream = Stream::new(client.connect(ServerName::try_from("localhost").unwrap()).unwrap(), TcpStream::connect(address).unwrap());
        stream.write_all(b"ping").unwrap();
        let mut response = [0; 4]; stream.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        assert_eq!(stream.engine.alpn_protocol(), Some(b"h2".as_slice()));
        assert_eq!(stream.engine.handshake_kind(), Some(if index == 0 { rustls::HandshakeKind::Full } else { rustls::HandshakeKind::Resumed }));
    }
    task.join().unwrap();
}

fn attempt(options: ClientOptions, name: &'static str) -> Result<(), String> {
    let cert = certificate();
    let config = server_config(&cert);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let address = listener.local_addr().unwrap();
    let task = thread::spawn(move || {
        let mut stream = Stream::new(config.accept().unwrap(), listener.accept().unwrap().0);
        let mut data = [0]; let _ = stream.read_exact(&mut data);
    });
    let mut stream = Stream::new(ClientConfig::new(options, NOW).unwrap().connect(ServerName::try_from(name).unwrap()).unwrap(), TcpStream::connect(address).unwrap());
    let result = stream.write_all(b"x").map_err(|e| {
        let tls = e.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()).unwrap();
        turnloop_tls::node_error_code(tls).to_string()
    });
    drop(stream); task.join().unwrap(); result
}
#[test]
fn untrusted_chain_and_explicit_insecure_option() {
    assert_eq!(attempt(ClientOptions::default(), "localhost").unwrap_err(), "UNABLE_TO_VERIFY_LEAF_SIGNATURE");
    attempt(ClientOptions { reject_unauthorized: false, ..Default::default() }, "wrong.invalid").unwrap();
}
#[test]
fn injected_deadline_is_terminal_once() {
    let mut client = ClientConfig::new(ClientOptions::default(), NOW).unwrap().connect(ServerName::try_from("localhost").unwrap()).unwrap();
    let now = Instant::now(); client.set_handshake_deadline(Some(now + Duration::from_secs(1)));
    assert_eq!(client.handle_timeout(now), None);
    assert_eq!(client.handle_timeout(now + Duration::from_secs(1)), Some("ERR_TLS_HANDSHAKE_TIMEOUT"));
    assert_eq!(client.handle_timeout(now + Duration::from_secs(2)), None);
    assert!(client.process(&mut [], NOW).state.is_err());
}
