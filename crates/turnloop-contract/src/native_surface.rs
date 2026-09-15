//! Shared native surface contracts; backends use identical assertions.
use std::time::Duration;
use turnloop::{backend::Backend, *};

/// Connect and accept local IPC, checking both operation tokens.
pub fn pipe_pair<B: Backend>(l: &mut Driver<B>, name: &PipeName) -> (Handle, Handle, Handle) {
    let listener = l
        .pipe_listen(name, &ListenOpts::default())
        .expect("pipe listen");
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
                OpResult::Connected => {
                    assert_eq!(c.token, Token(2));
                    connected = true;
                }
                OpResult::PipeAccepted { conn: h } => {
                    assert_eq!(c.op, Some(accept));
                    conn = Some(h);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    (listener, client, conn.expect("accepted pipe"))
}
/// Write and read all bytes, allowing partial reads and validating completions.
pub fn transfer<B: Backend>(l: &mut Driver<B>, a: Handle, b: Handle, bytes: &[u8]) {
    l.write(a, WriteBuf::Owned(bytes.to_vec()), Token(11))
        .expect("write");
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
                    assert_eq!(n, b.as_slice().len());
                    assert!(n > 0);
                    read.extend_from_slice(b.as_slice());
                    if read.len() < bytes.len() {
                        l.read(c.handle.expect("reader"), ReadBuf::Pooled, c.token)
                            .expect("continue read");
                    }
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, bytes.len());
                    writes += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(writes, 1);
    assert_eq!(read, bytes);
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
                OpResult::HandleSent => {
                    assert_eq!(c.token, Token(20));
                    sent += 1;
                }
                OpResult::HandleReceived { handle } => {
                    assert_eq!(c.token, Token(21));
                    received = Some(handle);
                }
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((sent, closed), (1, 1));
    let passed = received.expect("received transport");
    let detached = l.detach(passed).expect("detach received socket");
    let attached = l
        .attach(detached, Token(30))
        .expect("attach received socket");
    transfer(&mut l, attached, rx, b"passed socket remains usable");
}

/// Spawn 256 children concurrently and require one distinct reaped exit each.
pub fn children<B: Backend>(program: &std::ffi::OsStr) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(program);
    spec.args.push("exit".into());
    let mut children = Vec::new();
    for i in 0..256 {
        children.push(l.spawn(&spec, Token(i)).expect("spawn"));
    }
    // Every child has a chance to exit before the first completion poll.
    std::thread::sleep(Duration::from_millis(50));
    let mut seen = [false; 256];
    let mut count = 0;
    let mut closed = 0;
    let mut out = Completions::with_capacity(7);
    let until = l.now() + Duration::from_secs(10);
    while count != 256 || closed != 256 {
        assert!(
            l.now() < until,
            "children timed out: {count} exits {closed} closes"
        );
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Exited(status) => {
                    assert_eq!(
                        status,
                        ExitStatus {
                            code: Some(23),
                            signal: None
                        }
                    );
                    let i = c.token.0 as usize;
                    assert!(!seen[i]);
                    seen[i] = true;
                    count += 1;
                    assert_eq!(c.handle, Some(children[i].handle));
                    l.close(children[i].handle, Token(999))
                        .expect("close reaped child");
                }
                OpResult::Closed => closed += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(seen.into_iter().all(|v| v));
    assert!(!l.alive());
}
/// Child stdio is driven by its own loop, proving pipe-backed open_stdio ran.
pub fn child_stdio<B: Backend>(program: &std::ffi::OsStr) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(program);
    spec.args.push("stdio".into());
    spec.stdio = [ProcessStdio::Pipe; 3];
    let child = l.spawn(&spec, Token(1)).expect("spawn stdio child");
    let payload = b"stdio via child loop\n";
    l.write(
        child.stdin.expect("stdin"),
        WriteBuf::Owned(payload.to_vec()),
        Token(2),
    )
    .expect("stdin write");
    l.read_start(child.stdout.expect("stdout"), Token(3))
        .expect("stdout read");
    l.read_start(child.stderr.expect("stderr"), Token(4))
        .expect("stderr read");
    let mut bytes = [Vec::new(), Vec::new()];
    let mut exit = false;
    let mut eof = 0;
    let mut writes = 0;
    let mut out = Completions::with_capacity(2);
    let until = l.now() + Duration::from_secs(10);
    while !exit || eof != 2 {
        assert!(l.now() < until, "child stdio timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Wrote(n) => {
                    assert_eq!(n, payload.len());
                    writes += 1;
                    l.close(child.stdin.expect("stdin"), Token(5))
                        .expect("close stdin");
                }
                OpResult::Read { n, lease: Some(b) } => {
                    assert!(n > 0);
                    bytes[c.token.0 as usize - 3].extend_from_slice(b.as_slice());
                }
                OpResult::Eof => eof += 1,
                OpResult::Exited(status) => {
                    assert_eq!(status.code, Some(23));
                    assert!(!exit);
                    exit = true;
                }
                OpResult::Closed => {}
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(writes, 1);
    assert_eq!(bytes[0], payload);
    assert_eq!(bytes[1], payload);
}
/// Every subscribed loop receives its own signal on its owning thread.
pub fn signal_fanout<B: Backend>(signal: Signal, send: impl FnOnce()) {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for i in 0..4 {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut l = Driver::<B>::new(Config::default()).expect("thread loop");
            let h = l
                .signal_start(signal, Token(i))
                .expect("signal subscription");
            barrier.wait();
            let mut out = Completions::default();
            let until = l.now() + Duration::from_secs(5);
            let mut delivered = 0;
            while delivered == 0 {
                assert!(l.now() < until, "signal was not delivered to loop {i}");
                l.turn(Timeout::Until(until), &mut out).expect("turn");
                for c in out.drain() {
                    assert_eq!(c.token, Token(i));
                    assert_eq!(c.handle, Some(h));
                    assert!(matches!(c.result, OpResult::Signal(received) if received == signal));
                    delivered += 1;
                }
            }
            l.signal_stop(h, Token(100)).expect("signal stop");
            let mut stopped = 0;
            let mut closed = 0;
            while closed == 0 {
                assert!(l.now() < until, "signal stop timed out");
                l.turn(Timeout::Now, &mut out).expect("stop turn");
                for c in out.drain() {
                    match c.result {
                        OpResult::Stopped => stopped += 1,
                        OpResult::Closed => {
                            assert_eq!(stopped, 1);
                            closed += 1;
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                }
            }
            assert_eq!((delivered, stopped, closed), (1, 1, 1));
            assert!(!l.alive());
            delivered
        }));
    }
    barrier.wait();
    send();
    assert_eq!(
        workers
            .into_iter()
            .map(|t| t.join().expect("loop thread"))
            .sum::<usize>(),
        4
    );
}

/// A thousand waits on four owning loops, plus timeout/cancellation races.
pub fn external_waits<B: Backend>() {
    let condition = WaitCondition::new(7).expect("condition");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for thread in 0..4 {
        let condition = condition.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut l = Driver::<B>::new(Config::default()).expect("loop");
            let until = l.now() + Duration::from_secs(5);
            for i in 0..256 {
                l.external_wait(&condition, 7, Some(until), Token(thread * 256 + i))
                    .expect("wait registration");
            }
            barrier.wait();
            let mut seen = [false; 256];
            let mut count = 0;
            let mut out = Completions::with_capacity(3);
            while count != 256 {
                assert!(l.now() < until, "external wait starvation");
                l.turn(Timeout::Until(until), &mut out).expect("turn");
                for c in out.drain() {
                    assert!(matches!(
                        c.result,
                        OpResult::ExternalWait(WaitResult::Notified)
                    ));
                    let i = (c.token.0 - thread * 256) as usize;
                    assert!(!seen[i]);
                    seen[i] = true;
                    count += 1;
                }
            }
            assert!(!l.alive());
            count
        }));
    }
    barrier.wait();
    condition.notify();
    assert_eq!(
        workers
            .into_iter()
            .map(|t| t.join().expect("wait thread"))
            .sum::<usize>(),
        1024
    );
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let at = l.now() + Duration::from_millis(5);
    l.external_wait(&condition, 7, Some(at), Token(1))
        .expect("deadline wait");
    let cancel = l
        .external_wait(&condition, 7, None, Token(2))
        .expect("cancel wait");
    assert!(l.cancel(cancel));
    l.external_wait(&condition, 8, None, Token(3))
        .expect("unequal wait");
    let mut out = Completions::default();
    let mut seen = [false; 3];
    let until = l.now() + Duration::from_secs(2);
    while seen.iter().any(|v| !v) {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            let i = c.token.0 as usize - 1;
            assert!(!seen[i]);
            seen[i] = true;
            match c.result {
                OpResult::ExternalWait(WaitResult::TimedOut) => {
                    assert_eq!(i, 0);
                    assert!(l.now() >= at);
                }
                OpResult::Cancelled => assert_eq!(i, 1),
                OpResult::ExternalWait(WaitResult::NotEqual) => assert_eq!(i, 2),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(!l.alive());
}

/// Signal both a demonstrably live child and its grandchild; inherited stdout
/// reaches EOF only after every process holding its write end has terminated.
pub fn process_group<B: Backend>(program: &std::ffi::OsStr) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(program);
    spec.args.push("grandchild".into());
    spec.new_process_group = true;
    spec.stdio[1] = ProcessStdio::Pipe;
    let process = l.spawn(&spec, Token(1)).expect("spawn tree");
    let stdout = process.stdout.expect("child stdout");
    l.read_start(stdout, Token(2))
        .expect("read readiness marker");
    let mut out = Completions::default();
    let mut marker = Vec::new();
    let until = l.now() + Duration::from_secs(5);
    while !marker.contains(&b'\n') {
        assert!(l.now() < until, "grandchild never became ready");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease: Some(b) } => {
                    assert!(n > 0);
                    marker.extend_from_slice(b.as_slice());
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    let pid = std::str::from_utf8(&marker)
        .expect("marker UTF8")
        .trim()
        .strip_prefix("grandchild:")
        .expect("grandchild marker")
        .parse::<u32>()
        .expect("grandchild PID");
    assert!(pid > 0);
    assert_ne!(pid, process.pid);
    l.kill_group(process.handle, Signal::Kill)
        .expect("kill process group");
    let mut exited = 0;
    let mut eof = 0;
    while exited == 0 || eof == 0 {
        assert!(l.now() < until, "process group still owns stdout");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Exited(status) => {
                    assert!(status.code != Some(0));
                    exited += 1;
                }
                OpResult::Eof => eof += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((exited, eof), (1, 1));
    assert!(
        l.kill(process.handle, Signal::Kill).is_err(),
        "reaped PID must never be signaled"
    );
}
/// Subscribed signals and a live sleeping process must preserve the no-spin limits.
pub fn services_no_spin<B: Backend>(program: &std::ffi::OsStr, signal: Signal) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let mut spec = ProcessSpec::new(program);
    spec.args.push("sleep".into());
    let child = l.spawn(&spec, Token(100)).expect("sleeping child");
    let signal = l.signal_start(signal, Token(101)).expect("idle signal");
    l.set_ref(child.handle, false).expect("unref child");
    l.set_ref(signal, false).expect("unref signal");
    let mut out = Completions::default();
    // Drain registration readiness before measuring exact timer waits.
    l.turn(Timeout::Now, &mut out).expect("registration turn");
    assert!(out.is_empty());
    let mut count = 0;
    let mut waits = 0;
    for micros in [500, 2_000, 10_000] {
        for _ in 0..20 {
            let at = l.now() + Duration::from_micros(micros);
            let timer = l.timer(at, None, Token(1)).expect("timer");
            let mut turns = 0;
            let mut zero = 0;
            loop {
                turns += 1;
                assert!(turns <= 2, "registered services spun");
                let info = l.turn(Timeout::Until(at), &mut out).expect("turn");
                zero += info.zero_event_waits;
                waits += info.os_waits;
                assert!(zero <= 1);
                assert!(info.os_waits <= 1);
                if !out.is_empty() {
                    assert_eq!(out.len(), 1);
                    assert!(matches!(out[0].result, OpResult::Timer));
                    assert!(l.now() >= at);
                    count += 1;
                    break;
                }
            }
            l.close(timer, Token(2)).expect("close timer");
            l.turn(Timeout::Now, &mut out).expect("drain close");
            assert_eq!(out.len(), 1);
            assert!(matches!(out[0].result, OpResult::Closed));
        }
    }
    assert_eq!(count, 60);
    assert!(waits >= 60);
    assert!(
        !l.alive(),
        "unreferenced services cannot keep the loop alive"
    );
}

/// Move a connected socket to a child and back, with real bytes across processes.
pub fn ipc_process<B: Backend>(program: &std::ffi::OsStr, name: &PipeName) {
    let mut l = Driver::<B>::new(Config::default()).expect("loop");
    let listener = l
        .pipe_listen(name, &ListenOpts::default())
        .expect("IPC listener");
    l.accept(listener, Token(1)).expect("IPC accept");
    let (_, tx, rx) = crate::pair(&mut l);
    let mut spec = ProcessSpec::new(program);
    spec.args = vec!["handle".into(), name.0.clone().into_os_string()];
    let child = l.spawn(&spec, Token(2)).expect("IPC child");
    l.read_start(rx, Token(3)).expect("socket read");
    let mut bytes = Vec::new();
    let mut returned = None;
    let mut sent = 0;
    let mut exited = 0;
    let until = l.now() + Duration::from_secs(5);
    let mut out = Completions::default();
    while returned.is_none() || sent == 0 || exited == 0 || bytes.len() < 20 {
        assert!(l.now() < until, "process handle passing timed out");
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::PipeAccepted { conn } => {
                    l.send_handle(conn, tx, Token(4)).expect("send socket");
                    l.recv_handle(conn, Token(5))
                        .expect("receive returned socket");
                }
                OpResult::HandleSent => {
                    sent += 1;
                    l.close(tx, Token(6)).expect("close source socket");
                }
                OpResult::HandleReceived { handle } => returned = Some(handle),
                OpResult::Read {
                    n,
                    lease: Some(data),
                } => {
                    assert!(n > 0);
                    bytes.extend_from_slice(data.as_slice());
                }
                OpResult::Exited(status) => {
                    assert_eq!(c.handle, Some(child.handle));
                    assert_eq!(status.code, Some(0));
                    exited += 1;
                }
                OpResult::Closed => {}
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!(bytes, b"cross-process socket");
    assert_eq!((sent, exited), (1, 1));
    // Stop the multishot read before the next one-shot transfer assertion.
    l.close(rx, Token(7)).expect("close receiver");
    let handle = returned.expect("returned descriptor");
    let d = l.detach(handle).expect("detach returned descriptor");
    let mut other = Driver::<B>::new(Config::default()).expect("other loop");
    let handle = other.attach(d, Token(8)).expect("attach on other loop");
    other
        .close(handle, Token(9))
        .expect("close returned socket");
}

/// Deadline setup, completion removal and cancellation use the same core on
/// every native backend, including when the deadline already elapsed at submit.
pub fn pipe_connect_deadlines<B: Backend>(name: &PipeName) {
    let mut driver = Driver::<B>::new(Config::default()).expect("loop");
    let listener = driver
        .pipe_listen(name, &ListenOpts::default())
        .expect("listener");
    driver.accept(listener, Token(1)).expect("accept");
    let at = driver.now() + Duration::from_secs(5);
    let client = driver
        .pipe_connect_until(name, at, Token(2))
        .expect("deadline connect");
    assert_eq!(driver.next_deadline(), Some(at));
    let mut out = Completions::with_capacity(1);
    let (mut connected, mut accepted) = (0, 0);
    while connected == 0 || accepted == 0 {
        assert!(driver.now() < at);
        driver
            .turn(Timeout::Until(at), &mut out)
            .expect("connect before deadline");
        for c in out.drain() {
            match c.result {
                OpResult::Connected => {
                    assert_eq!(c.handle, Some(client));
                    connected += 1;
                }
                OpResult::PipeAccepted { .. } => accepted += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((connected, accepted), (1, 1));
    assert_eq!(
        driver.next_deadline(),
        None,
        "successful connect retires its deadline"
    );
    for cancel in [false, true] {
        let at = driver.now();
        let h = driver
            .pipe_connect_until(name, at, Token(3))
            .expect("elapsed deadline connect");
        if cancel {
            driver
                .close(h, Token(4))
                .expect("explicit close wins before expiry");
        }
        let until = driver.now() + Duration::from_secs(5);
        let mut count = 0;
        while count < if cancel { 2 } else { 1 } {
            assert!(driver.now() < until);
            driver
                .turn(Timeout::Until(until), &mut out)
                .expect("deadline cancellation");
            for c in out.drain() {
                assert_eq!(c.handle, Some(h));
                assert!(c.terminal);
                assert!(match (cancel, count) {
                    (false, 0) => matches!(
                        c.result,
                        OpResult::Err(Error {
                            kind: ErrorKind::TimedOut,
                            ..
                        })
                    ),
                    (true, 0) => matches!(c.result, OpResult::Cancelled),
                    (true, 1) => matches!(c.result, OpResult::Closed),
                    _ => false,
                });
                count += 1;
            }
        }
        assert_eq!(driver.next_deadline(), None);
    }
}
