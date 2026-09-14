#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[path = "../../../crates/turnloop-io/tests/support/count.rs"]
mod count;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};
use turnloop_io::{
    turnloop::{Config as LoopConfig, LocalExecutor, Timeout, backend::Platform},
    *,
};
fn finish<T>(task: &mut turnloop::JoinHandle<T>) -> T {
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(Ok(v)) => v,
        _ => panic!("task must finish"),
    }
}
fn drive<T>(executor: &mut LocalExecutor<Platform>, task: &mut turnloop::JoinHandle<T>) -> T {
    let at = executor.handle().now() + Duration::from_secs(90);
    while !task.is_finished() {
        assert!(executor.handle().now() < at, "test deadline");
        executor.turn(Timeout::Until(at)).expect("turn");
    }
    finish(task)
}
async fn exact<S: Stream>(s: &mut S, mut bytes: &mut [u8]) {
    while !bytes.is_empty() {
        let n = read(s, bytes).await.expect("read");
        assert!(n > 0, "unexpected EOF");
        bytes = &mut bytes[n..];
    }
}
use turnloop_mongodb::{
    asynchronous::Connection,
    bson::{doc, raw::RawDocumentBuf},
    uri::Options,
    wire,
};
async fn mongo_packet<S: Stream>(s: &mut S) -> Vec<u8> {
    let mut length = [0; 4];
    exact(s, &mut length).await;
    let n = i32::from_le_bytes(length) as usize;
    assert!((16..16384).contains(&n));
    let mut frame = vec![0; n];
    frame[..4].copy_from_slice(&length);
    exact(s, &mut frame[4..]).await;
    frame
}
#[test]
fn command_reply_and_cancel_close() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut s = listener.accept().await.expect("accept");
            let mut reply = Vec::new();
            for round in 0..3 {
                let bytes = mongo_packet(&mut s).await;
                let message = wire::Message::parse(&bytes, 16384).expect("OP_MSG");
                if round == 2 {
                    assert_eq!(message.body.get_i32("ping").expect("ping"), 1);
                    let mut b = [0];
                    assert_eq!(read(&mut s, &mut b).await.expect("EOF"), 0);
                    break;
                }
                let body = RawDocumentBuf::try_from(&if round == 0 {
                    doc! {"ok":1,"maxWireVersion":27}
                } else {
                    doc! {"ok":1,"answer":42}
                })
                .expect("BSON");
                wire::encode(&mut reply, 99, message.request_id, 0, &body, &[], 16384)
                    .expect("encode");
                write_all(&mut s, &reply).await.expect("reply");
            }
        })
        .expect("spawn");
    let mut client = ex
        .spawn_local(async move {
            let at = h.now() + Duration::from_secs(5);
            let mut c = Connection::connect(
                &h,
                address,
                Options::parse("mongodb://127.0.0.1/?directConnection=true").expect("options"),
                None,
                at,
            )
            .await
            .expect("connect");
            let command =
                RawDocumentBuf::try_from(&doc! {"ping":1,"$db":"admin"}).expect("command");
            let mut replies = 0;
            c.command(&command, &[], at, |reply| {
                assert_eq!(reply.get_i32("answer").expect("answer"), 42);
                replies += 1;
                Ok(())
            })
            .await
            .expect("command");
            assert_eq!(replies, 1);
            assert_eq!(
                c.command(
                    &command,
                    &[],
                    h.now() + Duration::from_millis(2),
                    |_| panic!("no reply expected")
                )
                .await
                .expect_err("timeout")
                .kind(),
                std::io::ErrorKind::TimedOut
            );
            assert!(!c.is_reusable());
        })
        .expect("spawn");
    drive(&mut ex, &mut client);
    drive(&mut ex, &mut server);
}
#[test]
fn warmed_async_client_selection_pool_and_reply_allocate_zero() {
    for preference in [
        "primary",
        "primaryPreferred",
        "secondary",
        "secondaryPreferred",
        "nearest",
    ] {
        warmed_selection(preference);
    }
}
fn warmed_selection(preference: &str) {
    use turnloop_mongodb::{
        asynchronous::{Client, ConnectOptions},
        operation::OperationKind,
    };
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server=ex.spawn_local(async move {
        // Dedicated monitor socket remains idle for the 60-second heartbeat.
        let mut monitor=listener.accept().await.expect("monitor");let m=mongo_packet(&mut monitor).await;let m=wire::Message::parse(&m,16384).expect("hello");
        let hello=RawDocumentBuf::try_from(&doc!{"ok":1,"ismaster":true,"isWritablePrimary":true,"minWireVersion":0,"maxWireVersion":27}).expect("hello");let mut reply=Vec::new();wire::encode(&mut reply,1,m.request_id,0,&hello,&[],16384).expect("encode");write_all(&mut monitor,&reply).await.expect("hello");
        let mut s=listener.accept().await.expect("application");let m=mongo_packet(&mut s).await;let m=wire::Message::parse(&m,16384).expect("hello");wire::encode(&mut reply,1,m.request_id,0,&hello,&[],16384).expect("encode");write_all(&mut s,&reply).await.expect("hello");
        let body=RawDocumentBuf::try_from(&doc!{"ok":1,"answer":42}).expect("reply");
        for _ in 0..1001 {let m=mongo_packet(&mut s).await;let m=wire::Message::parse(&m,16384).expect("command");assert_eq!(m.body.get_i32("ping").expect("ping"),1);wire::encode(&mut reply,1,m.request_id,0,&body,&[],16384).expect("encode");write_all(&mut s,&reply).await.expect("reply");}
        1001
    }).expect("server");
    let preference = preference.to_owned();
    let mut client = ex
        .spawn_local(async move {
            count::prove_counter().await;
            let at = h.now() + Duration::from_secs(30);
            let mut c = Client::connect(
                &h,
                ConnectOptions {
                    protocol: Options::parse(&format!(
                        "mongodb://{address}/?directConnection=true&heartbeatFrequencyMS=60000&readPreference={preference}"
                    ))
                    .expect("options"),
                    tls: None,
                },
                at,
            )
            .await
            .expect("client");
            let command =
                RawDocumentBuf::try_from(&doc! {"ping":1,"$db":"admin"}).expect("command");
            let mut replies = 0;
            for i in 0..1001 {
                let (result, n) = count::measure(c.command(
                    &command,
                    &[],
                    OperationKind::RunCommand,
                    at,
                    |reply| {
                        assert_eq!(reply.get_i32("answer").expect("answer"), 42);
                        replies += 1;
                        Ok(())
                    },
                ))
                .await;
                result.expect("command");
                if i > 0 {
                    assert_eq!(n, 0, "selection, CMAP checkout, coordinator and reply");
                }
            }
            assert_eq!(replies, 1001);
            assert_eq!(c.heartbeat_count(), 1);
        })
        .expect("client");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 1001);
}
#[test]
fn cancelled_client_operation_reconnects_and_sdam_idle_parks() {
    use turnloop_mongodb::{
        asynchronous::{Client, ConnectOptions},
        operation::OperationKind,
    };
    async fn hello(s: &mut AsyncIo<Platform>) {
        let bytes = mongo_packet(s).await;
        let message = wire::Message::parse(&bytes, 16384).expect("hello");
        let body=RawDocumentBuf::try_from(&doc!{"ok":1,"ismaster":true,"isWritablePrimary":true,"minWireVersion":0,"maxWireVersion":27}).expect("body");
        let mut reply = Vec::new();
        wire::encode(&mut reply, 1, message.request_id, 0, &body, &[], 16384).expect("encode");
        write_all(s, &reply).await.expect("hello reply");
    }
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let listener = Listener::bind(&h, "127.0.0.1:0".parse().expect("address")).expect("listen");
    let address = listener.local_addr().expect("address");
    let mut server = ex
        .spawn_local(async move {
            let mut monitor = listener.accept().await.expect("monitor");
            hello(&mut monitor).await;
            let mut first = listener.accept().await.expect("first");
            hello(&mut first).await;
            let bytes = mongo_packet(&mut first).await;
            let message = wire::Message::parse(&bytes, 16384).expect("command");
            assert_eq!(message.body.get_i32("ping").expect("ping"), 1);
            let mut b = [0];
            assert_eq!(read(&mut first, &mut b).await.expect("cancel EOF"), 0);
            let mut next = listener.accept().await.expect("replacement");
            hello(&mut next).await;
            let bytes = mongo_packet(&mut next).await;
            let message = wire::Message::parse(&bytes, 16384).expect("command");
            assert_eq!(message.body.get_i32("ping").expect("ping"), 1);
            let body = RawDocumentBuf::try_from(&doc! {"ok":1,"answer":42}).expect("body");
            let mut reply = Vec::new();
            wire::encode(&mut reply, 1, message.request_id, 0, &body, &[], 16384).expect("encode");
            write_all(&mut next, &reply).await.expect("reply");
            assert_eq!(read(&mut next, &mut b).await.expect("client drop EOF"), 0);
            2
        })
        .expect("server");
    let idle = std::rc::Rc::new(std::cell::Cell::new(false));
    let client_idle = idle.clone();
    let mut client=ex.spawn_local(async move {
        let at=h.now()+Duration::from_secs(5);
        let mut c=Client::connect(&h,ConnectOptions{protocol:Options::parse(&format!("mongodb://{address}/?directConnection=true&heartbeatFrequencyMS=60000&maxPoolSize=1")).expect("options"),tls:None},at).await.expect("client");
        let body=RawDocumentBuf::try_from(&doc!{"ping":1,"$db":"admin"}).expect("command");
        let result=deadline(&h,h.now()+Duration::from_millis(20),c.command(&body,&[],OperationKind::RunCommand,at,|_|panic!("stalled command replied"))).await;
        assert_eq!(result.expect_err("outer future cancelled").kind(),std::io::ErrorKind::TimedOut);
        let mut replies=0;c.command(&body,&[],OperationKind::RunCommand,at,|r|{assert_eq!(r.get_i32("answer").expect("answer"),42);replies+=1;Ok(())}).await.expect("client reusable after cancellation");assert_eq!(replies,1);assert_eq!(c.heartbeat_count(),1);
        client_idle.set(true);h.sleep(Duration::from_millis(60)).await.expect("idle");
    }).expect("client");
    let end = ex.handle().now() + Duration::from_secs(10);
    while !idle.get() {
        assert!(ex.handle().now() < end);
        ex.turn(Timeout::Until(end)).expect("turn");
    }
    let mut waited = false;
    for _ in 0..8 {
        let before = ex.handle().now();
        let info = ex.turn(Timeout::Until(end)).expect("turn");
        if ex.handle().now().duration_since(before) >= Duration::from_millis(10) {
            assert_eq!(info.os_waits, 1);
            waited = true;
            break;
        }
    }
    assert!(waited, "SDAM and idle CMAP must park");
    drive(&mut ex, &mut client);
    assert_eq!(drive(&mut ex, &mut server), 2);
}
