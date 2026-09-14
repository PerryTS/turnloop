//! TLS record processing with caller-owned encrypted buffers and caller-supplied time.
//!
//! Handle every returned rustls state before calling `process` again, discard
//! `status.discard` input bytes only after handling it, and acknowledge
//! `TransmitTlsData` only after the host has completed all writes. Plaintext
//! records borrow rustls storage; consume them before advancing the state.
#![deny(unsafe_op_in_unsafe_fn)]

use rustls::{
    CertificateError, DigitallySignedStruct, Error, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub use rustls;
pub use rustls::unbuffered::{ConnectionState, UnbufferedStatus};

/// Wall time supplied by the host, never sampled from an operating-system clock.
#[derive(Debug)]
struct SuppliedTime(AtomicU64);
impl rustls::time_provider::TimeProvider for SuppliedTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            self.0.load(Ordering::Relaxed),
        )))
    }
}

/// Node's explicit `ca` replaces default roots. Extra CA PEM augments Mozilla
/// roots only when explicit CA is absent. The host reads NODE_EXTRA_CA_CERTS
/// once at startup and supplies its contents; this crate never reads files/env.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub alpn: Vec<Vec<u8>>,
    pub ca: Option<Vec<CertificateDer<'static>>>,
    pub extra_ca_pem: Vec<u8>,
    pub reject_unauthorized: bool,
    pub enable_sni: bool,
}
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            alpn: vec![b"http/1.1".to_vec()],
            ca: None,
            extra_ca_pem: Vec::new(),
            reject_unauthorized: true,
            enable_sni: true,
        }
    }
}

#[derive(Clone)]
pub struct ClientConfig {
    config: Arc<rustls::ClientConfig>,
    time: Arc<SuppliedTime>,
}
impl ClientConfig {
    pub fn new(options: ClientOptions, unix_seconds: u64) -> Result<Self, Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let time = Arc::new(SuppliedTime(AtomicU64::new(unix_seconds)));
        let mut roots = rustls::RootCertStore::empty();
        if let Some(ca) = options.ca {
            for cert in ca {
                roots.add(cert)?;
            }
        } else {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            for cert in CertificateDer::pem_slice_iter(&options.extra_ca_pem) {
                roots.add(cert.map_err(|e| Error::General(e.to_string()))?)?;
            }
        }
        let mut config = rustls::ClientConfig::builder_with_details(provider.clone(), time.clone())
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        if !options.reject_unauthorized {
            config
                .dangerous()
                .set_certificate_verifier(Arc::new(Unverified { provider }));
        }
        config.alpn_protocols = options.alpn;
        config.enable_sni = options.enable_sni;
        Ok(Self {
            config: Arc::new(config),
            time,
        })
    }
    /// Reuse this configuration across connections to share the session cache.
    pub fn connect(&self, name: ServerName<'static>) -> Result<Client, Error> {
        Ok(Client {
            inner: rustls::client::UnbufferedClientConnection::new(self.config.clone(), name)?,
            time: self.time.clone(),
            deadline: None,
            timed_out: false,
        })
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    config: Arc<rustls::ServerConfig>,
    time: Arc<SuppliedTime>,
}
impl ServerConfig {
    pub fn new(
        chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
        alpn: Vec<Vec<u8>>,
        unix_seconds: u64,
    ) -> Result<Self, Error> {
        let time = Arc::new(SuppliedTime(AtomicU64::new(unix_seconds)));
        let mut config = rustls::ServerConfig::builder_with_details(
            Arc::new(rustls::crypto::ring::default_provider()),
            time.clone(),
        )
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
        config.alpn_protocols = alpn;
        // Rustls's default stateful session cache avoids a clock-reading ticketer.
        Ok(Self {
            config: Arc::new(config),
            time,
        })
    }
    pub fn accept(&self) -> Result<Server, Error> {
        Ok(Server {
            inner: rustls::server::UnbufferedServerConnection::new(self.config.clone())?,
            time: self.time.clone(),
            deadline: None,
            timed_out: false,
        })
    }
}

macro_rules! endpoint {
    ($name:ident, $connection:ty, $data:ty) => {
        pub struct $name {
            inner: $connection,
            time: Arc<SuppliedTime>,
            deadline: Option<Instant>,
            timed_out: bool,
        }
        impl $name {
            /// No input/output staging copies are introduced by this wrapper.
            pub fn process<'c, 'i>(
                &'c mut self,
                incoming: &'i mut [u8],
                unix_seconds: u64,
            ) -> UnbufferedStatus<'c, 'i, $data> {
                self.time.0.store(unix_seconds, Ordering::Relaxed);
                if self.timed_out {
                    return UnbufferedStatus {
                        discard: 0,
                        state: Err(Error::General("TLS handshake timeout".into())),
                    };
                }
                self.inner.process_tls_records(incoming)
            }
            pub fn alpn_protocol(&self) -> Option<&[u8]> {
                self.inner.alpn_protocol()
            }
            pub fn is_handshaking(&self) -> bool {
                self.inner.is_handshaking()
            }
            pub fn handshake_kind(&self) -> Option<rustls::HandshakeKind> {
                self.inner.handshake_kind()
            }
            pub fn set_handshake_deadline(&mut self, deadline: Option<Instant>) {
                self.deadline = deadline;
            }
            pub fn next_timeout(&self) -> Option<Instant> {
                if self.inner.is_handshaking() && !self.timed_out {
                    self.deadline
                } else {
                    None
                }
            }
            /// Returns a terminal timeout event exactly once.
            pub fn handle_timeout(&mut self, now: Instant) -> Option<&'static str> {
                if self.next_timeout().is_some_and(|d| now >= d) {
                    self.timed_out = true;
                    self.deadline = None;
                    Some("ERR_TLS_HANDSHAKE_TIMEOUT")
                } else {
                    None
                }
            }
        }
    };
}
endpoint!(
    Client,
    rustls::client::UnbufferedClientConnection,
    rustls::client::ClientConnectionData
);
endpoint!(
    Server,
    rustls::server::UnbufferedServerConnection,
    rustls::server::ServerConnectionData
);

/// Stable Node-style cause code. Rustls cannot distinguish a missing intermediate
/// from an untrusted root; both map to UNABLE_TO_VERIFY_LEAF_SIGNATURE.
pub fn node_error_code(error: &Error) -> &'static str {
    match error {
        Error::InvalidCertificate(
            CertificateError::Expired | CertificateError::ExpiredContext { .. },
        ) => "CERT_HAS_EXPIRED",
        Error::InvalidCertificate(
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. },
        ) => "CERT_NOT_YET_VALID",
        Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
        ) => "ERR_TLS_CERT_ALTNAME_INVALID",
        Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
            "UNABLE_TO_VERIFY_LEAF_SIGNATURE"
        }
        Error::InvalidCertificate(CertificateError::Revoked) => "CERT_REVOKED",
        Error::NoApplicationProtocol => "ERR_SSL_TLSV1_ALERT_NO_APPLICATION_PROTOCOL",
        _ => "ERR_SSL_PROTOCOL_ERROR",
    }
}

#[derive(Debug)]
struct Unverified {
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }
    // rejectUnauthorized disables trust/name validation, not proof of possession.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(feature = "turnloop")]
pub mod asynchronous;
#[cfg(feature = "turnloop")]
pub use asynchronous::TlsStream;
