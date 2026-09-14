use std::{
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use turnloop_mongodb::{
    bson::{
        raw::{RawDocument, RawDocumentBuf},
        Document,
    },
    uri::Options,
    Connection, ConnectionEvent, Error, ErrorKind, Result,
};
pub fn raw(d: &Document) -> RawDocumentBuf {
    RawDocumentBuf::try_from(d).expect("fixture operation must succeed")
}
pub fn tools_dir() -> PathBuf {
    std::env::var_os("TURNLOOP_TEST_MONGODB_TOOLS")
        .expect("TURNLOOP_TEST_MONGODB_TOOLS required; run scripts/test-servers.py")
        .into()
}
pub fn ports() -> Vec<(String, u16)> {
    let port = |name| std::env::var(name).expect("required private MongoDB port").parse::<u16>().expect("valid port");
    let mut ports = vec![("standalone".to_owned(), port("TURNLOOP_TEST_MONGODB_PORT")),
        ("tls".to_owned(), port("TURNLOOP_TEST_MONGODB_TLS_PORT"))];
    let replicas = std::env::var("TURNLOOP_TEST_MONGODB_REPLICA_PORTS").expect("required replica ports");
    let replicas: Vec<_> = replicas.split(',').collect();
    assert_eq!(replicas.len(), 3, "three replica members must run");
    for (i, value) in replicas.iter().enumerate() {
        ports.push((format!("rs{i}"), value.parse().expect("replica port")));
    }
    ports
}
trait Socket: Read + Write {}
impl<T: Read + Write> Socket for T {}
pub struct Driver {
    pub core: Connection,
    stream: Box<dyn Socket>,
    token: u64,
}
impl Driver {
    pub fn connect(uri: &str) -> Result<Self> {
        let options = Options::parse(uri)?;
        let address = options.seeds[0].authority();
        let socket = TcpStream::connect(address).map_err(network)?;
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(network)?;
        socket
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(network)?;
        let tls = options.tls;
        let mut core = Connection::new(options);
        let mut entropy = [0u8; 24];
        rustls::crypto::ring::default_provider()
            .secure_random.fill(&mut entropy).expect("test nonce entropy");
        use base64::Engine;
        let nonce = base64::engine::general_purpose::STANDARD.encode(entropy);
        core.connected(Instant::now(), &nonce)?;
        let stream: Box<dyn Socket> = if tls {
            assert!(matches!(
                core.poll_event(),
                Some(ConnectionEvent::UpgradeTls)
            ));
            let mut roots = rustls::RootCertStore::empty();
            let cert = std::fs::read(tools_dir().join("cert.pem")).map_err(network)?;
            for c in rustls_pemfile::certs(&mut cert.as_slice()) {
                roots.add(c.expect("fixture operation must succeed")).expect("fixture operation must succeed");
            }
            let cfg = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let mut conn =
                rustls::ClientConnection::new(Arc::new(cfg), "localhost".try_into().expect("fixture operation must succeed"))
                    .expect("fixture operation must succeed");
            let mut socket = socket;
            while conn.is_handshaking() {
                conn.complete_io(&mut socket).map_err(network)?;
            }
            core.tls_established()?;
            Box::new(rustls::StreamOwned::new(conn, socket))
        } else {
            Box::new(socket)
        };
        let mut d = Self {
            core,
            stream,
            token: 0,
        };
        loop {
            if let Some(event) = d.core.poll_event() {
                match event {
                    ConnectionEvent::Ready => break,
                    ConnectionEvent::Failed { error, .. } => return Err(error),
                    e => panic!("Unexpected handshake event {e:?}"),
                }
            }
            d.turn()?;
        }
        Ok(d)
    }
    pub fn turn(&mut self) -> Result<()> {
        while !self.core.transmit().is_empty() {
            let n = self.stream.write(self.core.transmit()).map_err(network)?;
            if n == 0 {
                return Err(Error::protocol("Socket write returned zero"));
            }
            self.core.consume_transmit(n)?;
        }
        self.stream.flush().map_err(network)?;
        let mut b = [0u8; 8192];
        let n = self.stream.read(&mut b).map_err(network)?;
        if n == 0 {
            return Err(Error::new(ErrorKind::Network, "Socket closed"));
        }
        let mut at = 0;
        while at < n {
            let used = self.core.receive(&b[at..n])?;
            assert!(used > 0);
            at += used;
        }
        Ok(())
    }
    pub fn raw_command(
        &mut self,
        body: &RawDocument,
        sequences: &[(&str, &[&RawDocument])],
    ) -> Result<Document> {
        self.token += 1;
        self.core
            .command(self.token, body, sequences, Instant::now())?;
        loop {
            if let Some(event) = self.core.poll_event() {
                match event {
                    ConnectionEvent::Reply { token } => {
                        assert_eq!(token, self.token);
                        let r = self
                            .core
                            .reply()?
                            .try_into()
                            .map_err(|_| Error::protocol("Cannot own reply"));
                        self.core.release_reply()?;
                        return r;
                    }
                    ConnectionEvent::Failed { error, .. } => return Err(error),
                    e => panic!("Unexpected command event {e:?}"),
                }
            }
            self.turn()?;
        }
    }
    pub fn run(&mut self, db: &str, mut d: Document) -> Result<Document> {
        d.insert("$db", db);
        let out = self.raw_command(&raw(&d), &[])?;
        Error::from_response(&raw(&out))?;
        Ok(out)
    }
}
fn network(e: std::io::Error) -> Error {
    Error::new(ErrorKind::Network, e.to_string())
}
