#![cfg(any(unix, windows))]
//! Independent native probe: detach transfers ownership out of Turnloop, so the
//! OS result cannot be supplied by the adapter's read-shut flag.
use std::{
    io::{Read, Write},
    time::Duration,
};
use turnloop::*;

#[test]
fn native_receive_shutdown_is_observable_after_detach() {
    let mut driver = Loop::new(Config::default()).expect("loop");
    let (listener, a, b) = turnloop_contract::pair(&mut driver);
    assert_eq!(
        driver
            .shutdown_receive(listener)
            .expect_err("listener cannot half-close")
            .kind,
        ErrorKind::Unsupported
    );
    driver.shutdown_receive(a).expect("native receive shutdown");
    let transport = driver.detach(a).expect("quiescent transport");
    #[cfg(unix)]
    let mut owned = std::net::TcpStream::from(transport.into_fd());
    #[cfg(windows)]
    let mut owned = std::net::TcpStream::from(transport.into_socket().expect("TCP socket"));
    owned.set_nonblocking(false).expect("owned blocking stream");
    owned
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("read deadline");
    owned
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("write deadline");
    match owned.read(&mut [0]) {
        Ok(0) => (),
        #[cfg(windows)]
        Err(error)
            if error.raw_os_error()
                == Some(windows_sys::Win32::Networking::WinSock::WSAESHUTDOWN) =>
        {
            ()
        }
        other => panic!("native receive direction remained open with peer still live: {other:?}"),
    }
    owned
        .write_all(b"native send survived")
        .expect("native send after receive shutdown");
    driver
        .read(b, ReadBuf::Pooled, Token(31))
        .expect("peer read");
    let until = driver.now() + Duration::from_secs(3);
    let mut out = Completions::default();
    let mut received = Vec::new();
    while received.len() < b"native send survived".len() {
        assert!(driver.now() < until);
        driver
            .turn(Timeout::Until(until), &mut out)
            .expect("read turn");
        for completion in out.drain() {
            assert_eq!(completion.token, Token(31));
            match completion.result {
                OpResult::Read {
                    n,
                    lease: Some(lease),
                } => {
                    received.extend_from_slice(&lease.as_slice()[..n]);
                }
                result => panic!("unexpected peer result: {result:?}"),
            }
        }
        if received.len() < b"native send survived".len() {
            driver
                .read(b, ReadBuf::Pooled, Token(31))
                .expect("remaining bytes");
        }
    }
    assert_eq!(received, b"native send survived");
    driver.close(b, Token(32)).expect("peer close");
    assert_eq!(
        driver.shutdown_receive(b).expect_err("closing handle").kind,
        ErrorKind::InvalidInput
    );
    driver.close(listener, Token(33)).expect("listener close");
}
