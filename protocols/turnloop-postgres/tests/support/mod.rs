use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

pub enum Transport {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}
impl Transport {
    pub fn connect(port: u16) -> Self {
        let stream =
            TcpStream::connect(("127.0.0.1", port)).expect("fixture operation must succeed");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("fixture operation must succeed");
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .expect("fixture operation must succeed");
        Self::Plain(stream)
    }
    pub fn upgrade(&mut self) {
        let Self::Plain(socket) = self else {
            panic!("duplicate TLS upgrade")
        };
        let mut roots = RootCertStore::empty();
        roots
            .add(
                fs::read(tools().join("server.der"))
                    .expect("fixture operation must succeed")
                    .into(),
            )
            .expect("fixture operation must succeed");
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let session = ClientConnection::new(
            Arc::new(config),
            ServerName::try_from("localhost").expect("fixture operation must succeed"),
        )
        .expect("fixture operation must succeed");
        let mut stream = StreamOwned::new(
            session,
            socket.try_clone().expect("fixture operation must succeed"),
        );
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .expect("fixture operation must succeed");
        }
        *self = Self::Tls(Box::new(stream));
    }
}
impl Read for Transport {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(b),
            Self::Tls(s) => s.read(b),
        }
    }
}
impl Write for Transport {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(b),
            Self::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
        }
    }
}
pub fn tools() -> PathBuf {
    std::env::var_os("TURNLOOP_TEST_SQL_TOOLS")
        .expect("run via scripts/test-servers.py run cargo test --workspace -- --ignored")
        .into()
}
