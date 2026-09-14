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

fn port(name: &str) -> u16 {
    std::env::var(name)
        .expect("required private fixture port; run scripts/test-servers.py")
        .parse()
        .expect("port")
}
fn tls(ca: Vec<u8>) -> turnloop_tls::asynchronous::ClientTls {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall time")
        .as_secs();
    turnloop_tls::asynchronous::ClientTls {
        config: turnloop_tls::ClientConfig::new(
            turnloop_tls::ClientOptions {
                ca: Some(vec![turnloop_tls::rustls::pki_types::CertificateDer::from(
                    ca,
                )]),
                alpn: vec![],
                ..Default::default()
            },
            now,
        )
        .expect("TLS config"),
        server_name: "localhost".try_into().expect("name"),
        unix_seconds: now,
    }
}
use turnloop_redis::{
    Event,
    asynchronous::{Client, ClusterClient, ConnectOptions, sentinel},
    resp::Value,
    routing::Endpoint,
};
fn options(port: u16) -> ConnectOptions {
    ConnectOptions {
        address: ([127, 0, 0, 1], port).into(),
        protocol: turnloop_redis::Config {
            username: Some("lane".into()),
            password: Some(
                std::env::var("TURNLOOP_TEST_REDIS_PASSWORD").expect("fixture password"),
            ),
            ..Default::default()
        },
        tls: None,
        retry_delay: Duration::from_millis(5),
        max_reconnects: 3,
    }
}
#[test]
#[ignore = "private Redis fixture"]
fn real_async_tls_pipeline_pubsub_deadlines_reconnect_cluster_sentinel() {
    let mut ex = LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");
    let h = ex.handle();
    let plain = options(port("TURNLOOP_TEST_REDIS_PORT"));
    let mut secure = options(port("TURNLOOP_TEST_REDIS_TLS_PORT"));
    secure.protocol.tls = true;
    secure.tls = Some(tls(include_bytes!("fixtures/ca.der").to_vec()));
    let mut cluster_options = options(port("TURNLOOP_TEST_REDIS_CLUSTER_PORT"));
    cluster_options.protocol.username = None;
    cluster_options.protocol.password = None;
    let mut sentinel_options = cluster_options.clone();
    sentinel_options
        .address
        .set_port(port("TURNLOOP_TEST_REDIS_SENTINEL_PORT"));
    let mut task=ex.spawn_local(async move{
        let at=h.now()+Duration::from_secs(30);let mut c=Client::connect(&h,&plain,at).await.expect("connect");let mut secure=Client::connect(&h,&secure,at).await.expect("TLS connect");
        assert_eq!(secure.command(&[b"PING"],at).await.expect("TLS PING").bytes(),Some(b"PONG".as_slice()));
        c.command(&[b"DEL",b"async-counter"],at).await.expect("DEL");let mut replies=0;
        c.pipeline(&[&[b"INCR",b"async-counter"],&[b"INCR",b"async-counter"]],at,|i,r|{assert_eq!(r.expect("reply"),Value::Integer(i as i64+1));replies+=1;Ok(())}).await.expect("pipeline");assert_eq!(replies,2);
        let id=c.command(&[b"CLIENT",b"ID"],at).await.expect("id");let Value::Integer(id)=id else{panic!("client id")};
        let id=id.to_string();assert_eq!(secure.command(&[b"CLIENT",b"KILL",b"ID",id.as_bytes()],at).await.expect("kill"),Value::Integer(1));
        assert_eq!(c.command(&[b"GET",b"async-counter"],at).await.expect("reconnect GET").bytes(),Some(b"2".as_slice()));assert_eq!(c.reconnect_count(),1);
        let mut subscriber=c.subscribe(&[b"async-channel"],at).await.expect("subscribe");assert_eq!(secure.command(&[b"PUBLISH",b"async-channel",b"payload"],at).await.expect("publish"),Value::Integer(1));assert!(matches!(subscriber.next(at).await.expect("message"),Event::Message{channel,payload,..}if channel==b"async-channel"&&payload==b"payload"));drop(subscriber);
        assert_eq!(secure.command(&[b"BLPOP",b"missing-async-key",b"0"],h.now()+Duration::from_millis(10)).await.expect_err("blocking timeout").kind(),std::io::ErrorKind::TimedOut);assert!(!secure.is_connected());
        let mut cluster=ClusterClient::connect(&h,cluster_options,at).await.expect("cluster");assert_eq!(cluster.command(&[b"SET",b"{async}key",b"value"],at).await.expect("cluster SET").bytes(),Some(b"OK".as_slice()));assert_eq!(cluster.command(&[b"GET",b"{async}key"],at).await.expect("cluster GET").bytes(),Some(b"value".as_slice()));
        let mut master=sentinel(&h,vec![Endpoint{host:"127.0.0.1".into(),port:sentinel_options.address.port()}],"turnloop".into(),&sentinel_options,&plain,at).await.expect("Sentinel ROLE");assert_eq!(master.command(&[b"PING"],at).await.expect("discovered PING").bytes(),Some(b"PONG".as_slice()));
        9
    }).expect("spawn");
    assert_eq!(drive(&mut ex, &mut task), 9);
}
