//! Host-driven fetch transport policy. DNS, environment, sockets and time are inputs.
//! JS body conversions, promise dispatch and abort reason objects remain in Perry.
use crate::{
    Error, Result,
    http1::{Head, Header},
};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};
use url::Url;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Http1,
    Http2,
}
pub fn from_alpn(alpn: Option<&[u8]>) -> Result<Protocol> {
    match alpn {
        Some(b"h2") => Ok(Protocol::Http2),
        None | Some(b"http/1.1") => Ok(Protocol::Http1),
        _ => Err(Error::new("UND_ERR_NOT_SUPPORTED", "unsupported ALPN")),
    }
}
/// Resolver implementations enqueue host operations; they must not block here.
pub trait Resolver {
    type Token: Copy;
    fn resolve(&mut self, hostname: &str, port: u16) -> Result<Self::Token>;
    fn poll(&mut self, token: Self::Token, addresses: &mut Vec<IpAddr>) -> Option<Result<()>>;
    fn cancel(&mut self, token: Self::Token);
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    Follow,
    Manual,
    Error,
}
#[derive(Debug, Clone)]
pub struct Request {
    pub url: Url,
    pub method: String,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    pub replayable: bool,
    pub redirects: usize,
}
impl Request {
    pub fn new(url: &str, method: &str) -> Result<Self> {
        let url = Url::parse(url).map_err(|_| Error::new("ERR_INVALID_URL", "invalid URL"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(Error::new("ERR_INVALID_URL", "unsupported URL scheme"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Error::new(
                "UND_ERR_INVALID_ARG",
                "URL contains credentials",
            ));
        }
        http::Method::from_bytes(method.as_bytes())
            .map_err(|_| Error::new("UND_ERR_INVALID_ARG", "invalid method"))?;
        if ["CONNECT", "TRACE", "TRACK"]
            .iter()
            .any(|m| method.eq_ignore_ascii_case(m))
        {
            return Err(Error::new("UND_ERR_INVALID_ARG", "forbidden fetch method"));
        }
        let method = if ["DELETE", "GET", "HEAD", "OPTIONS", "POST", "PUT"]
            .iter()
            .any(|m| method.eq_ignore_ascii_case(m))
        {
            method.to_ascii_uppercase()
        } else {
            method.into()
        };
        Ok(Self {
            url,
            method,
            headers: Vec::new(),
            body: Vec::new(),
            replayable: true,
            redirects: 0,
        })
    }
    pub fn head(&self, absolute_form: bool) -> Head {
        let mut url = self.url.clone();
        url.set_fragment(None);
        let target = if absolute_form {
            url.as_str().to_owned()
        } else {
            let mut target = url.path().to_string();
            if let Some(q) = url.query() {
                target.push('?');
                target.push_str(q);
            }
            target
        };
        let mut headers = self.headers.clone();
        headers.retain(|h| h.name != "host");
        headers.push(Header::new("host", authority(&url)));
        Head {
            method: self.method.clone(),
            target,
            status: 0,
            version: 1,
            headers,
            keep_alive: true,
        }
    }
    /// `true` means resend the modified request; `false` exposes the response as-is.
    pub fn redirect(
        &mut self,
        status: u16,
        location: Option<&str>,
        mode: RedirectMode,
        max: usize,
    ) -> Result<bool> {
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return Ok(false);
        }
        if mode == RedirectMode::Manual {
            return Ok(false);
        }
        if mode == RedirectMode::Error {
            return Err(Error::new("UND_ERR_REQ_RETRY", "redirect mode is error"));
        }
        let Some(location) = location else {
            return Ok(false);
        };
        if self.redirects >= max {
            return Err(Error::new("UND_ERR_REDIRECT", "redirect count exceeded"));
        }
        let next = self
            .url
            .join(location)
            .map_err(|_| Error::new("ERR_INVALID_URL", "invalid redirect URL"))?;
        if !matches!(next.scheme(), "http" | "https")
            || !next.username().is_empty()
            || next.password().is_some()
        {
            return Err(Error::new("UND_ERR_INVALID_ARG", "invalid redirect target"));
        }
        let rewrite = (matches!(status, 301 | 302) && self.method == "POST")
            || (status == 303 && self.method != "GET" && self.method != "HEAD");
        if !rewrite && !self.replayable {
            return Err(Error::new(
                "UND_ERR_REQ_RETRY",
                "streaming body cannot be replayed",
            ));
        }
        if self.url.origin() != next.origin() {
            self.headers.retain(|h| {
                !matches!(
                    h.name.as_str(),
                    "authorization" | "proxy-authorization" | "cookie" | "cookie2" | "host"
                )
            });
        }
        if rewrite {
            self.method = "GET".into();
            self.body.clear();
            self.replayable = true;
            self.headers.retain(|h| {
                !matches!(
                    h.name.as_str(),
                    "content-encoding"
                        | "content-language"
                        | "content-location"
                        | "content-type"
                        | "content-length"
                        | "transfer-encoding"
                )
            });
        }
        self.url = next;
        self.redirects += 1;
        Ok(true)
    }
}
fn authority(url: &Url) -> String {
    let mut result = url.host_str().unwrap_or("").to_string();
    if let Some(port) = url.port() {
        result.push(':');
        result.push_str(&port.to_string());
    }
    result
}
#[derive(Debug, Default, Clone)]
pub struct ProxyEnvironment {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: String,
}
impl ProxyEnvironment {
    /// Values are supplied by the host from HTTP_PROXY/HTTPS_PROXY/NO_PROXY (or lowercase overrides).
    pub fn proxy_for(&self, url: &Url) -> Result<Option<Url>> {
        if bypass(url, &self.no_proxy) {
            return Ok(None);
        }
        let proxy = if url.scheme() == "https" {
            self.https_proxy.as_deref()
        } else {
            self.http_proxy.as_deref()
        };
        proxy
            .filter(|p| !p.is_empty())
            .map(|p| {
                let parsed =
                    Url::parse(p).map_err(|_| Error::new("ERR_INVALID_URL", "invalid proxy"))?;
                if parsed.scheme() != "http" || parsed.host_str().is_none() {
                    return Err(Error::new(
                        "UND_ERR_NOT_SUPPORTED",
                        "only HTTP proxies are supported",
                    ));
                }
                Ok(parsed)
            })
            .transpose()
    }
}
fn bypass(url: &Url, list: &str) -> bool {
    let host = url.host_str().unwrap_or("");
    list.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .any(|entry| {
            if entry == "*" {
                return true;
            }
            let (domain, port) = if let Some((h, p)) = entry.rsplit_once(':') {
                if let Ok(p) = p.parse::<u16>() {
                    (h, Some(p))
                } else {
                    (entry, None)
                }
            } else {
                (entry, None)
            };
            if port.is_some() && port != url.port_or_known_default() {
                return false;
            }
            let domain = domain.trim_start_matches("*.").trim_start_matches('.');
            host.eq_ignore_ascii_case(domain)
                || (host.len() > domain.len()
                    && host.as_bytes()[host.len() - domain.len() - 1] == b'.'
                    && host[host.len() - domain.len()..].eq_ignore_ascii_case(domain))
        })
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolKey {
    pub origin: String,
    pub proxy: Option<String>,
}
impl PoolKey {
    pub fn new(url: &Url, proxy: Option<&Url>) -> Self {
        Self {
            origin: url.origin().ascii_serialization(),
            proxy: proxy.map(ToString::to_string),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquire {
    Connect(ConnectionId),
    Reuse(ConnectionId),
    Wait,
}
struct Slot {
    id: ConnectionId,
    key: PoolKey,
    active: usize,
    capacity: usize,
    connecting: bool,
    deadline: Option<Instant>,
    closed: bool,
}
pub struct Pool {
    slots: Vec<Slot>,
    max_per_host: usize,
    idle: Duration,
    next: u64,
}
impl Pool {
    pub fn new(max_per_host: usize, idle: Duration) -> Self {
        Self {
            slots: Vec::new(),
            max_per_host,
            idle,
            next: 1,
        }
    }
    /// A Connect reservation counts immediately, so concurrent callers cannot overbook.
    pub fn acquire(&mut self, key: &PoolKey, now: Instant) -> Acquire {
        if let Some(s) = self.slots.iter_mut().find(|s| {
            !s.closed
                && s.key == *key
                && !s.connecting
                && s.active < s.capacity
                && s.deadline.is_none_or(|d| d > now)
        }) {
            s.active += 1;
            s.deadline = None;
            return Acquire::Reuse(s.id);
        }
        if self
            .slots
            .iter()
            .filter(|s| !s.closed && s.key == *key)
            .count()
            >= self.max_per_host
        {
            return Acquire::Wait;
        }
        let id = ConnectionId(self.next);
        self.next += 1;
        let slot = Slot {
            id,
            key: key.clone(),
            active: 1,
            capacity: 1,
            connecting: true,
            deadline: None,
            closed: false,
        };
        if let Some(s) = self.slots.iter_mut().find(|s| s.closed) {
            *s = slot;
        } else {
            self.slots.push(slot);
        }
        Acquire::Connect(id)
    }
    fn slot(&mut self, id: ConnectionId) -> Result<&mut Slot> {
        self.slots
            .iter_mut()
            .find(|s| s.id == id && !s.closed)
            .ok_or(Error::new("UND_ERR_CLOSED", "unknown connection"))
    }
    pub fn connected(
        &mut self,
        id: ConnectionId,
        protocol: Protocol,
        max_streams: usize,
    ) -> Result<()> {
        let s = self.slot(id)?;
        s.connecting = false;
        s.capacity = if protocol == Protocol::Http1 {
            1
        } else {
            max_streams
        };
        Ok(())
    }
    pub fn release(&mut self, id: ConnectionId, reusable: bool, now: Instant) -> Result<()> {
        let idle = self.idle;
        let s = self.slot(id)?;
        if s.active == 0 {
            return Err(Error::new("UND_ERR_INVALID_ARG", "duplicate pool release"));
        }
        s.active -= 1;
        if !reusable {
            s.capacity = 0;
        }
        if s.active == 0 {
            s.deadline = Some(if s.capacity == 0 { now } else { now + idle });
        }
        Ok(())
    }
    pub fn closed(&mut self, id: ConnectionId) -> Result<()> {
        self.slot(id)?.closed = true;
        Ok(())
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter(|s| !s.closed)
            .filter_map(|s| s.deadline)
            .min()
    }
    /// Repeatedly call to obtain all Close requests at this time. No clock or timer is owned.
    pub fn handle_timeout(&mut self, now: Instant) -> Option<ConnectionId> {
        let s = self
            .slots
            .iter_mut()
            .find(|s| !s.closed && s.deadline.is_some_and(|d| d <= now))?;
        s.closed = true;
        Some(s.id)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Connect,
    Headers,
    Body,
    Done,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Success,
    Error(Error),
}
pub struct Lifecycle {
    phase: Phase,
    deadline: Option<Instant>,
    terminal: Option<Completion>,
    delivered: bool,
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            phase: Phase::Connect,
            deadline: None,
            terminal: None,
            delivered: false,
        }
    }
}
impl Lifecycle {
    pub fn transition(&mut self, phase: Phase, deadline: Option<Instant>) {
        if self.terminal.is_none() {
            self.phase = phase;
            self.deadline = deadline;
        }
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.deadline.is_some_and(|d| d <= now) {
            let (code, message) = match self.phase {
                Phase::Connect => ("UND_ERR_CONNECT_TIMEOUT", "Connect Timeout Error"),
                Phase::Headers => ("UND_ERR_HEADERS_TIMEOUT", "Headers Timeout Error"),
                _ => ("UND_ERR_BODY_TIMEOUT", "Body Timeout Error"),
            };
            self.finish(Completion::Error(Error::new(code, message)));
        }
    }
    pub fn abort(&mut self) {
        self.finish(Completion::Error(Error::new(
            "UND_ERR_ABORTED",
            "Request aborted",
        )));
    }
    pub fn finish(&mut self, result: Completion) {
        if self.terminal.is_none() {
            self.terminal = Some(result);
            self.phase = Phase::Done;
            self.deadline = None;
        }
    }
    pub fn poll(&mut self) -> Option<Completion> {
        if self.delivered {
            return None;
        }
        let event = self.terminal?;
        self.delivered = true;
        Some(event)
    }
}
/// After a CONNECT 2xx upgrade, the host feeds remaining bytes into TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportRequest {
    Resolve { hostname: String, port: u16 },
    UpgradeTls { server_name: String },
    Ready(Protocol),
}
pub struct Route {
    target: Url,
    proxy: Option<Url>,
    tunnel: bool,
}
impl Route {
    pub fn new(target: Url, proxy: Option<Url>) -> Self {
        Self {
            target,
            proxy,
            tunnel: false,
        }
    }
    pub fn resolve(&self) -> TransportRequest {
        let peer = self.proxy.as_ref().unwrap_or(&self.target);
        TransportRequest::Resolve {
            hostname: peer.host_str().unwrap_or("").into(),
            port: peer.port_or_known_default().unwrap_or(80),
        }
    }
    pub fn connect_head(&self, authorization: Option<&[u8]>) -> Option<Head> {
        if self.proxy.is_none() || self.target.scheme() != "https" {
            return None;
        }
        let target = format!(
            "{}:{}",
            self.target.host_str().unwrap_or(""),
            self.target.port_or_known_default().unwrap_or(443)
        );
        let mut headers = vec![Header::new("host", &target)];
        if let Some(value) = authorization {
            headers.push(Header::new("proxy-authorization", value));
        }
        Some(Head {
            method: "CONNECT".into(),
            target,
            status: 0,
            version: 1,
            headers,
            keep_alive: true,
        })
    }
    pub fn tunnel_response(&mut self, status: u16) -> Result<TransportRequest> {
        if !(200..300).contains(&status) {
            return Err(Error::new("UND_ERR_PRX_TLS", "proxy CONNECT rejected"));
        }
        self.tunnel = true;
        Ok(TransportRequest::UpgradeTls {
            server_name: self.target.host_str().unwrap_or("").into(),
        })
    }
    pub fn connected(&self) -> Option<TransportRequest> {
        if self.target.scheme() == "https" {
            if self.proxy.is_some() && !self.tunnel {
                None
            } else {
                Some(TransportRequest::UpgradeTls {
                    server_name: self.target.host_str().unwrap_or("").into(),
                })
            }
        } else {
            Some(TransportRequest::Ready(Protocol::Http1))
        }
    }
}
pub fn transport_error(kind: std::io::ErrorKind) -> Error {
    match kind {
        std::io::ErrorKind::ConnectionRefused => Error::new("ECONNREFUSED", "connect ECONNREFUSED"),
        std::io::ErrorKind::ConnectionReset => Error::new("ECONNRESET", "read ECONNRESET"),
        std::io::ErrorKind::NotFound => Error::new("ENOTFOUND", "getaddrinfo ENOTFOUND"),
        std::io::ErrorKind::TimedOut => Error::new("ETIMEDOUT", "connect ETIMEDOUT"),
        _ => Error::new("UND_ERR_SOCKET", "socket error"),
    }
}
