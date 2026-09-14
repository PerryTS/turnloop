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

/// Spawn 256 children concurrently and require one distinct reaped exit each.
pub fn children<B: Backend>(program: &std::ffi::OsStr) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(program);
    spec.args.push("exit".into());
    let mut children = Vec::new();
    for i in 0..256 { children.push(l.spawn(&spec, Token(i)).expect("spawn")); }
    // Every child has a chance to exit before the first completion poll.
    std::thread::sleep(Duration::from_millis(50));
    let mut seen = [false; 256];
    let mut count = 0;
    let mut closed = 0;
    let mut out = Completions::with_capacity(7);
    let until = l.now() + Duration::from_secs(10);
    while count != 256 || closed != 256 {
        assert!(l.now() < until, "children timed out: {count} exits {closed} closes");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Exited(status) => {
                    assert_eq!(status, ExitStatus { code: Some(23), signal: None });
                    let i = c.token.0 as usize; assert!(!seen[i]); seen[i] = true; count += 1;
                    assert_eq!(c.handle, Some(children[i].handle));
                    l.close(children[i].handle, Token(999)).expect("close reaped child");
                }
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(seen.into_iter().all(|v| v)); assert!(!l.alive());
}
/// Child stdio is driven by its own loop, proving pipe-backed open_stdio ran.
pub fn child_stdio<B: Backend>(program: &std::ffi::OsStr) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(program); spec.args.push("stdio".into());
    spec.stdio = [ProcessStdio::Pipe; 3];
    let child = l.spawn(&spec, Token(1)).expect("spawn stdio child");
    let payload = b"stdio via child loop\n";
    l.write(child.stdin.expect("stdin"), WriteBuf::Owned(payload.to_vec()), Token(2)).expect("stdin write");
    l.read_start(child.stdout.expect("stdout"), Token(3)).expect("stdout read");
    l.read_start(child.stderr.expect("stderr"), Token(4)).expect("stderr read");
    let mut bytes = [Vec::new(), Vec::new()];
    let mut exit = false; let mut eof = 0; let mut writes = 0;
    let mut out = Completions::with_capacity(2);
    let until = l.now() + Duration::from_secs(10);
    while !exit || eof != 2 {
        assert!(l.now() < until, "child stdio timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Wrote(n) => { assert_eq!(n, payload.len()); writes += 1; l.close(child.stdin.expect("stdin"), Token(5)).expect("close stdin"); }
                OpResult::Read { n, lease: Some(b) } => { assert!(n > 0); bytes[c.token.0 as usize - 3].extend_from_slice(b.as_slice()); }
                OpResult::Eof => eof += 1,
                OpResult::Exited(status) => { assert_eq!(status.code, Some(23)); assert!(!exit); exit = true; }
                OpResult::Closed => {},
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(writes, 1); assert_eq!(bytes[0], payload); assert_eq!(bytes[1], payload);
}
/// Every subscribed loop receives its own signal on its owning thread.
pub fn signal_fanout<B: Backend>(send: impl FnOnce()) {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for i in 0..4 {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut l = Driver::<B>::new(Config::default()).expect("thread loop");
            let h = l.signal_start(Signal::Usr1, Token(i)).expect("signal subscription");
            barrier.wait();
            let mut out = Completions::default();
            let until = l.now() + Duration::from_secs(5);
            let mut delivered = 0;
            while delivered == 0 {
                assert!(l.now() < until, "signal was not delivered to loop {i}");
                l.turn(Timeout::Until(until), &mut out).expect("turn");
                for c in out.drain() {
                    assert_eq!(c.token, Token(i)); assert_eq!(c.handle, Some(h));
                    assert!(matches!(c.result, OpResult::Signal(Signal::Usr1))); delivered += 1;
                }
            }
            l.signal_stop(h, Token(100)).expect("signal stop");
            let mut stopped = 0; let mut closed = 0;
            while closed == 0 {
                l.turn(Timeout::Now, &mut out).expect("stop turn");
                for c in out.drain() {
                    match c.result { OpResult::Stopped => stopped += 1, OpResult::Closed => { assert_eq!(stopped, 1); closed += 1; }, other => panic!("unexpected {other:?}") }
                }
            }
            assert_eq!((delivered, stopped, closed), (1, 1, 1));
            assert!(!l.alive()); delivered
        }));
    }
    barrier.wait(); send();
    assert_eq!(workers.into_iter().map(|t| t.join().expect("loop thread")).sum::<usize>(), 4);
}

/// A thousand waits on four owning loops, plus timeout/cancellation races.
pub fn external_waits<B: Backend>() {
    let condition = WaitCondition::new(7).expect("condition");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for thread in 0..4 {
        let condition = condition.clone(); let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut l = Driver::<B>::new(Config::default()).expect("loop");
            let until = l.now() + Duration::from_secs(5);
            for i in 0..256 { l.external_wait(&condition, 7, Some(until), Token(thread * 256 + i)).expect("wait registration"); }
            barrier.wait();
            let mut seen = [false; 256]; let mut count = 0;
            let mut out = Completions::with_capacity(3);
            while count != 256 {
                assert!(l.now() < until, "external wait starvation");
                l.turn(Timeout::Until(until), &mut out).expect("turn");
                for c in out.drain() {
                    assert!(matches!(c.result, OpResult::ExternalWait(WaitResult::Notified)));
                    let i = (c.token.0 - thread * 256) as usize; assert!(!seen[i]); seen[i] = true; count += 1;
                }
            }
            assert!(!l.alive()); count
        }));
    }
    barrier.wait(); condition.notify();
    assert_eq!(workers.into_iter().map(|t| t.join().expect("wait thread")).sum::<usize>(), 1024);
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let at = l.now() + Duration::from_millis(5);
    l.external_wait(&condition, 7, Some(at), Token(1)).expect("deadline wait");
    let cancel = l.external_wait(&condition, 7, None, Token(2)).expect("cancel wait");
    assert!(l.cancel(cancel));
    l.external_wait(&condition, 8, None, Token(3)).expect("unequal wait");
    let mut out = Completions::default(); let mut seen = [false; 3];
    let until = l.now() + Duration::from_secs(2);
    while seen.iter().any(|v| !v) {
        assert!(l.now() < until); l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            let i = c.token.0 as usize - 1; assert!(!seen[i]); seen[i] = true;
            match c.result {
                OpResult::ExternalWait(WaitResult::TimedOut) => { assert_eq!(i, 0); assert!(l.now() >= at); }
                OpResult::Cancelled => assert_eq!(i, 1),
                OpResult::ExternalWait(WaitResult::NotEqual) => assert_eq!(i, 2),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(!l.alive());
}
