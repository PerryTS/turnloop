//! Backend-neutral identifiers, errors, deadlines and socket options.
use crate::Instant;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
/// Opaque host routing value, returned unchanged with its completions.
pub struct Token(pub u64);

macro_rules! id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        /// Loop-scoped generational identity; stale or foreign IDs never address a reused resource.
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
/// Portable error categories independent of native numeric error codes.
pub enum ErrorKind {
    /// The operation was cancelled and its native buffer access has ended.
    Cancelled,
    /// This platform or resource does not support the requested capability.
    Unsupported,
    /// An argument or resource state is invalid for the requested operation.
    InvalidInput,
    /// The requested identity or resource does not exist or is no longer owned.
    NotFound,
    /// Progress requires a later turn or release of outstanding capacity.
    WouldBlock,
    /// The requested deadline expired.
    TimedOut,
    /// The remote endpoint refused the connection.
    ConnectionRefused,
    /// The established connection was reset or aborted.
    ConnectionReset,
    /// The write side has no reader or the connection cannot accept writes.
    BrokenPipe,
    /// A configured capacity or native resource limit was reached.
    ResourceLimit,
    /// An error without a more specific portable category.
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Portable error kind with an optional original operating-system code.
pub struct Error {
    /// Portable error category.
    pub kind: ErrorKind,
    /// Original OS error code, when one exists.
    pub os: Option<i32>,
}
impl Error {
    /// Create a portable error without an OS code.
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
/// A fallible turnloop operation preserving portable and native error information.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug)]
/// Exact timeout budget for one host-driven turn.
pub enum Timeout {
    /// Collect immediately available work without blocking.
    Now,
    /// Wait at most this duration measured from turn entry.
    After(Duration),
    /// Wait until this absolute backend-clock deadline.
    Until(Instant),
    /// Wait without a host deadline, still bounded by pending timers and notifications.
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
/// Fixed loop capacities and shared blocking-pool configuration.
pub struct Config {
    /// Maximum simultaneously allocated handles, including closed results awaiting delivery.
    pub max_handles: usize,
    /// Maximum outstanding operations, including terminal results awaiting delivery.
    pub max_operations: usize,
    /// Native event budget and per-source completion reserve.
    pub events_per_turn: usize,
    /// Number of reusable pooled read buffers.
    pub pooled_buffers: usize,
    /// Bytes retained in each pooled read buffer.
    pub pooled_buffer_size: usize,
    /// Maximum queued cross-thread posts, rounded up to a power of two.
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
/// TCP connection options applied when creating the socket.
pub struct TcpOpts {
    /// Disable the TCP Nagle algorithm for latency-sensitive small writes.
    pub nodelay: bool,
}
#[derive(Clone, Copy, Debug)]
/// Listener backlog and optional kernel reuse-port configuration.
pub struct ListenOpts {
    /// Enable SO_REUSEPORT when supported; macOS does not promise balanced accepts.
    pub reuse_port: bool,
    /// Maximum pending connection backlog requested from the OS.
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
/// UDP binding options.
pub struct UdpOpts {
    /// Enable SO_REUSEPORT when supported; macOS does not promise balanced accepts.
    pub reuse_port: bool,
}

/// The fd/event is borrowed from the driver; it must never be closed by the host.
#[derive(Clone, Copy, Debug)]
pub enum Integration {
    /// Borrowed native poller descriptor for external host waiting.
    Fd(i32),
    /// Native waitable event, represented as bits without imposing Windows types.
    Event(usize),
    /// The host schedules nonblocking turns when work arrives.
    HostCallback,
    /// The component runtime owns scheduling and the wait primitive.
    RuntimeOwned,
}

#[derive(Debug)]
/// Backend-neutral resource creation requests; unsupported kinds must fail explicitly.
pub enum Open {
    /// Unconnected local stream.
    Pipe(crate::PipeName),
    /// Local stream listener.
    PipeListener {
        /// Local IPC address in the platform namespace.
        name: crate::PipeName,
        /// Options controlling resource creation.
        opts: ListenOpts,
    },
    /// Duplicate and classify a host standard stream.
    Stdio(crate::Stdio),
    /// Create an unconnected TCP socket for a subsequent Connect operation.
    Tcp {
        /// IP address and port used to bind or connect this resource.
        addr: SocketAddr,
        /// Options controlling resource creation.
        opts: TcpOpts,
    },
    /// Create and bind a listening TCP socket.
    Listener {
        /// IP address and port used to bind or connect this resource.
        addr: SocketAddr,
        /// Options controlling resource creation.
        opts: ListenOpts,
    },
    /// Create and bind a UDP socket.
    Udp {
        /// IP address and port used to bind or connect this resource.
        addr: SocketAddr,
        /// Options controlling resource creation.
        opts: UdpOpts,
    },
}
