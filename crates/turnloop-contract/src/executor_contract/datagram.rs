//! Real UDP packet boundaries, addresses and cancellation through the executor.
use super::*;
use std::net::SocketAddr;

fn bind<B: backend::Backend>(
    ex: &LocalExecutor<B>,
    address: &str,
) -> (AsyncDatagram<B>, SocketAddr) {
    let handle = ex
        .driver()
        .udp_bind(address.parse().expect("address"), &UdpOpts::default())
        .expect("bind");
    let address = ex.driver().local_addr(handle).expect("local address");
    (ex.handle().datagram(handle), address)
}
fn drive<B: backend::Backend>(ex: &mut LocalExecutor<B>, job: &JoinHandle<()>, done: &Cell<bool>) {
    let until = ex.driver().now() + Duration::from_secs(5);
    while !job.is_finished() {
        assert!(ex.driver().now() < until, "datagram task stalled");
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    assert!(done.get(), "packet assertions must execute");
    for _ in 0..4 {
        ex.turn(Timeout::Now).expect("cancel drain");
    }
}

pub fn addresses_empty_and_full_packets<B: backend::Backend>() {
    for address in ["127.0.0.1:0", "[::1]:0"] {
        let mut ex = LocalExecutor::<B>::with_config(
            Config::default(),
            ExecutorConfig {
                operations: 16,
                buffer_size: 65_535,
                ..ExecutorConfig::default()
            },
        )
        .expect("executor");
        let (mut receiver, destination) = bind(&ex, address);
        let (mut first, first_addr) = bind(&ex, address);
        let (mut second, second_addr) = bind(&ex, address);
        // macOS's default UDP send high-water mark is smaller than a full
        // packet. Test the adapter capacity with an explicitly sufficient queue.
        ex.driver()
            .set_option(second.handle(), SocketOption::SendBufferSize(131_072))
            .expect("full packet send buffer");
        let done = Rc::new(Cell::new(false));
        let mark = done.clone();
        let job = ex
            .spawn_local(async move {
                let mut buffer = vec![0xcc; 65_535];
                for (use_second, source, payload) in [
                    (false, first_addr, Vec::new()),
                    (true, second_addr, vec![71; 65_507]),
                    (false, first_addr, b"after full packet".to_vec()),
                ] {
                    let sender = if use_second { &mut second } else { &mut first };
                    assert_eq!(
                        poll_fn(|cx| sender.poll_send_to(cx, &payload, destination))
                            .await
                            .expect("send"),
                        payload.len()
                    );
                    poll_fn(|cx| sender.poll_flush(cx)).await.expect("flush");
                    let (n, from) = poll_fn(|cx| receiver.poll_recv_from(cx, &mut buffer))
                        .await
                        .expect("receive");
                    assert_eq!(from, source);
                    assert_eq!(&buffer[..n], payload);
                }
                poll_fn(|cx| receiver.poll_close(cx)).await.expect("close");
                let error = poll_fn(|cx| receiver.poll_recv_from(cx, &mut buffer))
                    .await
                    .expect_err("closed receive");
                assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
                mark.set(true);
            })
            .expect("task");
        drive(&mut ex, &job, &done);
    }
}

pub fn receive_cancellation_and_truncation<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (mut receiver, destination) = bind(&ex, "127.0.0.1:0");
    let (mut sender, source) = bind(&ex, "127.0.0.1:0");
    let h = ex.handle();
    let done = Rc::new(Cell::new(false));
    let mark = done.clone();
    let job = ex
        .spawn_local(async move {
            let mut old = [0xcc; 1];
            poll_fn(|cx| {
                assert!(receiver.poll_recv_from(cx, &mut old).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            receiver.cancel_receive();
            // Let native cancellation complete before the peer transmits. This test
            // does not demand recovery of a packet already consumed by cancellation.
            h.sleep(Duration::from_millis(1))
                .await
                .expect("cancel turn");
            poll_fn(|cx| sender.poll_send_to(cx, b"abcdef", destination))
                .await
                .expect("send");
            poll_fn(|cx| sender.poll_flush(cx)).await.expect("flush");
            let mut short = [0; 2];
            assert_eq!(
                poll_fn(|cx| receiver.poll_recv_from(cx, &mut short))
                    .await
                    .expect("truncated packet"),
                (2, source)
            );
            assert_eq!(&short, b"ab");
            assert_eq!(old, [0xcc]);
            // A second packet must not be confused with the first one's discarded tail.
            poll_fn(|cx| {
                assert!(receiver.poll_recv_from(cx, &mut old).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            poll_fn(|cx| sender.poll_send_to(cx, b"next", destination))
                .await
                .expect("next send");
            poll_fn(|cx| sender.poll_flush(cx))
                .await
                .expect("next flush");
            let mut replacement = [0; 8];
            assert_eq!(
                poll_fn(|cx| receiver.poll_recv_from(cx, &mut replacement))
                    .await
                    .expect("replacement buffer"),
                (4, source)
            );
            assert_eq!(&replacement[..4], b"next");
            assert_eq!(old, [0xcc]);
            poll_fn(|cx| {
                assert!(receiver.poll_recv_from(cx, &mut old).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            drop(receiver); // Pending receive ownership must survive through native cancellation.
            mark.set(true);
        })
        .expect("task");
    drive(&mut ex, &job, &done);
}

pub fn oversized_and_pending_send_are_atomic<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::with_config(
        Config::default(),
        ExecutorConfig {
            operations: 16,
            buffer_size: 8,
            ..ExecutorConfig::default()
        },
    )
    .expect("executor");
    let (mut receiver, destination) = bind(&ex, "127.0.0.1:0");
    let (mut sender, source) = bind(&ex, "127.0.0.1:0");
    let (mut untouched, other) = bind(&ex, "127.0.0.1:0");
    let h = ex.handle();
    let done = Rc::new(Cell::new(false));
    let mark = done.clone();
    let job = ex
        .spawn_local(async move {
            let error = poll_fn(|cx| sender.poll_send_to(cx, b"too-large", destination))
                .await
                .expect_err("reject oversize");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(
                poll_fn(|cx| sender.poll_send_to(cx, b"first", destination))
                    .await
                    .expect("first"),
                5
            );
            poll_fn(|cx| {
                assert!(sender.poll_send_to(cx, b"discard", other).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            assert_eq!(
                poll_fn(|cx| sender.poll_send_to(cx, b"second", destination))
                    .await
                    .expect("replacement send"),
                6
            );
            poll_fn(|cx| sender.poll_flush(cx)).await.expect("flush");
            let mut bytes = [0; 8];
            for expected in [b"first".as_slice(), b"second".as_slice()] {
                let (n, from) = poll_fn(|cx| receiver.poll_recv_from(cx, &mut bytes))
                    .await
                    .expect("receive");
                assert_eq!(from, source);
                assert_eq!(&bytes[..n], expected);
            }
            // A Pending send must not retain either that call's bytes or destination.
            assert!(
                h.timeout(
                    Duration::from_millis(10),
                    poll_fn(|cx| untouched.poll_recv_from(cx, &mut bytes))
                )
                .await
                .is_err()
            );
            untouched.cancel_receive();
            mark.set(true);
        })
        .expect("task");
    drive(&mut ex, &job, &done);
}
