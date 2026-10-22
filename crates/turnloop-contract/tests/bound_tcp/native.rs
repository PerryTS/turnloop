use super::*;
#[cfg(unix)]
mod platform {
    use super::*;
    use std::{
        mem::size_of,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };
    pub fn socket(v6: bool, bind: bool) -> (Transport, SocketAddr) {
        // SAFETY: valid socket constants; successful creation transfers ownership.
        let raw = unsafe {
            libc::socket(
                if v6 { libc::AF_INET6 } else { libc::AF_INET },
                libc::SOCK_STREAM,
                0,
            )
        };
        assert!(raw >= 0, "socket: {}", std::io::Error::last_os_error());
        // SAFETY: raw is newly created and exclusively owned.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let option = |level, name, value: i32| {
            assert_eq!(
                // SAFETY: live descriptor and initialized integer input of exact size.
                unsafe {
                    libc::setsockopt(
                        raw,
                        level,
                        name,
                        std::ptr::from_ref(&value).cast(),
                        size_of::<i32>() as _,
                    )
                },
                0
            );
        };
        option(libc::IPPROTO_TCP, libc::TCP_NODELAY, 1);
        option(libc::SOL_SOCKET, libc::SO_RCVBUF, 65536);
        option(
            if v6 {
                libc::IPPROTO_IPV6
            } else {
                libc::IPPROTO_IP
            },
            if v6 {
                libc::IPV6_UNICAST_HOPS
            } else {
                libc::IP_TTL
            },
            47,
        );
        if bind {
            // SAFETY: zeroed C address storage has no invalid bit patterns.
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let len;
            if v6 {
                // SAFETY: sockaddr_storage is large/aligned enough for sockaddr_in6.
                let a =
                    unsafe { &mut *std::ptr::from_mut(&mut storage).cast::<libc::sockaddr_in6>() };
                a.sin6_family = libc::AF_INET6 as _;
                a.sin6_addr.s6_addr[15] = 1;
                len = size_of::<libc::sockaddr_in6>();
                #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
                {
                    a.sin6_len = len as _;
                }
            } else {
                // SAFETY: sockaddr_storage is large/aligned enough for sockaddr_in.
                let a =
                    unsafe { &mut *std::ptr::from_mut(&mut storage).cast::<libc::sockaddr_in>() };
                a.sin_family = libc::AF_INET as _;
                a.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
                len = size_of::<libc::sockaddr_in>();
                #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
                {
                    a.sin_len = len as _;
                }
            }
            assert_eq!(
                // SAFETY: initialized sockaddr matches length and family; fd is owned.
                unsafe { libc::bind(raw, std::ptr::from_ref(&storage).cast(), len as _) },
                0
            );
        }
        // TcpStream owns an unconnected stream socket; local_addr only queries it.
        let stream = TcpStream::from(fd);
        let local = stream.local_addr().expect("local address");
        assert_eq!(stream.as_raw_fd(), raw);
        (Transport::from_fd(stream.into()).expect("adopt"), local)
    }
    pub fn listener(socket: TcpListener) -> Transport {
        Transport::from_listener_fd(socket.into()).unwrap()
    }
    pub fn stream(socket: TcpStream) -> Transport {
        Transport::from_fd(socket.into()).unwrap()
    }
}
#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        mem::size_of,
        os::windows::io::{FromRawSocket, OwnedSocket},
    };
    use windows_sys::Win32::Networking::WinSock::*;
    pub fn socket(v6: bool, bind_socket: bool) -> (Transport, SocketAddr) {
        // Ensure Winsock is initialized using std's process-lifetime setup.
        let init = TcpListener::bind("127.0.0.1:0").unwrap();
        drop(init);
        // SAFETY: initialized Winsock, no provider pointer, exclusive new socket.
        let raw = unsafe {
            WSASocketW(
                if v6 { AF_INET6 } else { AF_INET } as _,
                SOCK_STREAM,
                IPPROTO_TCP,
                std::ptr::null(),
                0,
                WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
            )
        };
        assert_ne!(raw, INVALID_SOCKET);
        // SAFETY: successful WSASocketW transferred exclusive ownership.
        let owned = unsafe { OwnedSocket::from_raw_socket(raw as _) };
        let option = |level, name, value: i32| {
            assert_eq!(
                // SAFETY: valid socket and initialized integer of exact option size.
                unsafe {
                    setsockopt(
                        raw,
                        level,
                        name,
                        std::ptr::from_ref(&value).cast(),
                        size_of::<i32>() as _,
                    )
                },
                0
            );
        };
        option(IPPROTO_TCP, TCP_NODELAY, 1);
        option(SOL_SOCKET, SO_RCVBUF, 65536);
        option(SOL_SOCKET, SO_EXCLUSIVEADDRUSE, 1);
        option(
            if v6 { IPPROTO_IPV6 } else { IPPROTO_IP },
            if v6 { IPV6_UNICAST_HOPS } else { IP_TTL },
            47,
        );
        if bind_socket {
            // SAFETY: zeroed C address storage has no invalid bit patterns.
            let mut storage: SOCKADDR_STORAGE = unsafe { std::mem::zeroed() };
            let len;
            if v6 {
                // SAFETY: sockaddr storage is large/aligned enough for SOCKADDR_IN6.
                let a = unsafe { &mut *std::ptr::from_mut(&mut storage).cast::<SOCKADDR_IN6>() };
                a.sin6_family = AF_INET6;
                a.sin6_addr.u.Byte = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
                len = size_of::<SOCKADDR_IN6>();
            } else {
                // SAFETY: sockaddr storage is large/aligned enough for SOCKADDR_IN.
                let a = unsafe { &mut *std::ptr::from_mut(&mut storage).cast::<SOCKADDR_IN>() };
                a.sin_family = AF_INET;
                a.sin_addr.S_un.S_addr = u32::from_ne_bytes([127, 0, 0, 1]);
                len = size_of::<SOCKADDR_IN>();
            }
            assert_eq!(
                // SAFETY: initialized address matches family/length; owned live socket.
                unsafe { bind(raw, std::ptr::from_ref(&storage).cast(), len as _) },
                0
            );
        }
        let stream = TcpStream::from(owned);
        let local = if bind_socket {
            stream.local_addr().unwrap()
        } else if v6 {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        };
        (Transport::from_socket(stream.into()).expect("adopt"), local)
    }
    pub fn listener(socket: TcpListener) -> Transport {
        Transport::from_socket(socket.into()).unwrap()
    }
    pub fn stream(socket: TcpStream) -> Transport {
        Transport::from_socket(socket.into()).unwrap()
    }
}
pub use platform::*;

#[cfg(unix)]
pub fn identity(transport: &Transport) -> RawTransport {
    transport.raw_transport()
}
#[cfg(windows)]
pub fn identity(transport: &Transport) -> RawTransport {
    transport.raw_transport().unwrap()
}
