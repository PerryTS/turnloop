//! Backend-generic executor conformance scenarios.
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

pub fn executor_echoes_64_real_connections<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
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
                        poll_fn(|cx| Pin::new(&mut stream).poll_flush(cx))
                            .await
                            .expect("server flush");
                        count.set(count.get() + 1);
                    })
                    .expect("spawn connection"),
                );
            }
            for task in tasks {
                task.await.expect("join connection");
            }
        })
        .expect("spawn server");
    let until = ex.driver().now() + Duration::from_secs(10);
    while !server.is_finished() {
        assert!(ex.driver().now() < until, "server timeout");
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
pub fn sleep_timeout_and_drop_cancel_pending_io<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (_, a, b) = crate::pair(&mut ex.driver());
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
    let until = ex.driver().now() + Duration::from_secs(2);
    loop {
        assert!(ex.driver().now() < until);
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
    let until = ex.driver().now() + Duration::from_secs(3);
    while !timers.is_finished() {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    assert_eq!(samples.get(), 4);
}
pub fn pending_future_buffers_may_move_and_shrink<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (_, a, b) = crate::pair(&mut ex.driver());
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

/// UDP and stdio adapters actually transfer bytes; explicit task cancellation
/// resolves its JoinHandle, including cancellation before the first task poll.
pub fn udp_stdio_and_join_cancel<B: backend::Backend>(program: &std::ffi::OsStr) {
    use turnloop::executor::JoinError;
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let a = ex
        .driver()
        .udp_bind("127.0.0.1:0".parse().expect("address"), &UdpOpts::default())
        .expect("UDP a");
    let b = ex
        .driver()
        .udp_bind("127.0.0.1:0".parse().expect("address"), &UdpOpts::default())
        .expect("UDP b");
    let a_addr = ex.driver().local_addr(a).expect("a address");
    let b_addr = ex.driver().local_addr(b).expect("b address");
    assert_ne!(a_addr, b_addr, "default UDP endpoints must be distinct");
    let mut a = ex.handle().udp(a, b_addr);
    let mut b = ex.handle().udp(b, a_addr);
    let h = ex.handle();
    let job = ex
        .spawn_local(async move {
            let n = poll_fn(|cx| Pin::new(&mut a).poll_write(cx, b"one UDP datagram"))
                .await
                .expect("UDP write");
            assert_eq!(n, 16);
            poll_fn(|cx| Pin::new(&mut a).poll_flush(cx))
                .await
                .expect("UDP flush");
            let mut bytes = [0; 64];
            let n = poll_fn(|cx| Pin::new(&mut b).poll_read(cx, &mut bytes))
                .await
                .expect("UDP read");
            assert_eq!(&bytes[..n], b"one UDP datagram");
            h.sleep(Duration::from_micros(500))
                .await
                .expect("mixed UDP timer");
        })
        .expect("UDP task");
    let until = ex.driver().now() + Duration::from_secs(3);
    while !job.is_finished() {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).expect("UDP turn");
    }
    let mut spec = ProcessSpec::new(program);
    spec.windows_hide = true;
    spec.args.push("copy".into());
    spec.stdio = [ProcessStdio::Pipe, ProcessStdio::Pipe, ProcessStdio::Null];
    let child = ex.driver().spawn(&spec, Token(1)).expect("stdio child");
    let mut stdin = ex.handle().io(child.stdin.expect("stdin"));
    let mut stdout = ex.handle().io(child.stdout.expect("stdout"));
    let job = ex
        .spawn_local(async move {
            let data = b"executor child stdio";
            assert_eq!(
                poll_fn(|cx| Pin::new(&mut stdin).poll_write(cx, data))
                    .await
                    .expect("stdin write"),
                data.len()
            );
            poll_fn(|cx| Pin::new(&mut stdin).poll_close(cx))
                .await
                .expect("stdin close");
            let mut bytes = [0; 64];
            let mut read = 0;
            while read < data.len() {
                let n = poll_fn(|cx| Pin::new(&mut stdout).poll_read(cx, &mut bytes[read..]))
                    .await
                    .expect("stdout read");
                assert!(n > 0);
                read += n;
            }
            assert_eq!(&bytes[..read], data);
        })
        .expect("stdio task");
    while !job.is_finished() {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until)).expect("stdio turn");
    }
    let mut cancelled = ex
        .spawn_local(std::future::pending::<()>())
        .expect("cancelled task");
    cancelled.cancel();
    ex.run_ready();
    assert!(cancelled.is_finished());
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert_eq!(
        Pin::new(&mut cancelled).poll(&mut cx),
        std::task::Poll::Ready(Err(JoinError::Cancelled))
    );
}

/// Dropping a ready accept future releases the accepted socket even when it was
/// already attached by the completion driver before its future was polled again.
pub fn drop_ready_accept<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let listener = ex
        .driver()
        .tcp_listen(
            "127.0.0.1:0".parse().expect("address"),
            &ListenOpts::default(),
        )
        .expect("listen");
    let addr = ex.driver().local_addr(listener).expect("address");
    let mut peer = std::net::TcpStream::connect(addr).expect("peer");
    peer.set_read_timeout(Some(Duration::from_secs(2)))
        .expect("peer timeout");
    let mut accept = ex.handle().accept(listener);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(Pin::new(&mut accept).poll(&mut cx).is_pending());
    let info = ex
        .turn(Timeout::After(Duration::from_secs(1)))
        .expect("accept turn");
    assert!(
        info.completions > 0,
        "the accepted descriptor must actually be attached"
    );
    drop(accept);
    ex.turn(Timeout::Now).expect("release abandoned accept");
    let mut byte = [0];
    assert_eq!(peer.read(&mut byte).expect("accepted socket closed"), 0);
    ex.driver()
        .close(listener, Token(0))
        .expect("close listener");
}

/// A Pending write consumes no caller bytes; replacing that slice cannot report
/// an earlier operation's count or emit bytes from the abandoned call.
pub fn pending_writes_may_replace_the_caller_slice<B: backend::Backend>() {
    use std::task::{Context, Poll, Waker};
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (_, a, b) = crate::pair(&mut ex.driver());
    let mut a = ex.handle().io(a);
    let mut b = ex.handle().io(b);
    let mut cx = Context::from_waker(Waker::noop());
    let mut first = vec![7; 32];
    let Poll::Ready(n) = Pin::new(&mut a).poll_write(&mut cx, &first) else {
        panic!("empty staging buffer")
    };
    assert_eq!(n.expect("accepted bytes"), first.len());
    first.fill(0);
    drop(first);
    assert!(
        Pin::new(&mut a)
            .poll_write(&mut cx, b"discarded pending slice")
            .is_pending()
    );
    let until = ex.driver().now() + Duration::from_secs(3);
    loop {
        assert!(ex.driver().now() < until);
        ex.turn(Timeout::Until(until))
            .expect("first write completion");
        if let Poll::Ready(n) = Pin::new(&mut a).poll_write(&mut cx, b"new") {
            assert_eq!(n.expect("replacement bytes"), 3);
            break;
        }
    }
    loop {
        match Pin::new(&mut a).poll_flush(&mut cx) {
            Poll::Ready(result) => {
                result.expect("both writes completed");
                break;
            }
            Poll::Pending => {
                assert!(ex.driver().now() < until);
                ex.turn(Timeout::Until(until)).expect("flush turn");
            }
        }
    }
    let Poll::Ready(closed) = Pin::new(&mut a).poll_close(&mut cx) else {
        panic!("flushed close")
    };
    closed.expect("close after flush");
    let mut actual = Vec::new();
    loop {
        let mut bytes = [0; 64];
        match Pin::new(&mut b).poll_read(&mut cx, &mut bytes) {
            Poll::Ready(result) => {
                let n = result.expect("peer read");
                if n == 0 {
                    break;
                }
                actual.extend_from_slice(&bytes[..n]);
            }
            Poll::Pending => {
                assert!(ex.driver().now() < until);
                ex.turn(Timeout::Until(until)).expect("read turn");
            }
        }
    }
    assert_eq!(actual.len(), 35);
    assert_eq!(&actual[..32], [7; 32]);
    assert_eq!(&actual[32..], b"new");
}

/// A host that owns the loop submits its own read alongside an executor-driven
/// one, and both complete: the executor wakes its future and hands the host's
/// completions back through `unclaimed` instead of dropping them. Executor-owned
/// closes are claimed, so the host sees only the tokens it issued.
pub fn host_operations_share_the_loop<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::new(Config::default()).expect("executor");
    let (host_server, host_client, host_conn) = crate::pair(&mut ex.driver());
    let (exec_server, exec_client, exec_conn) = crate::pair(&mut ex.driver());
    let h = ex.handle();
    let task = ex
        .spawn_local(async move {
            let mut writer = h.io(exec_client);
            let mut reader = h.io(exec_conn);
            let sent = b"executor bytes";
            let mut wrote = 0;
            while wrote < sent.len() {
                wrote += poll_fn(|cx| Pin::new(&mut writer).poll_write(cx, &sent[wrote..]))
                    .await
                    .expect("executor write");
            }
            poll_fn(|cx| Pin::new(&mut writer).poll_flush(cx))
                .await
                .expect("executor flush");
            let mut bytes = [0; 32];
            let mut read = 0;
            while read < sent.len() {
                let n = poll_fn(|cx| Pin::new(&mut reader).poll_read(cx, &mut bytes[read..]))
                    .await
                    .expect("executor read");
                assert!(n > 0, "early EOF");
                read += n;
            }
            bytes[..read].to_vec()
            // Both adapters drop here and close their handles internally.
        })
        .expect("spawn executor task");
    ex.driver()
        .read(host_conn, ReadBuf::Pooled, Token(10))
        .expect("host read");
    ex.driver()
        .write(
            host_client,
            WriteBuf::Owned(b"host bytes".to_vec()),
            Token(11),
        )
        .expect("host write");
    let mut host_read = Vec::new();
    let mut host_wrote = None;
    let mut unexpected = Vec::new();
    let mut out = Completions::default();
    let until = ex.driver().now() + Duration::from_secs(5);
    while !task.is_finished() || host_wrote.is_none() || host_read.len() < 10 {
        assert!(
            ex.driver().now() < until,
            "host read {:?}, host write {host_wrote:?}, task finished {}",
            host_read,
            task.is_finished()
        );
        ex.turn_into(Timeout::Until(until), &mut out)
            .expect("executor turn");
        for c in out.drain() {
            match (c.token, c.result) {
                (Token(10), OpResult::Read { lease: Some(b), .. }) => {
                    host_read.extend_from_slice(b.as_slice());
                    if host_read.len() < 10 {
                        ex.driver()
                            .read(host_conn, ReadBuf::Pooled, Token(10))
                            .expect("continue host read");
                    }
                }
                (Token(11), OpResult::Wrote(n)) => host_wrote = Some(n),
                (token, result) => unexpected.push(format!("{token:?} {result:?}")),
            }
        }
    }
    assert_eq!(host_read, b"host bytes");
    assert_eq!(host_wrote, Some(10));
    let mut task = task;
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(value) = Pin::new(&mut task).poll(&mut cx) else {
        panic!("finished task must be ready")
    };
    assert_eq!(value.expect("executor task"), b"executor bytes");
    // The adapters' own closes are the executor's; the host's closes, collected
    // here through plain `turn`, are the only ones handed back.
    for handle in [host_client, host_conn, host_server, exec_server] {
        ex.driver().close(handle, Token(20)).expect("host close");
    }
    let mut closed = 0;
    while ex.alive() {
        assert!(ex.driver().now() < until, "drain closes");
        ex.turn(Timeout::Until(until)).expect("close turn");
        for c in ex.unclaimed().drain() {
            match (c.token, c.result) {
                (Token(20), OpResult::Closed) => closed += 1,
                (token, result) => unexpected.push(format!("{token:?} {result:?}")),
            }
        }
    }
    assert_eq!(closed, 4, "every host close is handed back exactly once");
    assert!(unexpected.is_empty(), "foreign completions: {unexpected:?}");
}

/// The executor presets carry one request end to end: a lookup, a connect with
/// its deadline, a write, a half-close and a read to EOF, all under a request
/// timeout, then a second request on a fresh connection after the first
/// adapter was dropped mid-read.
pub fn single_connection_presets<B: backend::Backend>() {
    let mut ex = LocalExecutor::<B>::with_config(
        Config::single_connection(),
        turnloop::ExecutorConfig::single_connection(),
    )
    .expect("executor");
    let (addr, peer) = crate::echo_peer(2);
    let h = ex.handle();
    let task = ex
        .spawn_local(async move {
            let opts = TcpOpts {
                connect_timeout: Some(Duration::from_secs(5)),
                ..TcpOpts::default()
            };
            let lookup = DnsRequest {
                host: "localhost".into(),
                port: addr.port(),
            };
            assert!(!h.resolve(lookup).await.expect("resolve").is_empty());
            // Abandoned with a read pending: its slots return on cancellation.
            let mut first = h.connect(addr, opts).await.expect("connect");
            let mut bytes = [0; 16];
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(
                Pin::new(&mut first)
                    .poll_read(&mut cx, &mut bytes)
                    .is_pending()
            );
            drop(first);
            let request = async {
                let mut stream = h.connect(addr, opts).await.expect("reconnect");
                let sent = b"one request";
                let mut wrote = 0;
                while wrote < sent.len() {
                    wrote += poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, &sent[wrote..]))
                        .await
                        .expect("write");
                }
                poll_fn(|cx| stream.poll_shutdown(cx))
                    .await
                    .expect("shutdown");
                let mut reply = Vec::new();
                loop {
                    let n = poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, &mut bytes))
                        .await
                        .expect("read");
                    if n == 0 {
                        break reply;
                    }
                    reply.extend_from_slice(&bytes[..n]);
                }
            };
            h.timeout(Duration::from_secs(5), request)
                .await
                .expect("request deadline")
        })
        .expect("spawn");
    let until = ex.driver().now() + Duration::from_secs(10);
    while !task.is_finished() {
        assert!(ex.driver().now() < until, "request stalled");
        ex.turn(Timeout::Until(until)).expect("turn");
    }
    let mut task = task;
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(reply) = Pin::new(&mut task).poll(&mut cx) else {
        panic!("finished task must be ready")
    };
    assert_eq!(reply.expect("task"), b"one request");
    let requests = peer.join().expect("peer");
    assert_eq!(requests[1], b"one request");
    while ex.alive() {
        assert!(ex.driver().now() < until, "teardown");
        ex.turn(Timeout::Until(until)).expect("turn");
    }
}
