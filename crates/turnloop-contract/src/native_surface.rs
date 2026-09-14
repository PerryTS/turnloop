//! Shared native surface contracts; backends use identical assertions.
use std::time::Duration;
use turnloop::{backend::Backend, *};

/// Connect and accept local IPC, checking both operation tokens.
pub fn pipe_pair<B: Backend>(l: &mut Driver<B>, name: &PipeName) -> (Handle, Handle, Handle) {
    let listener = l.pipe_listen(name, &ListenOpts::default()).expect("pipe listen");
    let accept = l.accept(listener, Token(1)).expect("pipe accept");
    let client = l.pipe_connect(name, Token(2)).expect("pipe connect");
    let until = l.now() + Duration::from_secs(3);
    let mut out = Completions::default();
    let mut conn = None;
    let mut connected = false;
    while conn.is_none() || !connected {
        assert!(l.now() < until, "local connect timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Connected => { assert_eq!(c.token, Token(2)); connected = true; }
                OpResult::PipeAccepted { conn: h } => { assert_eq!(c.op, Some(accept)); conn = Some(h); }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    (listener, client, conn.expect("accepted pipe"))
}
/// Write and read all bytes, allowing partial reads and validating completions.
pub fn transfer<B: Backend>(l: &mut Driver<B>, a: Handle, b: Handle, bytes: &[u8]) {
    l.write(a, WriteBuf::Owned(bytes.to_vec()), Token(11)).expect("write");
    l.read(b, ReadBuf::Pooled, Token(12)).expect("read");
    let until = l.now() + Duration::from_secs(3);
    let mut out = Completions::default();
    let mut read = Vec::new();
    let mut writes = 0;
    while read.len() < bytes.len() || writes == 0 {
        assert!(l.now() < until, "transfer timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease: Some(b) } => {
                    assert_eq!(n, b.as_slice().len()); assert!(n > 0);
                    read.extend_from_slice(b.as_slice());
                    if read.len() < bytes.len() { l.read(c.handle.expect("reader"), ReadBuf::Pooled, c.token).expect("continue read"); }
                }
                OpResult::Wrote(n) => { assert_eq!(n, bytes.len()); writes += 1; }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(writes, 1); assert_eq!(read, bytes);
}
/// Local IPC echo and SCM_RIGHTS/Windows-equivalent ownership transfer.
pub fn ipc<B: Backend>(name: &PipeName) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (_, a, b) = pipe_pair(&mut l, name);
    transfer(&mut l, a, b, b"local request");
    transfer(&mut l, b, a, b"local response");
    let (_, tx, rx) = crate::pair(&mut l);
    l.send_handle(a, tx, Token(20)).expect("send handle");
    l.recv_handle(b, Token(21)).expect("receive handle");
    // Sending must keep its own descriptor until the operation completes.
    l.close(tx, Token(22)).expect("close original");
    let mut received = None;
    let mut sent = 0;
    let mut closed = 0;
    let until = l.now() + Duration::from_secs(3);
    let mut out = Completions::with_capacity(1);
    while received.is_none() || sent == 0 || closed == 0 {
        assert!(l.now() < until, "handle transfer timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::HandleSent => { assert_eq!(c.token, Token(20)); sent += 1; }
                OpResult::HandleReceived { handle } => { assert_eq!(c.token, Token(21)); received = Some(handle); }
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((sent, closed), (1, 1));
    let passed = received.expect("received transport");
    let detached = l.detach(passed).expect("detach received socket");
    let attached = l.attach(detached, Token(30)).expect("attach received socket");
    transfer(&mut l, attached, rx, b"passed socket remains usable");
}
