#![cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
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
