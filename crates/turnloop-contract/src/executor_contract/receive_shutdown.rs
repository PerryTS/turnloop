//! Receive half-close must discard adapter state without revoking native buffer
//! ownership or affecting the opposite direction.
use super::*;

pub fn pending_read_and_live_write<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (listener, a, b) = crate::pair(&mut ex.driver());
    let h = ex.handle();
    let mut socket = h.io(a);
    let mut peer = h.io(b);
    let completed = Rc::new(Cell::new(0));
    let done = completed.clone();
    let sender = ex
        .spawn_local(async move {
            let mut bytes = [0xcc; 32];
            poll_fn(|cx| {
                assert!(
                    Pin::new(&mut socket).poll_read(cx, &mut bytes).is_pending(),
                    "read must be submitted before shutdown"
                );
                std::task::Poll::Ready(())
            })
            .await;
            assert!(!socket.is_read_shut());
            // Keep a write submitted while receive shutdown cancels the other lane.
            poll_fn(|cx| Pin::new(&mut socket).poll_write(cx, b"write survives"))
                .await
                .expect("write accepted");
            socket.shutdown_receive().expect("receive shutdown");
            socket.shutdown_receive().expect("idempotent shutdown");
            assert!(socket.is_read_shut());
            assert!(!socket.is_write_shut());
            for _ in 0..3 {
                assert_eq!(
                    poll_fn(|cx| Pin::new(&mut socket).poll_read(cx, &mut bytes))
                        .await
                        .expect("read EOF"),
                    0
                );
                assert_eq!(bytes, [0xcc; 32], "cancelled read exposed stale bytes");
            }
            poll_fn(|cx| Pin::new(&mut socket).poll_flush(cx))
                .await
                .expect("flush after receive shutdown");
            poll_fn(|cx| socket.poll_shutdown(cx))
                .await
                .expect("independent send shutdown");
            done.set(done.get() + 1);
        })
        .expect("sender task");
    let done = completed.clone();
    let receiver = ex
        .spawn_local(async move {
            let mut received = Vec::new();
            loop {
                let mut bytes = [0; 4];
                let n = poll_fn(|cx| Pin::new(&mut peer).poll_read(cx, &mut bytes))
                    .await
                    .expect("peer read");
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&bytes[..n]);
            }
            assert_eq!(received, b"write survives");
            done.set(done.get() + 1);
        })
        .expect("receiver task");
    let until = ex.driver().now() + Duration::from_secs(5);
    while !sender.is_finished() || !receiver.is_finished() {
        assert!(ex.driver().now() < until, "half-close stalled");
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    assert_eq!(
        completed.get(),
        2,
        "both directions must execute assertions"
    );
    ex.driver()
        .close(listener, Token(0))
        .expect("listener close");
    for _ in 0..4 {
        ex.turn(Timeout::Now).expect("terminal cancellation drain");
    }
}

pub fn buffered_read_is_discarded<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (listener, a, b) = crate::pair(&mut ex.driver());
    let h = ex.handle();
    let mut socket = h.io(a);
    let mut peer = h.io(b);
    let done = Rc::new(Cell::new(false));
    let mark = done.clone();
    let reader = ex
        .spawn_local(async move {
            let mut scratch = [0; 64];
            poll_fn(|cx| {
                assert!(
                    Pin::new(&mut socket)
                        .poll_read(cx, &mut scratch)
                        .is_pending()
                );
                std::task::Poll::Ready(())
            })
            .await;
            let mut byte = [0];
            assert_eq!(
                poll_fn(|cx| Pin::new(&mut socket).poll_read(cx, &mut byte))
                    .await
                    .expect("partial read"),
                1
            );
            assert_eq!(byte, [b'd']);
            socket.shutdown_receive().expect("shutdown buffered input");
            assert_eq!(
                poll_fn(|cx| Pin::new(&mut socket).poll_read(cx, &mut scratch))
                    .await
                    .expect("discarded tail"),
                0
            );
            poll_fn(|cx| Pin::new(&mut socket).poll_write(cx, b"ack"))
                .await
                .expect("opposite direction still usable");
            poll_fn(|cx| Pin::new(&mut socket).poll_flush(cx))
                .await
                .expect("flush");
            poll_fn(|cx| socket.poll_shutdown(cx)).await.expect("FIN");
            mark.set(true);
        })
        .expect("reader task");
    // Start the read before spawning the peer writer, so the internal completion
    // is larger than the one-byte buffer used on the subsequent poll.
    ex.run_ready();
    let peer_done = Rc::new(Cell::new(false));
    let mark = peer_done.clone();
    let writer = ex
        .spawn_local(async move {
            poll_fn(|cx| Pin::new(&mut peer).poll_write(cx, b"discard this tail"))
                .await
                .expect("peer write");
            poll_fn(|cx| Pin::new(&mut peer).poll_flush(cx))
                .await
                .expect("peer flush");
            let mut got = Vec::new();
            loop {
                let mut bytes = [0; 8];
                let n = poll_fn(|cx| Pin::new(&mut peer).poll_read(cx, &mut bytes))
                    .await
                    .expect("ack read");
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&bytes[..n]);
            }
            assert_eq!(got, b"ack");
            mark.set(true);
        })
        .expect("writer task");
    let until = ex.driver().now() + Duration::from_secs(5);
    while !reader.is_finished() || !writer.is_finished() {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    assert!(done.get() && peer_done.get());
    ex.driver()
        .close(listener, Token(0))
        .expect("listener close");
}

pub fn unsupported_udp_preserves_pending_read<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let handle = ex
        .driver()
        .udp_bind("127.0.0.1:0".parse().expect("address"), &UdpOpts::default())
        .expect("UDP");
    let address = ex.driver().local_addr(handle).expect("UDP address");
    let mut udp = ex.handle().udp(handle, address);
    let done = Rc::new(Cell::new(false));
    let mark = done.clone();
    let task = ex
        .spawn_local(async move {
            let mut bytes = [0; 16];
            poll_fn(|cx| {
                assert!(Pin::new(&mut udp).poll_read(cx, &mut bytes).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            assert_eq!(
                udp.shutdown_receive()
                    .expect_err("UDP has no receive half-close")
                    .kind(),
                std::io::ErrorKind::Unsupported
            );
            assert!(!udp.is_read_shut());
            poll_fn(|cx| Pin::new(&mut udp).poll_write(cx, b"untouched"))
                .await
                .expect("loopback send");
            poll_fn(|cx| Pin::new(&mut udp).poll_flush(cx))
                .await
                .expect("flush");
            let n = poll_fn(|cx| Pin::new(&mut udp).poll_read(cx, &mut bytes))
                .await
                .expect("pending read survives error");
            assert_eq!(&bytes[..n], b"untouched");
            mark.set(true);
        })
        .expect("task");
    let until = ex.driver().now() + Duration::from_secs(5);
    while !task.is_finished() {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    assert!(done.get());
}
