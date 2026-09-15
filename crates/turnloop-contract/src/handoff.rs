//! Handing a transport back to the host (DESIGN §5a; issue #35).
//!
//! These are the loop-side halves of the contract: which requests are refused,
//! and what the loop still knows about a transport once the host owns it. The
//! halves that need the descriptor itself — bytes flowing over it after the
//! handoff, a TLS handshake on it, the Windows completion-port rules — are
//! platform code and live in `tests/handoff.rs`, because a descriptor cannot be
//! used through a generic `B::Detached`.
use super::*;

/// The error kind of a refused handoff. `expect_err` needs `Debug`, which a
/// backend's transport type is not required to implement.
fn refused<B: Backend>(result: Result<B::Detached>, what: &str) -> ErrorKind {
    match result {
        Ok(_) => panic!("{what} was handed out"),
        Err(e) => e.kind,
    }
}

/// Drive the loop until every handle is gone, so a fixture cannot leak one.
fn close_all<B: Backend>(l: &mut Driver<B>, handles: &[Handle]) {
    let mut out = Completions::default();
    for (i, h) in handles.iter().enumerate() {
        if l.close(*h, Token(900 + i as u64)).is_err() {
            continue;
        }
    }
    let until = l.now() + Duration::from_secs(5);
    while l.alive() {
        assert!(l.now() < until, "close never completed");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        out.drain();
    }
}

/// A handle with work in flight is never handed out, and cancellation is never
/// silent: the refusal is `WouldBlock`, the host turns, collects the terminal
/// completion it is owed, and only then owns the transport.
pub fn pending_operations_refuse_handoff<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = pair(&mut l);
    let mut bytes = [0x3c; 24];
    // SAFETY: the region stays fixed and untouched until Cancelled is delivered.
    let provided = unsafe { IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len()) };
    let read = l
        .read(conn, ReadBuf::Provided(provided), Token(1))
        .expect("pending read");
    assert_eq!(
        refused::<B>(l.detach(conn), "a busy handle"),
        ErrorKind::WouldBlock,
        "a handle with an outstanding operation must not be handed out"
    );
    // The refusal must not have cancelled anything behind the host's back either:
    // the identity is still live and still refuses, turn after turn.
    let mut out = Completions::default();
    assert_eq!(
        refused::<B>(l.detach(conn), "a busy handle"),
        ErrorKind::WouldBlock
    );
    l.turn(Timeout::Now, &mut out).expect("cancel turn");
    assert_eq!(out.len(), 1, "exactly one terminal completion is owed");
    assert_eq!(out[0].op, Some(read));
    assert!(matches!(out[0].result, OpResult::Cancelled));
    assert_eq!(bytes, [0x3c; 24], "the loop wrote into a cancelled buffer");
    out.drain();

    let transport = l.detach(conn).expect("quiescent handoff");
    assert_eq!(
        refused::<B>(l.detach(conn), "an already handed-off transport"),
        ErrorKind::NotFound,
        "a transport cannot be handed out twice"
    );
    assert_eq!(
        l.raw_transport(conn).expect_err("gone").kind,
        ErrorKind::NotFound
    );
    assert!(
        l.read(conn, ReadBuf::Pooled, Token(2)).is_err(),
        "the loop still accepts operations on a transport it gave away"
    );
    // Nothing for that handle can arrive any more, however long the host turns.
    let until = l.now() + Duration::from_millis(60);
    while l.now() < until {
        l.turn(Timeout::After(Duration::from_millis(10)), &mut out)
            .expect("idle turn");
        assert_eq!(out.len(), 0, "a handed-off transport produced {out:?}");
    }
    drop(transport);
    close_all(&mut l, &[client, server]);
}

/// Close is the other owner of a transport's end of life; the two never overlap.
pub fn closing_and_closed_handles_refuse_handoff<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = pair(&mut l);
    l.read_start(conn, Token(1)).expect("multishot read");
    l.close(conn, Token(2)).expect("close");
    assert!(l.is_closing(conn), "close has not finished yet");
    assert_eq!(
        refused::<B>(l.detach(conn), "a closing transport"),
        ErrorKind::InvalidInput,
        "a closing transport must not be handed out"
    );
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut closed = false;
    while !closed {
        assert!(l.now() < until, "close never completed");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            closed |= matches!(c.result, OpResult::Closed);
        }
    }
    assert_eq!(
        refused::<B>(l.detach(conn), "a closed transport"),
        ErrorKind::NotFound
    );
    assert_eq!(
        l.raw_transport(conn).expect_err("closed handle").kind,
        ErrorKind::NotFound
    );
    close_all(&mut l, &[client, server]);
}

/// Both ends of an accept are ordinary transports: the listener and the
/// connection it produced are each handed over, and the loop keeps nothing.
pub fn listeners_and_accepted_sockets_are_handed_off<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = pair(&mut l);
    let accepted = l.detach(conn).expect("accepted socket");
    let listener = l.detach(server).expect("listener");
    assert!(
        l.local_addr(server).is_err() && l.local_addr(conn).is_err(),
        "the loop still answers for transports it gave away"
    );
    // The client is the only handle left, so liveness proves the other two are
    // gone from the core's accounting and not merely unregistered natively.
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("idle turn");
    assert_eq!(out.len(), 0);
    assert!(l.alive(), "the remaining client still counts");
    close_all(&mut l, &[client]);
    assert!(!l.alive());
    drop(accepted);
    drop(listener);
}

/// Reporting a descriptor is not owning it: the value is stable while the loop
/// holds the transport, and every non-transport handle says `Unsupported`.
pub fn raw_transport_reports_live_transports<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = pair(&mut l);
    let first = l.raw_transport(conn).expect("live transport");
    assert_eq!(first, l.raw_transport(conn).expect("stable"));
    assert_ne!(
        first,
        l.raw_transport(client).expect("client"),
        "two live transports reported the same identity"
    );
    assert_eq!(
        l.raw_transport(server).expect("listener"),
        l.raw_transport(server).expect("stable listener")
    );
    // Reporting is read-only: the socket is still fully usable afterwards.
    l.write(conn, WriteBuf::Owned(b"report".to_vec()), Token(1))
        .expect("write");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut wrote = 0;
    while wrote == 0 {
        assert!(l.now() < until, "write after reporting never completed");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            if let OpResult::Wrote(n) = c.result {
                assert_eq!(n, 6);
                wrote += 1;
            }
        }
    }
    assert_eq!(l.raw_transport(conn).expect("still live"), first);

    let timer = l
        .timer(l.now() + Duration::from_secs(30), None, Token(2))
        .expect("timer");
    assert_eq!(
        l.raw_transport(timer).expect_err("timer").kind,
        ErrorKind::Unsupported,
        "a timer is not a transport and has no descriptor"
    );
    assert_eq!(
        refused::<B>(l.detach(timer), "a timer"),
        ErrorKind::InvalidInput
    );
    close_all(&mut l, &[timer, client, conn, server]);
}

/// A platform with no descriptor to give says so, rather than inventing one.
/// WASI 0.2 and 0.3 sockets are component-model resource handles and web
/// resources are host objects: neither has an identity a host could act on.
pub fn handoff_is_unsupported<B: Backend>() {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let (server, client, conn) = pair(&mut l);
    for h in [server, client, conn] {
        assert_eq!(
            refused::<B>(l.detach(h), "a transport on a platform without descriptors"),
            ErrorKind::Unsupported
        );
        assert_eq!(
            l.raw_transport(h).expect_err("identity").kind,
            ErrorKind::Unsupported
        );
    }
    // The refusals changed nothing: the connection still works.
    l.write(conn, WriteBuf::Owned(b"still here".to_vec()), Token(1))
        .expect("write");
    l.read(client, ReadBuf::Pooled, Token(2)).expect("read");
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    let mut read = 0;
    while read == 0 {
        assert!(l.now() < until, "refused handoff broke the connection");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            if let OpResult::Read {
                n,
                lease: Some(data),
            } = c.result
            {
                assert_eq!(&data.as_slice()[..n], b"still here");
                read += 1;
            }
        }
    }
    close_all(&mut l, &[client, conn, server]);
}
