use crate::{
    operation::{Endpoint, Operation, drain, wsa_error},
    port::{Entry, Port, Wait, bool_result},
};
use std::{
    io,
    mem::size_of,
    os::windows::io::{FromRawSocket, OwnedSocket},
    ptr,
    rc::Rc,
    time::Duration,
};
use windows_sys::{
    Win32::{Networking::WinSock::*, Storage::FileSystem::SetFileCompletionNotificationModes},
    core::GUID,
};

pub(crate) struct Winsock;
impl Winsock {
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: initialized output storage; WSAStartup returns its error directly.
        // https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsastartup
        let mut data: WSADATA = unsafe { std::mem::zeroed() };
        // SAFETY: valid output and requested Winsock version 2.2.
        let result = unsafe { WSAStartup(0x202, &mut data) };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }
        let owner = Self;
        if data.wVersion != 0x202 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Winsock 2.2 required",
            ));
        }
        Ok(owner)
    }
}
impl Drop for Winsock {
    fn drop(&mut self) {
        // SAFETY: matches one successful WSAStartup; declared before all sockets/ops.
        unsafe {
            WSACleanup();
        }
    }
}

fn result(code: i32) -> io::Result<()> {
    if code == SOCKET_ERROR {
        Err(io::Error::from_raw_os_error(wsa_error()))
    } else {
        Ok(())
    }
}

fn socket() -> io::Result<Rc<Endpoint>> {
    // SAFETY: Winsock initialized by probe scope, no provider struct, overlapped TCP.
    let raw = unsafe {
        WSASocketW(
            AF_INET as i32,
            SOCK_STREAM,
            IPPROTO_TCP,
            ptr::null(),
            0,
            WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
        )
    };
    if raw == INVALID_SOCKET {
        return Err(io::Error::from_raw_os_error(wsa_error()));
    }
    // SAFETY: newly returned socket has one owner; OwnedSocket uses closesocket.
    Ok(Rc::new(Endpoint::Socket(unsafe {
        OwnedSocket::from_raw_socket(raw as _)
    })))
}

fn address(port: u16) -> SOCKADDR_IN {
    // SAFETY: zero is valid for every field, then fill family/address/port.
    let mut addr: SOCKADDR_IN = unsafe { std::mem::zeroed() };
    addr.sin_family = AF_INET;
    addr.sin_port = port.to_be();
    addr.sin_addr.S_un.S_addr = u32::from_ne_bytes([127, 0, 0, 1]);
    addr
}

fn bind_local(endpoint: &Endpoint) -> io::Result<()> {
    let addr = address(0);
    // SAFETY: live socket; IPv4 pointer and exact struct size valid for call.
    result(unsafe {
        bind(
            endpoint.raw() as usize,
            (&addr as *const SOCKADDR_IN).cast(),
            size_of::<SOCKADDR_IN>() as i32,
        )
    })
}

/// # Safety
/// T must be the nullable function-pointer type corresponding to guid.
unsafe fn extension<T: Copy>(socket: usize, guid: &GUID) -> io::Result<T> {
    let mut output = std::mem::MaybeUninit::<T>::uninit();
    let mut bytes = 0;
    // SAFETY: caller guarantees GUID/output ABI match; synchronous ioctl captures pointer.
    // Extensions must be obtained for the socket's provider, not statically linked.
    // https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsaioctl
    result(unsafe {
        WSAIoctl(
            socket,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            (guid as *const GUID).cast(),
            size_of::<GUID>() as u32,
            output.as_mut_ptr().cast(),
            size_of::<T>() as u32,
            &mut bytes,
            ptr::null_mut(),
            None,
        )
    })?;
    if bytes as usize != size_of::<T>() {
        return Err(io::Error::other("invalid extension size"));
    }
    // SAFETY: successful ioctl wrote the entire output of the documented type.
    Ok(unsafe { output.assume_init() })
}

fn associate(port: &Port, socket: &Endpoint, key: usize) -> io::Result<()> {
    // SAFETY: newly opened overlapped socket, no earlier association.
    unsafe { port.associate(socket.raw(), key) }?;
    let mut info: WSAPROTOCOL_INFOW = WSAPROTOCOL_INFOW::default();
    let mut length = size_of::<WSAPROTOCOL_INFOW>() as i32;
    // SAFETY: correct output type for SO_PROTOCOL_INFOW and its byte length.
    result(unsafe {
        getsockopt(
            socket.raw() as usize,
            SOL_SOCKET,
            SO_PROTOCOL_INFOW,
            (&mut info as *mut WSAPROTOCOL_INFOW).cast(),
            &mut length,
        )
    })?;
    if info.dwServiceFlags1 & XP1_IFS_HANDLES == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "skip-success requires an IFS provider",
        ));
    }
    // SAFETY: live IFS socket; flags are permanent. Only set skip after checking support.
    // https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setfilecompletionnotificationmodes
    bool_result(unsafe { SetFileCompletionNotificationModes(socket.raw(), 1) })
}

fn zero_read(op: &mut Operation) -> io::Result<()> {
    let (overlapped, buffer) = op.prepare()?;
    let wsabuf = WSABUF {
        len: 0,
        buf: buffer,
    };
    let mut flags = 0;
    let mut bytes = 0;
    // SAFETY: stable op/event; zero-length probe owns no receive payload; provider
    // captures the WSABUF descriptor before return. Recheck recv after completion.
    // https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsarecv
    let code = unsafe {
        WSARecv(
            op.endpoint.raw() as usize,
            &wsabuf,
            1,
            &mut bytes,
            &mut flags,
            overlapped,
            None,
        )
    };
    op.submitted(code == 0, wsa_error(), bytes, true)
}

fn send(op: &mut Operation, data: &[u8]) -> io::Result<()> {
    op.set_data(data);
    let (overlapped, buffer) = op.prepare()?;
    let wsabuf = WSABUF {
        len: data.len() as u32,
        buf: buffer,
    };
    let mut bytes = 0;
    // SAFETY: payload stored in stable op storage through actual completion;
    // provider captures stack WSABUF before return. One writer per socket.
    // https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsasend
    let code = unsafe {
        WSASend(
            op.endpoint.raw() as usize,
            &wsabuf,
            1,
            &mut bytes,
            0,
            overlapped,
            None,
        )
    };
    op.submitted(code == 0, wsa_error(), bytes, true)
}

#[derive(Debug)]
pub struct TcpStats {
    pub accepted: usize,
    pub connected: usize,
    pub bytes_echoed: usize,
    pub completions: usize,
    pub synchronous: usize,
    pub cancelled: usize,
    pub closed_after_cancel: bool,
}

pub fn probe() -> io::Result<TcpStats> {
    let _winsock = Winsock::new()?;
    let port = Port::new()?;
    let listener = socket()?;
    let client = socket()?;
    let server = socket()?;
    bind_local(&listener)?;
    bind_local(&client)?; // ConnectEx requires an explicitly bound socket.
    // SAFETY: bound TCP socket and positive backlog.
    result(unsafe { listen(listener.raw() as usize, 8) })?;
    associate(&port, &listener, 1)?;
    associate(&port, &client, 2)?;
    let mut addr = address(0);
    let mut addr_len = size_of::<SOCKADDR_IN>() as i32;
    // SAFETY: valid output for listener's assigned ephemeral port.
    result(unsafe {
        getsockname(
            listener.raw() as usize,
            (&mut addr as *mut SOCKADDR_IN).cast(),
            &mut addr_len,
        )
    })?;
    // SAFETY: GUIDs match nullable extension types.
    let accept: LPFN_ACCEPTEX = unsafe { extension(listener.raw() as usize, &WSAID_ACCEPTEX) }?;
    // SAFETY: GUID and output type match.
    let connect: LPFN_CONNECTEX = unsafe { extension(client.raw() as usize, &WSAID_CONNECTEX) }?;
    let accept = accept.ok_or_else(|| io::Error::other("AcceptEx missing"))?;
    let connect = connect.ok_or_else(|| io::Error::other("ConnectEx missing"))?;
    let mut accepting = Operation::new(Rc::clone(&listener))?;
    let mut connecting = Operation::new(Rc::clone(&client))?;
    let (ov, buffer) = accepting.prepare()?;
    let mut bytes = 0;
    // SAFETY: listener listening, server unbound/unconnected; 2*(IPv4+16) bytes
    // fit in stable buffer. Zero receive length avoids waiting for client data.
    // https://learn.microsoft.com/en-us/windows/win32/api/mswsock/nf-mswsock-acceptex
    let ok = unsafe {
        accept(
            listener.raw() as usize,
            server.raw() as usize,
            buffer.cast(),
            0,
            32,
            32,
            &mut bytes,
            ov,
        )
    };
    accepting.submitted(ok != 0, wsa_error(), bytes, true)?;
    let (ov, _) = connecting.prepare()?;
    // SAFETY: client bound, address valid, no send buffer; op storage stable.
    // https://learn.microsoft.com/en-us/windows/win32/api/mswsock/nc-mswsock-lpfn_connectex
    let ok = unsafe {
        connect(
            client.raw() as usize,
            (&addr as *const SOCKADDR_IN).cast(),
            addr_len,
            ptr::null(),
            0,
            &mut bytes,
            ov,
        )
    };
    connecting.submitted(ok != 0, wsa_error(), bytes, true)?;
    drain(&port, &mut [&mut accepting, &mut connecting])?;
    assert_eq!(accepting.result.map(|r| r.0), Some(0));
    assert_eq!(connecting.result.map(|r| r.0), Some(0));
    let listen_raw = listener.raw() as usize;
    // SAFETY: both extensions completed; install contexts before other socket APIs.
    unsafe {
        result(setsockopt(
            server.raw() as usize,
            SOL_SOCKET,
            SO_UPDATE_ACCEPT_CONTEXT,
            (&listen_raw as *const usize).cast(),
            size_of::<usize>() as i32,
        ))?;
        result(setsockopt(
            client.raw() as usize,
            SOL_SOCKET,
            SO_UPDATE_CONNECT_CONTEXT,
            ptr::null(),
            0,
        ))?;
    }
    associate(&port, &server, 3)?;
    for endpoint in [&server, &client] {
        let mut nonblocking = 1;
        // SAFETY: live connected socket; FIONBIO reads a u32.
        result(unsafe { ioctlsocket(endpoint.raw() as usize, FIONBIO, &mut nonblocking) })?;
    }
    let mut reading = Operation::new(Rc::clone(&server))?;
    let mut writing = Operation::new(Rc::clone(&client))?;
    let mut echoing = Operation::new(Rc::clone(&server))?;
    let mut receiving = Operation::new(Rc::clone(&client))?;
    let message = b"turnloop";
    let mut echoed = 0;
    for _ in 0..128 {
        zero_read(&mut reading)?;
        send(&mut writing, message)?;
        drain(&port, &mut [&mut reading, &mut writing])?;
        assert_eq!(writing.result, Some((0, message.len() as u32)));
        assert_eq!(reading.result, Some((0, 0)));
        let mut data = [0u8; 8];
        read_exact(&port, &mut reading, &mut data)?;
        assert_eq!(&data, message);
        zero_read(&mut receiving)?;
        send(&mut echoing, &data)?;
        drain(&port, &mut [&mut receiving, &mut echoing])?;
        assert_eq!(echoing.result, Some((0, 8)));
        read_exact(&port, &mut receiving, &mut data)?;
        assert_eq!(&data, message);
        echoed += data.len();
    }
    zero_read(&mut reading)?;
    assert!(
        reading.pending,
        "idle zero-byte receive must actually be pending"
    );
    reading.cancel()?;
    drain(&port, &mut [&mut reading])?;
    assert_eq!(reading.result.map(|r| r.0), Some(0xc0000120u32 as i32));
    let completions = accepting.completions
        + connecting.completions
        + reading.completions
        + writing.completions
        + echoing.completions
        + receiving.completions;
    let synchronous = accepting.synchronous
        + connecting.synchronous
        + reading.synchronous
        + writing.synchronous
        + echoing.synchronous
        + receiving.synchronous;
    assert!(synchronous > 0, "skip-success path must actually run");
    // The cancellation packet was drained before any socket/storage is released.
    drop(reading);
    drop(echoing);
    drop(server);
    let mut batch = [Entry::default(); 8];
    assert_eq!(
        port.wait(Some(Duration::from_millis(10)), false, &mut batch)?,
        Wait::Timeout,
        "duplicate packets after synchronous completion/cancel"
    );
    Ok(TcpStats {
        accepted: 1,
        connected: 1,
        bytes_echoed: echoed,
        completions,
        synchronous,
        cancelled: 1,
        closed_after_cancel: true,
    })
}

fn read_exact(port: &Port, op: &mut Operation, mut out: &mut [u8]) -> io::Result<()> {
    while !out.is_empty() {
        // SAFETY: nonblocking socket and exclusive initialized output slice.
        // https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-recv
        let n = unsafe {
            recv(
                op.endpoint.raw() as usize,
                out.as_mut_ptr(),
                out.len() as i32,
                0,
            )
        };
        if n == SOCKET_ERROR {
            let error = wsa_error();
            if error != WSAEWOULDBLOCK {
                return Err(io::Error::from_raw_os_error(error));
            }
            zero_read(op)?;
            drain(port, &mut [op])?;
            if op.result.map(|r| r.0) != Some(0) {
                return Err(io::Error::other("read probe failed"));
            }
        } else if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "TCP closed"));
        } else {
            out = &mut out[n as usize..];
        }
    }
    Ok(())
}
