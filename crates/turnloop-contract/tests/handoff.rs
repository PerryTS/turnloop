//! Handing a transport's descriptor back to the host (issue #35).
//!
//! The contract functions cover the loop side — what is refused, and what the
//! loop still knows afterwards. Everything here goes around turnloop entirely:
//! it takes the descriptor the loop handed over and drives it from the test
//! process with `libc`/Winsock or `std::net`, so a backend that kept a claim on
//! the transport, or handed back the wrong one, could not pass.
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
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};
use turnloop::{backend::Platform, *};

macro_rules! contract {
    ($($name:ident),+ $(,)?) => {
        $(#[test] fn $name() { turnloop_contract::handoff::$name::<Platform>(); })+
    };
}
contract!(
    pending_operations_refuse_handoff,
    closing_and_closed_handles_refuse_handoff,
    listeners_and_accepted_sockets_are_handed_off,
    raw_transport_reports_live_transports,
);

/// A loop-driven connection whose peer is an ordinary blocking `TcpStream` this
/// test owns, so the peer outlives the loop and can prove bytes still flow.
fn connected(l: &mut Loop) -> (Handle, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("peer listener");
    let address = listener.local_addr().expect("peer address");
    let h = l
        .tcp_connect(address, &TcpOpts::default(), Token(1))
        .expect("connect");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut connected = false;
    while !connected {
        assert!(l.now() < until, "connect timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::Connected), "{c:?}");
            connected = true;
        }
    }
    let (peer, _) = listener.accept().expect("accept");
    peer.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    (h, peer)
}

/// Write from the loop and read the bytes on the peer, so the connection is
/// provably live and mid-stream before it is handed over.
fn exchange_through_the_loop(l: &mut Loop, h: Handle, peer: &mut TcpStream) {
    l.write(h, WriteBuf::Owned(b"mid-stream".to_vec()), Token(2))
        .expect("write");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut wrote = 0;
    while wrote == 0 {
        assert!(l.now() < until, "loop write timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Wrote(n) => {
                    assert_eq!(n, 10);
                    wrote += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    let mut bytes = [0; 10];
    peer.read_exact(&mut bytes).expect("peer read");
    assert_eq!(&bytes, b"mid-stream");
}

/// Turn an idle loop for a while; nothing may arrive for a transport it gave away.
fn assert_quiet(l: &mut Loop) {
    let mut out = Completions::default();
    let until = l.now() + Duration::from_millis(60);
    while l.now() < until {
        l.turn(Timeout::After(Duration::from_millis(10)), &mut out)
            .expect("idle turn");
        assert_eq!(out.len(), 0, "a handed-off transport produced {out:?}");
    }
}

#[test]
fn a_handed_off_socket_carries_bytes_after_its_loop_is_dropped() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let (h, mut peer) = connected(&mut l);
    exchange_through_the_loop(&mut l, h, &mut peer);

    let reported = l.raw_transport(h).expect("live identity");
    let transport = l.detach(h).expect("quiescent handoff");
    assert_eq!(
        l.raw_transport(h).expect_err("gone").kind,
        ErrorKind::NotFound
    );
    assert_quiet(&mut l);
    assert!(!l.alive(), "the loop still counts a transport it gave away");

    let mut stream = platform::into_stream(transport, reported);
    // The loop is gone entirely: nothing it owned can be keeping this alive.
    drop(l);

    stream.write_all(b"after handoff").expect("host write");
    let mut bytes = [0; 13];
    peer.read_exact(&mut bytes).expect("peer read");
    assert_eq!(&bytes, b"after handoff");

    peer.write_all(b"and back again").expect("peer write");
    let mut bytes = [0; 14];
    stream.read_exact(&mut bytes).expect("host read");
    assert_eq!(&bytes, b"and back again");
}

#[test]
fn a_handed_off_socket_is_driven_by_the_bare_descriptor() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let (h, mut peer) = connected(&mut l);
    exchange_through_the_loop(&mut l, h, &mut peer);
    let reported = l.raw_transport(h).expect("live identity");
    let transport = l.detach(h).expect("quiescent handoff");

    // No std wrapper, no turnloop: the descriptor itself, as the host received it.
    let raw = platform::into_raw(transport, reported);
    assert_eq!(platform::send(raw, b"raw descriptor"), 14);
    let mut bytes = [0; 14];
    peer.read_exact(&mut bytes).expect("peer read");
    assert_eq!(&bytes, b"raw descriptor");

    peer.write_all(b"raw reply").expect("peer write");
    let mut bytes = [0; 9];
    assert_eq!(platform::recv(raw, &mut bytes), 9);
    assert_eq!(&bytes, b"raw reply");
    assert_quiet(&mut l);
    platform::close(raw);
}

#[test]
fn a_handed_off_listener_accepts_in_the_host() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let server = l
        .tcp_listen(
            "127.0.0.1:0".parse().expect("address"),
            &ListenOpts::default(),
        )
        .expect("listen");
    let address = l.local_addr(server).expect("address");
    let reported = l.raw_transport(server).expect("live identity");
    let transport = l.detach(server).expect("listener handoff");
    let listener: TcpListener = platform::into_listener(transport, reported);
    listener.set_nonblocking(false).expect("blocking listener");

    // The loop is now a client of a listener the host owns.
    let client = l
        .tcp_connect(address, &TcpOpts::default(), Token(1))
        .expect("connect");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut connected = false;
    while !connected {
        assert!(
            l.now() < until,
            "connect to a handed-off listener timed out"
        );
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert!(matches!(c.result, OpResult::Connected), "{c:?}");
            connected = true;
        }
    }
    let (mut accepted, _) = listener.accept().expect("host accept");
    accepted
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    l.write(client, WriteBuf::Owned(b"host accepted".to_vec()), Token(2))
        .expect("write");
    let until = l.now() + Duration::from_secs(5);
    let mut wrote = 0;
    while wrote == 0 {
        assert!(l.now() < until, "write timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Wrote(n) => {
                    assert_eq!(n, 13);
                    wrote += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    let mut bytes = [0; 13];
    accepted.read_exact(&mut bytes).expect("read");
    assert_eq!(&bytes, b"host accepted");
    l.close(client, Token(3)).expect("close");
    let until = l.now() + Duration::from_secs(5);
    while l.alive() {
        assert!(l.now() < until, "close timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        out.drain();
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

    fn owned(transport: Detached, reported: RawTransport) -> OwnedFd {
        let fd = transport.into_fd();
        assert_eq!(
            RawTransport::Fd(fd.as_raw_fd()),
            reported,
            "the host received a different descriptor from the reported one"
        );
        fd
    }
    pub fn into_stream(transport: Detached, reported: RawTransport) -> TcpStream {
        let stream = TcpStream::from(owned(transport, reported));
        // Loop-created sockets are handed over non-blocking, as documented.
        stream.set_nonblocking(false).expect("blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        stream
    }
    pub fn into_listener(transport: Detached, reported: RawTransport) -> TcpListener {
        TcpListener::from(owned(transport, reported))
    }
    pub fn into_raw(transport: Detached, reported: RawTransport) -> RawFd {
        owned(transport, reported).into_raw_fd()
    }
    pub fn send(fd: RawFd, bytes: &[u8]) -> usize {
        // SAFETY: a live descriptor this test owns, and a valid readable slice.
        let n = unsafe { libc::send(fd, bytes.as_ptr().cast(), bytes.len(), 0) };
        assert!(n >= 0, "send: {}", std::io::Error::last_os_error());
        n as usize
    }
    pub fn recv(fd: RawFd, bytes: &mut [u8]) -> usize {
        // The descriptor is still non-blocking; poll it rather than spinning.
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd describing a descriptor this test owns.
        let ready = unsafe { libc::poll(&mut poll, 1, 5_000) };
        assert_eq!(ready, 1, "poll: {}", std::io::Error::last_os_error());
        // SAFETY: a live descriptor this test owns, and a valid writable slice.
        let n = unsafe { libc::recv(fd, bytes.as_mut_ptr().cast(), bytes.len(), 0) };
        assert!(n >= 0, "recv: {}", std::io::Error::last_os_error());
        n as usize
    }
    pub fn close(fd: RawFd) {
        // SAFETY: this test owns the descriptor and uses it no further.
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawSocket, FromRawSocket, IntoRawSocket, OwnedSocket, RawSocket};
    use windows_sys::Win32::Networking::WinSock as ws;

    fn owned(transport: Detached, reported: RawTransport) -> OwnedSocket {
        let socket = transport.into_socket().expect("socket transport");
        assert_eq!(
            RawTransport::Socket(socket.as_raw_socket() as usize),
            reported,
            "the host received a different socket from the reported one"
        );
        socket
    }
    pub fn into_stream(transport: Detached, reported: RawTransport) -> TcpStream {
        let stream = TcpStream::from(owned(transport, reported));
        // Loop-created sockets are handed over non-blocking, as documented.
        stream.set_nonblocking(false).expect("blocking");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        stream
    }
    pub fn into_listener(transport: Detached, reported: RawTransport) -> TcpListener {
        TcpListener::from(owned(transport, reported))
    }
    pub fn into_raw(transport: Detached, reported: RawTransport) -> RawSocket {
        owned(transport, reported).into_raw_socket()
    }
    pub fn send(socket: RawSocket, bytes: &[u8]) -> usize {
        // SAFETY: a live socket this test owns, and a valid readable slice.
        let n = unsafe { ws::send(socket as usize, bytes.as_ptr(), bytes.len() as i32, 0) };
        assert_ne!(
            n,
            ws::SOCKET_ERROR,
            "send: {}",
            std::io::Error::last_os_error()
        );
        n as usize
    }
    pub fn recv(socket: RawSocket, bytes: &mut [u8]) -> usize {
        // The socket is still non-blocking (FIONBIO), as documented.
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            // SAFETY: a live socket this test owns, and a valid writable slice.
            let n = unsafe { ws::recv(socket as usize, bytes.as_mut_ptr(), bytes.len() as i32, 0) };
            if n != ws::SOCKET_ERROR {
                return n as usize;
            }
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::WouldBlock,
                "recv: {error}"
            );
            assert!(std::time::Instant::now() < until, "recv timed out");
            std::thread::yield_now();
        }
    }
    pub fn close(socket: RawSocket) {
        // SAFETY: this test owns the socket and uses it no further.
        drop(unsafe { OwnedSocket::from_raw_socket(socket) });
    }
}

/// The Windows half of the answer: a handle's completion-port association is
/// permanent, so what the receiving host may do with it is the whole contract.
#[cfg(windows)]
mod iocp {
    use super::*;
    use std::{
        os::windows::io::{AsRawHandle, AsRawSocket, FromRawHandle, OwnedHandle, RawSocket},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_PARAMETER, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
        Networking::WinSock as ws,
        Storage::FileSystem::{ReadFile, WriteFile},
        System::{
            IO::{CreateIoCompletionPort, GetOverlappedResult, OVERLAPPED},
            Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject},
        },
    };

    /// A completion port of this test's own, to try the association against.
    fn port() -> OwnedHandle {
        // SAFETY: INVALID_HANDLE_VALUE with a null port creates a new bare port.
        let raw = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, ptr::null_mut(), 0, 1) };
        assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
        // SAFETY: a newly created, uniquely owned kernel handle.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    }
    fn associate(handle: HANDLE, port: &OwnedHandle, key: usize) -> std::io::Result<()> {
        // SAFETY: a live handle this test owns and a live port; association
        // starts no I/O and retains no pointer.
        let result = unsafe { CreateIoCompletionPort(handle, port.as_raw_handle(), key, 0) };
        if result.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    /// `WSADuplicateSocketW` + `WSASocketW`. The new descriptor references the
    /// *same underlying socket*, which is where the completion-port association
    /// lives, so this does not escape it — the test below proves that.
    fn duplicate(socket: RawSocket) -> ws::SOCKET {
        // SAFETY: valid zeroed C output storage for the protocol information.
        let mut info: ws::WSAPROTOCOL_INFOW = unsafe { std::mem::zeroed() };
        // SAFETY: a live socket this test owns and writable protocol storage.
        let code =
            unsafe { ws::WSADuplicateSocketW(socket as usize, GetCurrentProcessId(), &mut info) };
        assert_eq!(code, 0, "duplicate: {}", std::io::Error::last_os_error());
        // SAFETY: `info` was just filled by WSADuplicateSocketW and stays live.
        let duplicate = unsafe {
            ws::WSASocketW(
                ws::FROM_PROTOCOL_INFO,
                ws::FROM_PROTOCOL_INFO,
                ws::FROM_PROTOCOL_INFO,
                &info,
                0,
                ws::WSA_FLAG_OVERLAPPED,
            )
        };
        assert_ne!(
            duplicate,
            ws::INVALID_SOCKET,
            "socket from protocol info: {}",
            std::io::Error::last_os_error()
        );
        duplicate
    }

    #[test]
    fn a_handed_off_socket_keeps_its_association_even_through_a_duplicate() {
        let mut l = Loop::new(Config::default()).expect("loop");
        let (h, mut peer) = connected(&mut l);
        exchange_through_the_loop(&mut l, h, &mut peer);
        let reported = l.raw_transport(h).expect("live identity");
        let transport = l.detach(h).expect("quiescent handoff");
        let socket = transport.into_socket().expect("socket transport");
        assert_eq!(
            RawTransport::Socket(socket.as_raw_socket() as usize),
            reported
        );

        let port = port();
        let refused = associate(socket.as_raw_socket() as HANDLE, &port, 1)
            .expect_err("a socket already on a port cannot join another");
        assert_eq!(
            refused.raw_os_error(),
            Some(ERROR_INVALID_PARAMETER as i32),
            "unexpected association error: {refused}"
        );

        // Synchronous Winsock calls never touch a completion port, so the
        // association being permanent costs the receiving host nothing.
        let raw = socket.as_raw_socket();
        assert_eq!(platform::send(raw, b"still ours"), 10);
        let mut bytes = [0; 10];
        peer.read_exact(&mut bytes).expect("peer read");
        assert_eq!(&bytes, b"still ours");
        peer.write_all(b"and back").expect("peer write");
        let mut bytes = [0; 8];
        assert_eq!(platform::recv(raw, &mut bytes), 8);
        assert_eq!(&bytes, b"and back");
        assert_quiet(&mut l);

        // There is no way out of the association. `WSADuplicateSocketW` gives a
        // new *descriptor* for the same underlying socket, and the association
        // belongs to that socket, so the duplicate inherits it. (Windows CI
        // caught this: the first version of this test asserted the opposite.)
        // The route back to completion-port-driven I/O is `Loop::attach`, which
        // detects an imported association and routes through overlapped events.
        let duplicate = duplicate(raw);
        let refused = associate(duplicate as HANDLE, &port, 2)
            .expect_err("a duplicate shares the original's association");
        assert_eq!(
            refused.raw_os_error(),
            Some(ERROR_INVALID_PARAMETER as i32),
            "unexpected duplicate association error: {refused}"
        );
        // SAFETY: the duplicate is this test's and is used no further.
        assert_eq!(unsafe { ws::closesocket(duplicate) }, 0);
        drop(socket);
    }

    #[test]
    fn a_handed_off_named_pipe_is_driven_with_a_tagged_event() {
        let name = PipeName(format!(r"\\.\pipe\tl-handoff-{}", std::process::id()).into());
        let mut l = Loop::new(Config::default()).expect("loop");
        let (listener, client, conn) =
            turnloop_contract::native_surface::pipe_pair::<Platform>(&mut l, &name);
        turnloop_contract::native_surface::transfer(&mut l, client, conn, b"mid-stream pipe");

        let reported = l.raw_transport(client).expect("live identity");
        let transport = l.detach(client).expect("pipe handoff");
        let handle = transport.into_handle().expect("pipe handle");
        assert_eq!(
            RawTransport::Handle(handle.as_raw_handle() as usize),
            reported
        );
        let port = port();
        let refused = associate(handle.as_raw_handle(), &port, 1)
            .expect_err("the association travels with the pipe instance");
        assert_eq!(
            refused.raw_os_error(),
            Some(ERROR_INVALID_PARAMETER as i32),
            "unexpected association error: {refused}"
        );

        // FILE_FLAG_OVERLAPPED survives the handoff, so every call needs an
        // OVERLAPPED. Tagging hEvent's low-order bit suppresses the completion
        // packet, which is what keeps the source loop's port clean.
        // SAFETY: a manual-reset, initially unsignalled, unnamed event.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        assert!(!event.is_null(), "{}", std::io::Error::last_os_error());
        // SAFETY: a newly created, uniquely owned kernel handle.
        let event = unsafe { OwnedHandle::from_raw_handle(event) };
        let tagged = (event.as_raw_handle() as usize | 1) as HANDLE;

        let raw = handle.as_raw_handle();
        let bytes = b"from the host";
        // SAFETY: all-zero OVERLAPPED is valid; only hEvent is set.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = tagged;
        // SAFETY: a live overlapped handle this test owns; the buffer and the
        // OVERLAPPED stay alive and unmoved until GetOverlappedResult returns.
        let started = unsafe {
            WriteFile(
                raw,
                bytes.as_ptr(),
                bytes.len() as u32,
                ptr::null_mut(),
                &mut overlapped,
            )
        };
        let mut written = 0;
        if started == 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING as i32),
                "host write: {error}"
            );
            // SAFETY: the event is live and owned by this test.
            let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), 5_000) };
            assert_eq!(
                waited, WAIT_OBJECT_0,
                "the host's own event never signalled"
            );
        }
        // SAFETY: the same live handle and pinned OVERLAPPED as the write.
        let finished = unsafe { GetOverlappedResult(raw, &overlapped, &mut written, 1) };
        assert_ne!(
            finished,
            0,
            "host write result: {}",
            std::io::Error::last_os_error()
        );
        assert_eq!(written as usize, bytes.len());

        // The loop still owns the other end and reads what the host wrote.
        l.read(conn, ReadBuf::Pooled, Token(30)).expect("read");
        let mut out = Completions::default();
        let until = l.now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while seen.len() < bytes.len() {
            assert!(l.now() < until, "the loop never saw the host's write");
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                match c.result {
                    OpResult::Read {
                        n,
                        lease: Some(data),
                    } => {
                        seen.extend_from_slice(&data.as_slice()[..n]);
                        if seen.len() < bytes.len() {
                            l.read(conn, ReadBuf::Pooled, Token(30)).expect("continue");
                        }
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        assert_eq!(seen, bytes);

        // And back: the loop writes, the host reads through its own event.
        l.write(conn, WriteBuf::Owned(b"to the host".to_vec()), Token(31))
            .expect("write");
        let until = l.now() + Duration::from_secs(5);
        let mut wrote = 0;
        while wrote == 0 {
            assert!(l.now() < until, "loop write timed out");
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            for c in out.drain() {
                match c.result {
                    OpResult::Wrote(n) => {
                        assert_eq!(n, 11);
                        wrote += 1;
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        let mut buffer = [0u8; 11];
        // SAFETY: all-zero OVERLAPPED is valid; only hEvent is set.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = tagged;
        // SAFETY: a live overlapped handle this test owns; the buffer and the
        // OVERLAPPED stay alive and unmoved until GetOverlappedResult returns.
        let started = unsafe {
            ReadFile(
                raw,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                ptr::null_mut(),
                &mut overlapped,
            )
        };
        let mut read = 0;
        if started == 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING as i32),
                "host read: {error}"
            );
            // SAFETY: the event is live and owned by this test.
            let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), 5_000) };
            assert_eq!(
                waited, WAIT_OBJECT_0,
                "the host's own event never signalled"
            );
        }
        // SAFETY: the same live handle and pinned OVERLAPPED as the read.
        let finished = unsafe { GetOverlappedResult(raw, &overlapped, &mut read, 1) };
        assert_ne!(
            finished,
            0,
            "host read result: {}",
            std::io::Error::last_os_error()
        );
        assert_eq!(&buffer[..read as usize], b"to the host");

        // The source loop's port never saw a packet for the handle it gave away.
        assert_quiet(&mut l);
        drop(handle);
        let mut out = Completions::default();
        for (i, h) in [conn, listener].into_iter().enumerate() {
            l.close(h, Token(40 + i as u64)).expect("close");
        }
        let until = l.now() + Duration::from_secs(5);
        while l.alive() {
            assert!(l.now() < until, "close timed out");
            l.turn(Timeout::Until(until), &mut out).expect("turn");
            out.drain();
        }
    }
}
