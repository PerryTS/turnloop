#[path = "allocations.rs"]
mod allocations;
use js_sys::{Function, Promise};
use std::{cell::Cell, rc::Rc, time::Duration};
use turnloop::*;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;
#[wasm_bindgen(module = "/tests/web/helpers.js")]
extern "C" {
    fn guard(p: &Promise) -> Promise;
    fn sleep(ms: f64) -> Promise;
    fn stats(base: &str) -> Promise;
    #[cfg(feature = "web-worker")]
    fn producers(descriptor: &JsValue, count: u32) -> JsValue;
    #[cfg(feature = "web-worker")]
    #[wasm_bindgen(js_name=stopProducers)]
    fn stop_producers(group: &JsValue);
    #[cfg(feature = "web-worker")]
    #[wasm_bindgen(js_name=producersDone)]
    fn producers_done(group: &JsValue) -> Promise;
    #[cfg(feature = "web-worker")]
    #[wasm_bindgen(js_name=fillRing)]
    fn fill_ring(descriptor: &JsValue);
}
fn base() -> &'static str {
    option_env!("TURNLOOP_WEB_FIXTURE").unwrap_or("http://127.0.0.1:18765")
}
async fn scheduled(l: &mut Loop, out: &mut Completions) {
    let mut resolve = None;
    let promise = Promise::new(&mut |r, _| resolve = Some(r));
    let resolve = resolve.expect("promise resolver");
    let callback = Closure::wrap(Box::new(move || {
        resolve.call0(&JsValue::UNDEFINED).expect("resolve");
    }) as Box<dyn FnMut()>);
    l.set_schedule_turn(callback.as_ref().unchecked_ref())
        .expect("schedule");
    JsFuture::from(guard(&promise))
        .await
        .expect("scheduled turn");
    let info = l.turn(Timeout::Now, out).expect("turn");
    assert_eq!(info.os_waits, 0);
    assert_eq!(info.zero_event_waits, 0);
    // Clear the callback before dropping the Rust closure.
    l.set_schedule_turn(&Function::new_no_args(""))
        .expect("clear dispatcher");
}
async fn websocket(l: &mut Loop) -> Handle {
    let url = base().replace("http:", "ws:") + "/echo";
    let h = l.websocket(&url, Token(1)).expect("websocket");
    let mut out = Completions::default();
    scheduled(l, &mut out).await;
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Connected));
    h
}
#[wasm_bindgen_test]
fn now_only_and_unsupported() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let mut out = Completions::default();
    assert!(matches!(
        l.integration().expect("integration"),
        Integration::HostCallback
    ));
    l.poster().post(Token(9), Payload::U64(77)).expect("post");
    for timeout in [
        Timeout::After(Duration::ZERO),
        Timeout::Until(l.now()),
        Timeout::Forever,
    ] {
        assert_eq!(
            l.turn(timeout, &mut out).expect_err("must reject").kind,
            ErrorKind::Unsupported
        );
    }
    l.turn(Timeout::Now, &mut out).expect("turn");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(77))));
    let addr = "127.0.0.1:1234".parse().expect("address");
    assert_eq!(
        l.tcp_connect(addr, &TcpOpts::default(), Token(1))
            .expect_err("raw TCP")
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        l.tcp_listen(addr, &ListenOpts::default())
            .expect_err("listen")
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        l.udp_bind(addr, &UdpOpts::default()).expect_err("UDP").kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        l.blocking(|| Ok(Payload::U64(1)), Token(1))
            .expect_err("blocking")
            .kind,
        ErrorKind::Unsupported
    );
}
#[wasm_bindgen_test(async)]
async fn timer_no_spin_with_idle_websocket() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let h = websocket(&mut l).await;
    let read = l.read(h, ReadBuf::Pooled, Token(90)).expect("idle read");
    let mut out = Completions::default();
    let mut expiries = 0;
    for micros in [500, 2000, 10000] {
        for _ in 0..20 {
            let at = l.now() + Duration::from_micros(micros);
            let timer = l.timer(at, None, Token(91)).expect("timer");
            scheduled(&mut l, &mut out).await;
            assert!(l.now() >= at);
            assert!(l.now() - at < Duration::from_millis(100));
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].handle, Some(timer));
            assert_eq!(out[0].token, Token(91));
            assert!(matches!(out[0].result, OpResult::Timer));
            expiries += 1;
            l.close(timer, Token(92)).expect("close");
            l.turn(Timeout::Now, &mut out).expect("drain close");
            assert!(matches!(out[0].result, OpResult::Closed));
        }
    }
    assert_eq!(expiries, 60);
    assert!(l.cancel(read));
    l.close(h, Token(93)).expect("close websocket");
    l.turn(Timeout::Now, &mut out).expect("drain");
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0].result, OpResult::Cancelled));
    assert!(matches!(out[1].result, OpResult::Closed));
}
#[wasm_bindgen_test(async)]
async fn fetch_bytes_abort_and_close_ordering() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let token = Token(u64::MAX - 7);
    let (h, op) = l
        .fetch(&(base().to_owned() + "/bytes"), ReadBuf::Pooled, token)
        .expect("fetch");
    scheduled(&mut l, &mut out).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].op, Some(op));
    assert_eq!(out[0].token, token);
    let OpResult::Read {
        n,
        lease: Some(lease),
    } = &out[0].result
    else {
        panic!("fetch bytes missing")
    };
    assert_eq!(*n, 257);
    for (i, &byte) in lease.as_slice().iter().enumerate() {
        assert_eq!(byte, (i * 73 + 19) as u8);
    }
    l.close(h, Token(3)).expect("close");
    l.turn(Timeout::Now, &mut out).expect("closed");
    assert!(matches!(out[0].result, OpResult::Closed));
    let baseline = JsFuture::from(stats(base())).await.expect("baseline");
    let baseline = js_sys::Reflect::get(&baseline, &"slow".into())
        .expect("slow count")
        .as_f64()
        .expect("number");
    let (h, op) = l
        .fetch(&(base().to_owned() + "/slow"), ReadBuf::Pooled, Token(4))
        .expect("slow fetch");
    // Ensure the fixture observed the actual request before testing AbortController.
    let mut observed = false;
    for _ in 0..100 {
        let value = JsFuture::from(stats(base())).await.expect("stats");
        if js_sys::Reflect::get(&value, &"slow".into())
            .expect("slow count")
            .as_f64()
            .is_some_and(|n| n > baseline)
        {
            observed = true;
            break;
        }
        JsFuture::from(sleep(5.0)).await.expect("sleep");
    }
    assert!(observed, "slow HTTP request must reach fixture");
    assert!(l.cancel(op));
    assert!(!l.cancel(op));
    l.close(h, Token(5)).expect("close");
    l.turn(Timeout::Now, &mut out).expect("cancel");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].op, Some(op));
    assert!(matches!(out[0].result, OpResult::Cancelled));
    l.turn(Timeout::Now, &mut out).expect("close");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Closed));
    JsFuture::from(sleep(20.0)).await.expect("abort callback");
    l.turn(Timeout::Now, &mut out).expect("late callback");
    assert!(out.is_empty());
}
#[wasm_bindgen_test(async)]
async fn websocket_bytes_and_schedule_coalescing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let h = websocket(&mut l).await;
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    let callback = Closure::wrap(Box::new(move || count.set(count.get() + 1)) as Box<dyn FnMut()>);
    l.set_schedule_turn(callback.as_ref().unchecked_ref())
        .expect("dispatcher");
    let before = l.schedule_count();
    for i in 0..100 {
        l.poster().post(Token(i), Payload::U64(i)).expect("post");
    }
    assert_eq!(l.schedule_count() - before, 1);
    assert_eq!(calls.get(), 0, "no synchronous user dispatch");
    JsFuture::from(sleep(0.0)).await.expect("yield");
    assert_eq!(calls.get(), 1);
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("posts");
    assert_eq!(out.len(), 100);
    let bytes: Vec<u8> = (0..257).map(|i| (i * 73 + 19) as u8).collect();
    l.read(h, ReadBuf::Pooled, Token(200)).expect("read");
    l.write(h, WriteBuf::Owned(bytes.clone()), Token(201))
        .expect("write");
    let mut wrote = false;
    let mut read = false;
    while !wrote || !read {
        scheduled(&mut l, &mut out).await;
        for c in out.drain() {
            match c.result {
                OpResult::Read { n, lease: Some(b) } => {
                    assert_eq!(n, 257);
                    assert_eq!(b.as_slice(), bytes);
                    read = true;
                }
                OpResult::Wrote(n) => {
                    assert_eq!(n, 257);
                    wrote = true;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(wrote && read);
    l.close(h, Token(202)).expect("close");
}

#[cfg(feature = "web-worker")]
#[wasm_bindgen_test(async)]
async fn worker_mpsc_backpressure_and_no_lost_wake() {
    let mut l = Loop::new(Config {
        post_capacity: 8,
        events_per_turn: 4,
        ..Config::default()
    })
    .expect("loop");
    let descriptor = l
        .worker_poster(16)
        .expect("isolated SAB + Atomics.waitAsync required");
    fill_ring(&descriptor);
    let mut out = Completions::with_capacity(3);
    let mut full = 0;
    while full < 16 {
        scheduled(&mut l, &mut out).await;
        for c in out.drain() {
            assert!(matches!(c.result,OpResult::Posted(Payload::U64(n)) if n==c.token.0));
            full += 1;
        }
    }
    assert_eq!(full, 16);
    let group = producers(&descriptor, 1000);
    let mut seen = vec![false; 2000];
    let mut count = 0;
    while count < 2000 {
        scheduled(&mut l, &mut out).await;
        for c in out.drain() {
            let n = c
                .token
                .0
                .checked_sub(0xf123456700000000)
                .expect("full token") as usize;
            assert!(n < 2000 && !seen[n]);
            seen[n] = true;
            assert!(matches!(c.result,OpResult::Posted(Payload::U64(v)) if v==(n as u64)^u64::MAX));
            count += 1;
        }
    }
    JsFuture::from(producers_done(&group))
        .await
        .expect("both workers finished");
    stop_producers(&group);
    assert_eq!(count, 2000);
    assert!(seen.into_iter().all(|s| s));
    let before = l.schedule_count();
    JsFuture::from(sleep(20.0)).await.expect("idle");
    assert_eq!(
        l.schedule_count(),
        before,
        "no periodic wake while ring empty"
    );
}
#[wasm_bindgen_test(async)]
async fn steady_rust_websocket_posts_and_timers_allocate_nothing() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let h = websocket(&mut l).await;
    let mut out = Completions::default();
    static BYTES: [u8; 64] = [0x59; 64];
    let mut total = 0;
    for _ in 0..100 {
        allocations::measure(|| {
            l.read(h, ReadBuf::Pooled, Token(1)).expect("read");
            // SAFETY: static immutable bytes survive until the write completes.
            let bytes = unsafe { IoBuf::from_raw_parts(BYTES.as_ptr(), BYTES.len()) };
            l.write(h, WriteBuf::Provided(bytes), Token(2))
                .expect("write");
            l.poster().post(Token(3), Payload::U64(4)).expect("post");
            let timer = l.timer(l.now(), None, Token(5)).expect("timer");
            l.close(timer, Token(6)).expect("close timer");
        });
        let mut received = 0;
        let mut written = 0;
        let mut posts = 0;
        let mut cancelled = 0;
        let mut closed = 0;
        for _ in 0..1000 {
            allocations::measure(|| l.turn(Timeout::Now, &mut out).expect("turn"));
            for c in out.drain() {
                match c.result {
                    OpResult::Read { n, lease: Some(b) } => {
                        assert_eq!(b.as_slice(), BYTES);
                        received += n;
                    }
                    OpResult::Wrote(n) => written += n,
                    OpResult::Posted(Payload::U64(4)) => posts += 1,
                    OpResult::Cancelled => cancelled += 1,
                    OpResult::Closed => closed += 1,
                    other => panic!("unexpected {other:?}"),
                }
            }
            if received == 64 && written == 64 && posts == 1 && cancelled == 1 && closed == 1 {
                break;
            }
            JsFuture::from(sleep(1.0)).await.expect("host yield");
        }
        assert_eq!(
            (received, written, posts, cancelled, closed),
            (64, 64, 1, 1, 1)
        );
        total += received;
    }
    assert_eq!(total, 6400);
}

#[wasm_bindgen_test(async)]
async fn capability_errors_and_oversize_response_are_terminal() {
    let mut l = Loop::new(Config {
        pooled_buffer_size: 16,
        ..Config::default()
    })
    .expect("loop");
    let h = websocket(&mut l).await;
    assert_eq!(
        l.read_start(h, Token(1)).expect_err("multishot").kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        l.writev(h, WriteVectored::new([]).expect("vector"), Token(2))
            .expect_err("writev")
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(
        l.detach(h).expect_err("transfer").kind,
        ErrorKind::Unsupported
    );
    let read = l.read(h, ReadBuf::Pooled, Token(3)).expect("read");
    assert_eq!(
        l.read(h, ReadBuf::Pooled, Token(4))
            .expect_err("second read")
            .kind,
        ErrorKind::WouldBlock
    );
    assert!(l.cancel(read));
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("cancel ack");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Cancelled));
    l.close(h, Token(5)).expect("close");
    l.turn(Timeout::Now, &mut out).expect("closed");
    let (h, op) = l
        .fetch(&(base().to_owned() + "/bytes"), ReadBuf::Pooled, Token(6))
        .expect("fetch");
    scheduled(&mut l, &mut out).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].op, Some(op));
    assert!(matches!(
        out[0].result,
        OpResult::Err(Error {
            kind: ErrorKind::ResourceLimit,
            ..
        })
    ));
    assert!(!l.cancel(op), "oversize read retired exactly once");
    l.close(h, Token(7)).expect("close");
    l.turn(Timeout::Now, &mut out).expect("closed");
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0].result, OpResult::Closed));
    let timer = l
        .timer(l.now() + Duration::from_millis(5), None, Token(8))
        .expect("timer");
    l.close(timer, Token(9)).expect("cancel timer");
    l.turn(Timeout::Now, &mut out).expect("timer close");
    assert_eq!(out.len(), 2);
    let before = l.schedule_count();
    JsFuture::from(sleep(20.0))
        .await
        .expect("past cancelled deadline");
    l.turn(Timeout::Now, &mut out).expect("late timer");
    assert!(out.is_empty());
    assert_eq!(l.schedule_count(), before, "cancelled deadline disarmed");
}

#[wasm_bindgen_test]
fn revision_two_single_agent_contracts() {
    turnloop_contract::single_agent::unsupported_native::<backend::Platform>();
    turnloop_contract::single_agent::waits::<backend::Platform>();
    let mut l = Loop::new(Config::default()).expect("loop");
    for which in [Stdio::Stdin, Stdio::Stdout, Stdio::Stderr] {
        assert_eq!(
            l.open_stdio(which).expect_err("browser stdio").kind,
            ErrorKind::Unsupported
        );
    }
    let condition = WaitCondition::new(0).expect("condition");
    let mut out = Completions::with_capacity(1);
    let mut completions = 0;
    allocations::measure(|| {
        for kind in 0..800 {
            let op = l
                .external_wait(
                    &condition,
                    u64::from(kind % 4 == 0),
                    Some(l.now()),
                    Token(kind),
                )
                .expect("wait");
            if kind % 4 == 1 {
                condition.notify();
            }
            if kind % 4 == 2 {
                assert!(l.cancel(op));
            }
            l.turn(Timeout::Now, &mut out).expect("completion");
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].op, Some(op));
            assert!(match kind % 4 {
                0 => matches!(out[0].result, OpResult::ExternalWait(WaitResult::NotEqual)),
                1 => matches!(out[0].result, OpResult::ExternalWait(WaitResult::Notified)),
                2 => matches!(out[0].result, OpResult::Cancelled),
                _ => matches!(out[0].result, OpResult::ExternalWait(WaitResult::TimedOut)),
            });
            completions += 1;
        }
    });
    assert_eq!(completions, 800);
    assert!(!l.alive());
}

#[wasm_bindgen_test(async)]
async fn external_wait_deadlines_schedule_without_spin() {
    let mut l = Loop::new(Config::default()).expect("loop");
    let socket = websocket(&mut l).await;
    let idle = l
        .read(socket, ReadBuf::Pooled, Token(90))
        .expect("idle read");
    let condition = WaitCondition::new(0).expect("condition");
    let mut out = Completions::default();
    let mut expiries = 0;
    for micros in [500, 2000, 10000] {
        for _ in 0..20 {
            let at = l.now() + Duration::from_micros(micros);
            let op = l
                .external_wait(&condition, 0, Some(at), Token(1))
                .expect("wait");
            assert_eq!(l.next_deadline(), Some(at));
            let mut turns = 0;
            loop {
                scheduled(&mut l, &mut out).await;
                turns += 1;
                assert!(turns <= 2);
                if out.is_empty() {
                    continue;
                }
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].op, Some(op));
                assert!(matches!(
                    out[0].result,
                    OpResult::ExternalWait(WaitResult::TimedOut)
                ));
                assert!(l.now() >= at);
                assert!(l.now() - at < Duration::from_millis(100));
                expiries += 1;
                break;
            }
        }
    }
    assert_eq!(expiries, 60);
    assert!(l.cancel(idle));
}

#[cfg(feature = "executor")]
async fn executor_scheduled(ex: &mut LocalExecutor<backend::Platform>) {
    let mut resolve = None;
    let promise = Promise::new(&mut |r, _| resolve = Some(r));
    let resolve = resolve.expect("resolver");
    let callback = Closure::wrap(Box::new(move || {
        resolve.call0(&JsValue::UNDEFINED).expect("resolve");
    }) as Box<dyn FnMut()>);
    ex.driver()
        .set_schedule_turn(callback.as_ref().unchecked_ref())
        .expect("schedule");
    JsFuture::from(guard(&promise))
        .await
        .expect("executor scheduled");
    allocations::measure(|| ex.turn(Timeout::Now).expect("executor turn"));
    ex.driver()
        .set_schedule_turn(&Function::new_no_args(""))
        .expect("clear callback");
}

#[cfg(feature = "executor")]
#[wasm_bindgen_test(async)]
async fn executor_websocket_sleep_and_cancel() {
    use futures_io::{AsyncRead, AsyncWrite};
    use std::{
        future::{Future, poll_fn},
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let socket = ex
        .driver()
        .websocket(&(base().replace("http:", "ws:") + "/echo"), Token(1))
        .expect("websocket");
    executor_scheduled(&mut ex).await;
    let mut stream = ex.handle().io(socket);
    let count = Rc::new(Cell::new(0));
    let completed = count.clone();
    let h = ex.handle();
    let task = ex
        .spawn_local(async move {
            for _ in 0..20 {
                assert_eq!(
                    poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, &[37; 64]))
                        .await
                        .expect("write"),
                    64
                );
                poll_fn(|cx| Pin::new(&mut stream).poll_flush(cx))
                    .await
                    .expect("flush");
                let mut bytes = [0; 64];
                let n = poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, &mut bytes))
                    .await
                    .expect("read");
                assert_eq!(n, 64);
                assert_eq!(bytes, [37; 64]);
                h.sleep(Duration::from_micros(500)).await.expect("sleep");
                completed.set(completed.get() + 1);
            }
            let error = h
                .timeout(Duration::from_millis(2), std::future::pending::<()>())
                .await
                .expect_err("timeout");
            assert_eq!(error.kind, ErrorKind::TimedOut);
        })
        .expect("task");
    assert_eq!(ex.run_ready(), 1, "task actually submitted a write");
    while !task.is_finished() {
        executor_scheduled(&mut ex).await;
    }
    assert_eq!(count.get(), 20);
    let mut cancelled = ex
        .spawn_local(std::future::pending::<()>())
        .expect("pending task");
    cancelled.cancel();
    ex.run_ready();
    assert_eq!(
        Pin::new(&mut cancelled).poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Err(JoinError::Cancelled))
    );
}

#[cfg(feature = "web-worker")]
#[wasm_bindgen(module = "/tests/web/helpers.js")]
extern "C" {
    #[wasm_bindgen(js_name=conditionWorker)]
    fn condition_worker(descriptor: &JsValue) -> Promise;
    #[wasm_bindgen(js_name=updateCondition)]
    fn update_condition(worker: &JsValue, store: bool, value: u64) -> Promise;
    #[wasm_bindgen(js_name=stopCondition)]
    fn stop_condition(worker: &JsValue);
    #[wasm_bindgen(js_name=conditionBackpressure)]
    fn condition_backpressure(descriptor: &JsValue);
    #[wasm_bindgen(js_name=conditionClosed)]
    fn condition_closed(descriptor: &JsValue);
}
#[cfg(feature = "web-worker")]
#[wasm_bindgen_test(async)]
async fn worker_conditions_notify_waiting_loops_and_close() {
    let mut a = Loop::new(Config::default()).expect("loop A");
    let mut b = Loop::new(Config::default()).expect("loop B");
    let condition = WaitCondition::new(0).expect("condition");
    let descriptor = a
        .worker_wait_condition(&condition, 2)
        .expect("condition ring");
    condition_backpressure(&descriptor);
    let mut out = Completions::with_capacity(1);
    a.turn(Timeout::Now, &mut out).expect("drain filled ring");
    assert!(out.is_empty());
    let worker = JsFuture::from(condition_worker(&descriptor))
        .await
        .expect("real worker started");
    let mut completions = 0;
    for round in 0..32 {
        let expected = condition.load();
        let one = a
            .external_wait(&condition, expected, None, Token(1))
            .expect("A wait");
        let two = b
            .external_wait(&condition, expected, None, Token(2))
            .expect("B wait");
        // Both loops enter externally parked state; the condition owner may differ
        // from a waiting loop. A Worker uses the shared ring's Atomics wake path.
        a.integration().expect("park A");
        b.integration().expect("park B");
        JsFuture::from(update_condition(
            &worker,
            round % 2 == 0,
            0xf123456700000000 + round,
        ))
        .await
        .expect("worker admitted update");
        for (l, op) in [(&mut a, one), (&mut b, two)] {
            scheduled(l, &mut out).await;
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].op, Some(op));
            assert!(matches!(
                out[0].result,
                OpResult::ExternalWait(WaitResult::Notified)
            ));
            completions += 1;
        }
        if round % 2 == 0 {
            assert_eq!(condition.load(), 0xf123456700000000 + round);
        } else {
            assert_eq!(condition.load(), expected, "same-value notification");
        }
    }
    stop_condition(&worker);
    assert_eq!(completions, 64);
    drop(a);
    condition_closed(&descriptor);
    let op = b
        .external_wait(&condition, condition.load(), None, Token(3))
        .expect("survivor wait");
    condition.notify();
    scheduled(&mut b, &mut out).await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].op, Some(op));
    assert!(!b.alive());
}

#[cfg(feature = "executor")]
#[wasm_bindgen_test(async)]
async fn executor_fetch_bytes_and_deadline_abort() {
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let h = ex.handle();
    let url = base().to_owned();
    let mut task = ex
        .spawn_local(async move {
            let bytes = h.fetch(&(url.clone() + "/bytes")).await.expect("fetch");
            assert_eq!(bytes.len(), 257);
            for (i, byte) in bytes.iter().enumerate() {
                assert_eq!(*byte, ((i * 73 + 19) & 255) as u8);
            }
            let at = h.now() + Duration::from_millis(30);
            assert_eq!(
                h.timeout_at(at, h.fetch(&(url + "/slow")))
                    .await
                    .expect_err("fetch deadline")
                    .kind,
                ErrorKind::TimedOut
            );
            bytes.len()
        })
        .expect("task");
    assert_eq!(ex.run_ready(), 1);
    while !task.is_finished() {
        let mut resolve = None;
        let promise = Promise::new(&mut |r, _| resolve = Some(r));
        let resolve = resolve.expect("resolver");
        let callback = Closure::wrap(Box::new(move || {
            resolve.call0(&JsValue::UNDEFINED).expect("resolve");
        }) as Box<dyn FnMut()>);
        ex.driver()
            .set_schedule_turn(callback.as_ref().unchecked_ref())
            .expect("schedule");
        JsFuture::from(guard(&promise))
            .await
            .expect("fetch scheduled");
        ex.turn(Timeout::Now).expect("fetch turn");
        ex.driver()
            .set_schedule_turn(&Function::new_no_args(""))
            .expect("clear callback");
    }
    match Pin::new(&mut task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(n)) => assert_eq!(n, 257),
        _ => panic!("fetch task incomplete"),
    }
    ex.turn(Timeout::Now).expect("cancel and close delivery");
}
