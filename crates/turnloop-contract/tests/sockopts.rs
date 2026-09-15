//! Socket options on live handles, on every native backend (issue #34).
//!
//! The shared contract functions already read every value back from the OS
//! through `get_option`. The probes in this file go around turnloop entirely:
//! they call `getsockopt` from the test process on a descriptor turnloop does not
//! know they hold, so a backend that answered its own getter from a cache of what
//! was set could not pass them.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        windows
    )
))]
use std::net::{Ipv4Addr, SocketAddr};
use turnloop::{backend::Platform, *};
use turnloop_contract::sockopts as contract;

macro_rules! contract {
    ($($name:ident),+ $(,)?) => { $(#[test] fn $name() { contract::$name::<Platform>(); })+ };
}
contract!(
    keep_alive_round_trip,
    buffer_sizes_round_trip,
    ttl_round_trip,
    accept_defaults_keep_alive,
    option_handle_validation,
    linger_zero_resets_the_connection,
    udp_broadcast_and_multicast_options,
    ipv6_only_is_readable_and_bind_time,
    nodelay_round_trip_and_accept_default,
    nodelay_small_write_round_trip,
    accept_defaults_survive_transfer,
);
#[test]
fn multicast_membership_is_tracked() {
    contract::multicast_membership_is_tracked::<Platform>(
        Ipv4Addr::new(224, 0, 0, 251).into(),
        (Ipv4Addr::UNSPECIFIED, 0).into(),
    );
}

#[cfg(unix)]
mod probe {
    use std::os::fd::{IntoRawFd, OwnedFd, RawFd};
    /// `getsockopt` straight from the test process, with no turnloop code involved.
    pub fn int(fd: RawFd, level: i32, name: i32) -> i32 {
        let mut value = 0i32;
        let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
        // SAFETY: a live descriptor with writable integer output and its capacity.
        let code = unsafe {
            libc::getsockopt(
                fd,
                level,
                name,
                std::ptr::from_mut(&mut value).cast(),
                &mut len,
            )
        };
        assert_eq!(code, 0, "getsockopt: {}", std::io::Error::last_os_error());
        value
    }
    pub fn nodelay(fd: RawFd) -> bool {
        int(fd, libc::IPPROTO_TCP, libc::TCP_NODELAY) != 0
    }
    pub fn keep_alive(fd: RawFd) -> bool {
        int(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE) != 0
    }
    fn endpoint(fd: RawFd, peer: bool) -> Option<std::net::SocketAddr> {
        // SAFETY: plain writable address storage; zero is a valid initial value.
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let out = std::ptr::from_mut(&mut storage).cast();
        // SAFETY: a possibly-unrelated descriptor with valid output storage; a
        // non-socket simply fails and is skipped.
        let code = unsafe {
            if peer {
                libc::getpeername(fd, out, &mut len)
            } else {
                libc::getsockname(fd, out, &mut len)
            }
        };
        if code != 0 || i32::from(storage.ss_family) != libc::AF_INET {
            return None;
        }
        // SAFETY: the family was checked and the storage is aligned for sockaddr_in.
        let addr = unsafe { &*std::ptr::from_ref(&storage).cast::<libc::sockaddr_in>() };
        Some(
            (
                std::net::Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes()),
                u16::from_be(addr.sin_port),
            )
                .into(),
        )
    }
    /// Find the descriptor turnloop accepted, by its endpoints alone. The listener
    /// shares the local address but has no peer, so the match is unique.
    pub fn accepted(local: std::net::SocketAddr, peer: std::net::SocketAddr) -> RawFd {
        let directory = if cfg!(any(target_os = "linux", target_os = "android")) {
            "/proc/self/fd"
        } else {
            "/dev/fd"
        };
        let mut found = None;
        for entry in std::fs::read_dir(directory).expect("descriptor directory") {
            let entry = entry.expect("descriptor entry");
            let Ok(fd) = entry.file_name().to_string_lossy().parse::<RawFd>() else {
                continue;
            };
            if endpoint(fd, false) == Some(local) && endpoint(fd, true) == Some(peer) {
                assert!(found.is_none(), "two descriptors match {local} <- {peer}");
                found = Some(fd);
            }
        }
        found.unwrap_or_else(|| panic!("no descriptor matches {local} <- {peer}"))
    }
    /// A second reference to one socket: `dup` shares the underlying socket, so an
    /// option set through turnloop's handle is visible here. The clone is leaked
    /// on purpose and reclaimed by `close_raw` once the loop has released its own.
    pub fn shared(socket: impl Into<OwnedFd>) -> (OwnedFd, RawFd) {
        let owned: OwnedFd = socket.into();
        let raw = owned.try_clone().expect("dup").into_raw_fd();
        (owned, raw)
    }
}

/// The accepted connection really carries `TCP_NODELAY` and `SO_KEEPALIVE`,
/// asserted with the test's own `getsockopt` on the accepted descriptor, found
/// without asking turnloop for it.
#[cfg(unix)]
#[test]
fn accepted_socket_options_are_visible_to_getsockopt() {
    let listen = ListenOpts {
        accept_defaults: AcceptDefaults {
            nodelay: true,
            keep_alive: Some(KeepAlive {
                idle: Some(std::time::Duration::from_secs(9)),
                ..KeepAlive::default()
            }),
        },
        ..ListenOpts::default()
    };
    let mut l = Loop::new(Config::default()).expect("loop");
    let (server, client, conn) = contract::plain_pair(&mut l, &listen);
    let local = l.local_addr(conn).expect("accepted local address");
    let peer = l.local_addr(client).expect("client local address");
    let fd = probe::accepted(local, peer);
    assert!(
        probe::nodelay(fd),
        "the listener's nodelay default never reached the accepted socket"
    );
    assert!(
        probe::keep_alive(fd),
        "the listener's keep-alive default never reached the accepted socket"
    );
    assert_eq!(
        probe::int(fd, libc::IPPROTO_TCP, KEEPIDLE),
        9,
        "the accepted socket kept a different idle time"
    );
    // And a later per-connection change is equally real.
    l.set_option(conn, SocketOption::NoDelay(false))
        .expect("clear nodelay");
    assert!(
        !probe::nodelay(fd),
        "set_option(false) did not reach the accepted socket"
    );
    l.set_option(conn, SocketOption::KeepAlive(None))
        .expect("stop probing");
    assert!(!probe::keep_alive(fd), "keep-alive was not turned off");
    contract::close_all(&mut l, &[client, conn, server]);
}
#[cfg(all(unix, target_vendor = "apple"))]
const KEEPIDLE: i32 = libc::TCP_KEEPALIVE;
#[cfg(all(unix, not(target_vendor = "apple")))]
const KEEPIDLE: i32 = libc::TCP_KEEPIDLE;

/// An adopted socket is configurable too, proven on a second reference to the
/// same socket that the loop knows nothing about.
#[test]
fn adopted_socket_options_reach_the_shared_socket() {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
    let address: SocketAddr = listener.local_addr().expect("address");
    let client = std::net::TcpStream::connect(address).expect("connect");
    let (accepted, _) = listener.accept().expect("accept");
    let mut l = Loop::new(Config::default()).expect("loop");

    #[cfg(unix)]
    let (owned, raw) = {
        let (owned, raw) = probe::shared(accepted);
        (Detached::from_fd(owned).expect("adopt"), raw)
    };
    #[cfg(windows)]
    let (owned, raw) = {
        let shared = accepted.try_clone().expect("duplicate");
        let raw = windows_probe::leak(shared);
        (Detached::from_socket(accepted.into()).expect("adopt"), raw)
    };
    let h = l.attach(owned, Token(1)).expect("attach");
    assert!(!nodelay_of(raw), "the shared socket starts with Nagle on");
    l.set_option(h, SocketOption::NoDelay(true))
        .expect("nodelay");
    assert!(
        nodelay_of(raw),
        "set_option did not reach the shared socket"
    );
    l.set_option(h, SocketOption::RecvBufferSize(48 * 1024))
        .expect("receive buffer");
    let bytes = recv_buffer_of(raw);
    assert!(
        (48 * 1024..=96 * 1024).contains(&bytes),
        "the OS reports {bytes} bytes for a 49152-byte request"
    );
    contract::close_all(&mut l, &[h]);
    drop(client);
    close_raw(raw);
}
#[cfg(unix)]
fn nodelay_of(raw: std::os::fd::RawFd) -> bool {
    probe::nodelay(raw)
}
#[cfg(unix)]
fn recv_buffer_of(raw: std::os::fd::RawFd) -> u32 {
    probe::int(raw, libc::SOL_SOCKET, libc::SO_RCVBUF).max(0) as u32
}
#[cfg(unix)]
fn close_raw(raw: std::os::fd::RawFd) {
    // SAFETY: this descriptor was leaked by `probe::shared` and has no other owner.
    drop(unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw) });
}
#[cfg(windows)]
mod windows_probe {
    use std::os::windows::io::{IntoRawSocket, OwnedSocket, RawSocket};
    use windows_sys::Win32::Networking::WinSock::*;
    pub fn leak(socket: std::net::TcpStream) -> RawSocket {
        let owned: OwnedSocket = socket.into();
        owned.into_raw_socket()
    }
    pub fn int(raw: RawSocket, level: i32, name: i32) -> i32 {
        let mut value = 0i32;
        let mut len = std::mem::size_of::<i32>() as i32;
        // SAFETY: a live socket with writable integer output and its capacity.
        let code = unsafe {
            getsockopt(
                raw as usize,
                level,
                name,
                std::ptr::from_mut(&mut value).cast(),
                &mut len,
            )
        };
        assert_eq!(code, 0, "getsockopt: {}", std::io::Error::last_os_error());
        value
    }
    pub fn close(raw: RawSocket) {
        // SAFETY: this socket was leaked by `leak` and has no other owner.
        drop(unsafe { <OwnedSocket as std::os::windows::io::FromRawSocket>::from_raw_socket(raw) });
    }
}
#[cfg(windows)]
fn nodelay_of(raw: std::os::windows::io::RawSocket) -> bool {
    windows_probe::int(
        raw,
        windows_sys::Win32::Networking::WinSock::IPPROTO_TCP,
        windows_sys::Win32::Networking::WinSock::TCP_NODELAY,
    ) != 0
}
#[cfg(windows)]
fn recv_buffer_of(raw: std::os::windows::io::RawSocket) -> u32 {
    windows_probe::int(
        raw,
        windows_sys::Win32::Networking::WinSock::SOL_SOCKET,
        windows_sys::Win32::Networking::WinSock::SO_RCVBUF,
    )
    .max(0) as u32
}
#[cfg(windows)]
fn close_raw(raw: std::os::windows::io::RawSocket) {
    windows_probe::close(raw);
}

/// Handles that are not sockets report `Unsupported` instead of reaching a
/// backend table with a foreign index.
#[test]
fn non_socket_handles_report_unsupported() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let stdio = l.open_stdio(Stdio::Stderr).expect("stderr");
    assert_eq!(
        l.set_option(stdio, SocketOption::NoDelay(true))
            .expect_err("stderr is not a socket")
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        l.get_option(stdio, SocketOptionKind::NoDelay)
            .expect_err("stderr is not a socket")
            .kind,
        ErrorKind::Unsupported
    );
    contract::close_all(&mut l, &[stdio]);
}
/// A local (AF_UNIX / named pipe) listener cannot apply TCP accept defaults, so
/// it refuses them when it is created rather than per connection.
#[test]
fn local_listener_refuses_tcp_accept_defaults() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let name = local_name();
    let listen = ListenOpts {
        accept_defaults: AcceptDefaults {
            nodelay: true,
            ..AcceptDefaults::EMPTY
        },
        ..ListenOpts::default()
    };
    assert_eq!(
        l.pipe_listen(&name, &listen)
            .expect_err("TCP defaults on a local listener")
            .kind,
        ErrorKind::Unsupported
    );
    assert!(!l.alive(), "a rejected listener retains nothing");
}
#[cfg(unix)]
fn local_name() -> PipeName {
    PipeName(std::env::temp_dir().join(format!("tl-sockopt-{}.sock", std::process::id())))
}
#[cfg(windows)]
fn local_name() -> PipeName {
    PipeName(format!(r"\\.\pipe\turnloop-sockopt-{}", std::process::id()).into())
}
/// An IPv4 membership keyed by interface index exists only on Linux; elsewhere it
/// is reported rather than quietly applied to the default interface.
#[test]
fn ipv4_membership_by_interface_index() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let udp = l
        .udp_bind((Ipv4Addr::UNSPECIFIED, 0).into(), &UdpOpts::default())
        .expect("bind");
    let group = MulticastGroup {
        group: Ipv4Addr::new(224, 0, 0, 251).into(),
        interface: 1,
    };
    let result = l.set_option(udp, SocketOption::MulticastJoin(group));
    if cfg!(any(target_os = "linux", target_os = "android")) {
        // Interface 1 is the loopback index on Linux; either it joins or the
        // kernel explains why, but the request is never silently redirected.
        if let Err(e) = result {
            assert_ne!(e.kind, ErrorKind::Unsupported, "Linux supports ip_mreqn");
        } else {
            l.set_option(udp, SocketOption::MulticastLeave(group))
                .expect("leave");
        }
    } else {
        assert_eq!(
            result.expect_err("no ip_mreqn here").kind,
            ErrorKind::Unsupported
        );
    }
    contract::close_all(&mut l, &[udp]);
}
