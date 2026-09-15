//! Backend-neutral identifiers, errors, deadlines and socket options.
use crate::Instant;
use std::net::{IpAddr, SocketAddr};
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
    /// Permissions or a capability boundary denied a filesystem operation.
    PermissionDenied,
    /// The target already exists (for example an exclusive create).
    AlreadyExists,
    /// A path component that must be a directory is not one.
    NotADirectory,
    /// A file operation targeted a directory.
    IsADirectory,
    /// A directory to be removed or replaced is not empty.
    DirectoryNotEmpty,
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
            I::PermissionDenied => ErrorKind::PermissionDenied,
            I::AlreadyExists => ErrorKind::AlreadyExists,
            I::NotADirectory => ErrorKind::NotADirectory,
            I::IsADirectory => ErrorKind::IsADirectory,
            I::DirectoryNotEmpty => ErrorKind::DirectoryNotEmpty,
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
/// TCP connection options applied when creating the socket. Anything a host
/// needs to change later goes through `Loop::set_option` and
/// [`SocketOption`] instead.
pub struct TcpOpts {
    /// Disable the TCP Nagle algorithm for latency-sensitive small writes.
    pub nodelay: bool,
}
#[derive(Clone, Copy, Debug)]
/// Listener backlog, kernel reuse-port configuration and accepted-socket defaults.
///
/// Address reuse is bind-time only and stays here rather than in
/// [`SocketOption`]: `SO_REUSEPORT` is `reuse_port`, and `SO_REUSEADDR` is applied
/// by the backend to every TCP listener it binds (TIME_WAIT rebinding). Neither
/// can be changed on a socket that is already bound, so neither is an option.
pub struct ListenOpts {
    /// Enable SO_REUSEPORT when supported; macOS does not promise balanced accepts.
    pub reuse_port: bool,
    /// Maximum pending connection backlog requested from the OS.
    pub backlog: u32,
    /// Options applied to every connection this listener accepts.
    pub accept_defaults: AcceptDefaults,
}
impl Default for ListenOpts {
    fn default() -> Self {
        Self {
            reuse_port: false,
            backlog: 128,
            accept_defaults: AcceptDefaults::EMPTY,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Options a listener applies to every connection it accepts.
///
/// **When they are applied:** by the accepting loop, on the accepted socket,
/// after the OS accept succeeds and *before* the `Accepted` completion reaches
/// the host. A host therefore never observes an unconfigured connection, and
/// never needs a `set_option` round trip per connection.
///
/// **What "cheaply" means:** each field costs at most one `setsockopt` on the new
/// socket. Anything that would need a syscall per accepted byte, a kernel query
/// or a second handle is not a default and belongs in [`Loop::set_option`].
///
/// A backend that cannot apply a requested default rejects the *listener* when it
/// is created, rather than ignoring the request once per connection. If the OS
/// rejects a default while applying it to a live accepted socket, that accept
/// operation fails with the OS error and the connection is closed.
///
/// [`Loop::set_option`]: crate::Driver::set_option
pub struct AcceptDefaults {
    /// Disable Nagle coalescing on each accepted connection (`TCP_NODELAY`).
    pub nodelay: bool,
    /// Enable keep-alive probes on each accepted connection, with this schedule.
    pub keep_alive: Option<KeepAlive>,
}
impl AcceptDefaults {
    /// Apply nothing: every accepted socket keeps the platform defaults.
    pub const EMPTY: Self = Self {
        nodelay: false,
        keep_alive: None,
    };
    /// Whether any default would need to be applied to an accepted socket.
    pub const fn is_empty(self) -> bool {
        !self.nodelay && self.keep_alive.is_none()
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// TCP keep-alive probe schedule. Each `None` keeps the platform default.
///
/// Native platforms express the schedule in whole seconds, so a duration is
/// rounded **up** to the next whole second and a zero duration is rejected as
/// `InvalidInput`. WASI takes the duration unrounded.
pub struct KeepAlive {
    /// Connection idle time before the first probe is sent.
    pub idle: Option<Duration>,
    /// Interval between probes once probing has started.
    pub interval: Option<Duration>,
    /// Unanswered probes before the connection is dropped.
    pub count: Option<u32>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A multicast group and the local interface that should carry it.
pub struct MulticastGroup {
    /// Group address. Its family must match the socket's own family.
    pub group: IpAddr,
    /// Local interface index, or zero for the kernel's default interface.
    ///
    /// An IPv6 group accepts any index on every native platform. An IPv4 group
    /// accepts a nonzero index only on Linux (`ip_mreqn`); elsewhere a nonzero
    /// index is reported `Unsupported` rather than silently ignored.
    pub interface: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// A socket option that can be changed after the socket exists.
///
/// Bind-time-only options are deliberately absent: `SO_REUSEPORT`/`SO_REUSEADDR`
/// live in [`ListenOpts`] and [`UdpOpts`], because the OS accepts them only on an
/// unbound socket. [`Ipv6Only`](Self::Ipv6Only) is the borderline case: it is
/// kept here so it can be *read* on any socket, but every platform rejects
/// setting it after bind, and turnloop binds listeners and UDP sockets when they
/// are created.
///
/// A backend with no equivalent for a variant reports `Unsupported`. It never
/// accepts the call and ignores it.
pub enum SocketOption {
    /// `TCP_NODELAY`: send small writes immediately instead of coalescing them.
    NoDelay(bool),
    /// `SO_KEEPALIVE` and its probe schedule; `None` disables probing.
    ///
    /// The switch and each schedule value are separate kernel settings, and every
    /// value is validated before any of them is written. If the OS still rejects
    /// one after the switch was set, the error is reported and the socket keeps
    /// whatever the OS left: read it back rather than assuming a rollback.
    /// Disabling clears the switch and leaves the schedule alone, as the OS does.
    KeepAlive(Option<KeepAlive>),
    /// `SO_LINGER`: `Some(d)` blocks the close until queued data is delivered or
    /// `d` elapses, and `Some(Duration::ZERO)` discards it and resets the
    /// connection. `None` restores the platform default (a graceful background
    /// close). The duration has one-second granularity, rounded up.
    Linger(Option<Duration>),
    /// `SO_RCVBUF`: requested receive-buffer bytes.
    ///
    /// **The final size is the kernel's choice, not the request.** The contract
    /// is only that the socket ends up with *at least* what was asked for, so
    /// read it back rather than assuming an exact value:
    ///
    /// * **Linux** doubles the request and clamps it to `net.core.rmem_max`
    ///   (asking 262144 on a stock kernel reports 524288).
    /// * **macOS/BSD** normally keep the request exactly, but start much higher
    ///   than Linux: an accepted loopback socket defaults to around 408300 bytes.
    /// * **Windows** rounds up to its own granularity and may keep an auto-tuned
    ///   receive window that is larger than the request.
    /// * **WASI** forwards to the host socket, so it inherits that host's policy.
    RecvBufferSize(u32),
    /// `SO_SNDBUF`: requested send-buffer bytes. The final size is the kernel's
    /// choice with the same per-platform rounding as
    /// [`RecvBufferSize`](Self::RecvBufferSize); read it back.
    SendBufferSize(u32),
    /// `IP_TTL` / `IPV6_UNICAST_HOPS`: hop limit for outgoing unicast packets.
    Ttl(u32),
    /// `IPV6_V6ONLY`: refuse IPv4-mapped peers on an IPv6 socket. Bind-time on
    /// every platform, so setting it on a bound socket fails.
    Ipv6Only(bool),
    /// `SO_BROADCAST`: permit datagrams addressed to a broadcast address.
    Broadcast(bool),
    /// `IP_MULTICAST_TTL` / `IPV6_MULTICAST_HOPS`: outgoing multicast hop limit.
    MulticastTtl(u32),
    /// `IP_MULTICAST_LOOP` / `IPV6_MULTICAST_LOOP`: deliver this socket's own
    /// multicast sends back to local members.
    MulticastLoop(bool),
    /// `IP_ADD_MEMBERSHIP` / `IPV6_JOIN_GROUP`: start receiving this group.
    MulticastJoin(MulticastGroup),
    /// `IP_DROP_MEMBERSHIP` / `IPV6_LEAVE_GROUP`: stop receiving this group.
    MulticastLeave(MulticastGroup),
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// Names a readable socket option for `Loop::get_option`.
///
/// Group membership has no getter: the OS exposes no per-socket membership
/// query, so [`SocketOption::MulticastJoin`] and
/// [`SocketOption::MulticastLeave`] have no kind here rather than a kind that
/// would have to answer `Unsupported` everywhere.
pub enum SocketOptionKind {
    /// Read [`SocketOption::NoDelay`].
    NoDelay,
    /// Read [`SocketOption::KeepAlive`], including the schedule the OS holds.
    KeepAlive,
    /// Read [`SocketOption::Linger`].
    Linger,
    /// Read [`SocketOption::RecvBufferSize`] as the OS actually kept it.
    RecvBufferSize,
    /// Read [`SocketOption::SendBufferSize`] as the OS actually kept it.
    SendBufferSize,
    /// Read [`SocketOption::Ttl`].
    Ttl,
    /// Read [`SocketOption::Ipv6Only`].
    Ipv6Only,
    /// Read [`SocketOption::Broadcast`].
    Broadcast,
    /// Read [`SocketOption::MulticastTtl`].
    MulticastTtl,
    /// Read [`SocketOption::MulticastLoop`].
    MulticastLoop,
}
#[derive(Clone, Copy, Debug, Default)]
/// UDP binding options. Reuse is bind-time only and stays here, not in
/// [`SocketOption`]; everything changeable on a live socket is an option.
pub struct UdpOpts {
    /// Enable SO_REUSEPORT when supported; macOS does not promise balanced accepts.
    /// Defaults to false: a live UDP endpoint cannot be shared by another bind.
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
    #[cfg(turnloop_backend = "web")]
    /// Open a host HTTP fetch stream.
    Fetch {
        /// Host URL for the request.
        url: String,
    },
    #[cfg(turnloop_backend = "web")]
    /// Open a host WebSocket stream.
    WebSocket {
        /// Host URL for the connection.
        url: String,
    },
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
