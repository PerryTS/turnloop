//! Backend-neutral identifiers, errors, deadlines and socket options.
use crate::Instant;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Token(pub u64);

macro_rules! id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name {
            pub(crate) owner: u64,
            pub(crate) key: u64,
        }
        impl $name {
            /// Slot index, for preallocated backend operation/resource storage.
            pub fn index(self) -> usize {
                self.key as u32 as usize
            }
            /// Generation and index. Unique within a loop until u32 generation exhaustion.
            pub fn key(self) -> u64 {
                self.key
            }
            /// Unique identity of the creating loop. Never route on the token alone.
            pub fn owner(self) -> u64 {
                self.owner
            }
        }
    };
}
id!(Handle);
id!(OpId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Cancelled,
    Unsupported,
    InvalidInput,
    NotFound,
    WouldBlock,
    TimedOut,
    ConnectionRefused,
    ConnectionReset,
    BrokenPipe,
    ResourceLimit,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub os: Option<i32>,
}
impl Error {
    pub const fn new(kind: ErrorKind) -> Self {
        Self { kind, os: None }
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as I;
        let kind = match e.kind() {
            I::Unsupported => ErrorKind::Unsupported,
            I::InvalidInput => ErrorKind::InvalidInput,
            I::NotFound => ErrorKind::NotFound,
            I::WouldBlock => ErrorKind::WouldBlock,
            I::TimedOut => ErrorKind::TimedOut,
            I::ConnectionRefused => ErrorKind::ConnectionRefused,
            I::ConnectionReset | I::ConnectionAborted => ErrorKind::ConnectionReset,
            I::BrokenPipe => ErrorKind::BrokenPipe,
            I::OutOfMemory => ErrorKind::ResourceLimit,
            _ => ErrorKind::Other,
        };
        Self {
            kind,
            os: e.raw_os_error(),
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} (OS {:?})", self.kind, self.os)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug)]
pub enum Timeout {
    Now,
    After(Duration),
    Until(Instant),
    Forever,
}
impl Timeout {
    /// Absolute deadline used once at turn entry (retries never extend the budget).
    pub fn deadline(self, now: Instant) -> Option<Instant> {
        match self {
            Self::Now => Some(now),
            Self::After(d) => now.checked_add(d),
            Self::Until(t) => Some(t),
            Self::Forever => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub max_handles: usize,
    pub max_operations: usize,
    pub events_per_turn: usize,
    pub pooled_buffers: usize,
    pub pooled_buffer_size: usize,
    pub post_capacity: usize,
    /// First pool submission fixes the process-wide configuration. Later loops
    /// must use the same configuration; mismatches are reported as InvalidInput.
    pub blocking_pool: crate::PoolConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max_handles: 1024,
            max_operations: 4096,
            events_per_turn: 256,
            pooled_buffers: 256,
            pooled_buffer_size: 16 * 1024,
            post_capacity: 1024,
            blocking_pool: crate::PoolConfig::default(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct TcpOpts {
    pub nodelay: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct ListenOpts {
    pub reuse_port: bool,
    pub backlog: u32,
}
impl Default for ListenOpts {
    fn default() -> Self {
        Self {
            reuse_port: false,
            backlog: 128,
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct UdpOpts {
    pub reuse_port: bool,
}

/// The fd/event is borrowed from the driver; it must never be closed by the host.
#[derive(Clone, Copy, Debug)]
pub enum Integration {
    Fd(i32),
    /// Native waitable event, represented as bits without imposing Windows types.
    Event(usize),
    HostCallback,
    RuntimeOwned,
}

#[derive(Debug)]
pub enum Open {
    Tcp { addr: SocketAddr, opts: TcpOpts },
    Listener { addr: SocketAddr, opts: ListenOpts },
    Udp { addr: SocketAddr, opts: UdpOpts },
}
