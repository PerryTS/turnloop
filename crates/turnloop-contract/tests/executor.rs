#![cfg(all(
    feature = "executor",
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    )
))]
use futures_io::{AsyncRead, AsyncWrite};
use std::{
    cell::Cell,
    future::{Future, poll_fn},
    io::{Read, Write},
    pin::Pin,
    rc::Rc,
    time::{Duration, Instant},
};
use turnloop::{executor::LocalExecutor, *};

#[test]
fn executor_echoes_64_real_connections() {
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let listener = ex
        .driver()
        .tcp_listen(
            "127.0.0.1:0".parse().expect("address"),
            &ListenOpts::default(),
        )
        .expect("listen");
    let addr = ex.driver().local_addr(listener).expect("address");
    let peers: Vec<_> = (0..64)
        .map(|i| {
            std::thread::spawn(move || {
                let mut stream = std::net::TcpStream::connect(addr).expect("peer connect");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("timeout");
                let data = [i as u8; 257];
                stream.write_all(&data).expect("peer send");
                let mut echoed = [0; 257];
                stream.read_exact(&mut echoed).expect("peer echo");
                assert_eq!(echoed, data);
                257
            })
        })
        .collect();
    let h = ex.handle();
    let completed = Rc::new(Cell::new(0));
    let count = completed.clone();
    let server = ex
        .spawn_local(async move {
            let mut tasks = Vec::new();
            for _ in 0..64 {
                let mut stream = h.accept(listener).await.expect("accept");
                let count = count.clone();
                tasks.push(
                    h.spawn_local(async move {
                        let mut bytes = [0; 257];
                        let mut read = 0;
                        while read < bytes.len() {
                            let n = poll_fn(|cx| {
                                Pin::new(&mut stream).poll_read(cx, &mut bytes[read..])
                            })
                            .await
                            .expect("server read");
                            assert!(n > 0);
                            read += n;
                        }
                        let mut wrote = 0;
                        while wrote < bytes.len() {
                            wrote +=
                                poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, &bytes[wrote..]))
                                    .await
                                    .expect("server write");
                        }
                        count.set(count.get() + 1);
                    })
                    .expect("spawn connection"),
                );
            }
            for task in tasks {
                task.await;
            }
        })
        .expect("spawn server");
    let until = Instant::now() + Duration::from_secs(10);
    while !server.is_finished() {
        assert!(Instant::now() < until, "server timeout");
        ex.turn(Timeout::Until(until)).expect("executor turn");
    }
    assert_eq!(completed.get(), 64);
    assert_eq!(
        peers
            .into_iter()
            .map(|p| p.join().expect("peer"))
            .sum::<usize>(),
        64 * 257
    );
    ex.driver()
        .close(listener, Token(0))
        .expect("close listener");
}
#[test]
fn sleep_timeout_and_drop_cancel_pending_io() {
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let (_, a, b) = turnloop_contract::pair(&mut ex.driver());
    let h = ex.handle();
    let mut stream = h.io(b);
    let touched = Rc::new(Cell::new(false));
    let flag = touched.clone();
    let blocked = ex
        .spawn_local(async move {
            let mut bytes = [0; 64];
            let _ = poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, &mut bytes)).await;
            flag.set(true);
        })
        .expect("read task");
    assert_eq!(ex.run_ready(), 1, "read future must actually submit");
    drop(blocked);
    ex.run_ready();
    for _ in 0..3 {
        ex.turn(Timeout::Now).expect("drain cancellation");
    }
    assert!(!touched.get());
    // The peer sees EOF: dropping the task closed its actual socket.
    ex.driver()
        .read(a, ReadBuf::Pooled, Token(50))
        .expect("peer EOF read");
    let mut out = Completions::default();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(Instant::now() < until);
        ex.driver()
            .turn(Timeout::Until(until), &mut out)
            .expect("peer turn");
        if !out.is_empty() {
            assert!(out.iter().any(|c| matches!(c.result, OpResult::Eof)));
            break;
        }
    }
    let samples = Rc::new(Cell::new(0));
    let count = samples.clone();
    let timers = ex
        .spawn_local(async move {
            for micros in [500, 2_000, 10_000] {
                let start = Instant::now();
                h.sleep(Duration::from_micros(micros)).await.expect("sleep");
                assert!(start.elapsed() >= Duration::from_micros(micros));
                assert!(start.elapsed() < Duration::from_millis(100));
                count.set(count.get() + 1);
            }
            let start = Instant::now();
            let error = h
                .timeout(Duration::from_millis(2), std::future::pending::<()>())
                .await
                .expect_err("timeout");
            assert_eq!(error.kind, ErrorKind::TimedOut);
            assert!(start.elapsed() >= Duration::from_millis(2));
            count.set(count.get() + 1);
        })
        .expect("timer task");
    let until = Instant::now() + Duration::from_secs(3);
    while !timers.is_finished() {
        assert!(Instant::now() < until);
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    assert_eq!(samples.get(), 4);
}
#[test]
fn pending_future_buffers_may_move_and_shrink() {
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let (_, a, b) = turnloop_contract::pair(&mut ex.driver());
    let mut stream = ex.handle().io(b);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    let mut original = vec![0; 32];
    assert!(
        Pin::new(&mut stream)
            .poll_read(&mut cx, &mut original)
            .is_pending()
    );
    drop(original); // A readiness adapter must never retain this caller pointer.
    ex.driver()
        .write(a, WriteBuf::Owned(vec![42; 32]), Token(0))
        .expect("send");
    ex.turn(Timeout::After(Duration::from_secs(1)))
        .expect("complete");
    let mut actual = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while actual.len() < 32 {
        assert!(Instant::now() < deadline, "retained read bytes missing");
        let mut small = [0; 3];
        match Pin::new(&mut stream).poll_read(&mut cx, &mut small) {
            std::task::Poll::Ready(Ok(n)) => {
                assert!(n > 0);
                actual.extend_from_slice(&small[..n]);
            }
            std::task::Poll::Pending => {
                ex.turn(Timeout::After(Duration::from_millis(100)))
                    .expect("read progress");
            }
            std::task::Poll::Ready(Err(e)) => panic!("read failed: {e}"),
        }
    }
    assert_eq!(actual, [42; 32]);
    // Exercise a !Unpin future through the timeout projection as well.
    let mut timer = Box::pin(ex.handle().timeout(Duration::ZERO, async { 7 }));
    assert_eq!(timer.as_mut().poll(&mut cx), std::task::Poll::Ready(Ok(7)));
}
