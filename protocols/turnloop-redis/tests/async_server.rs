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
        let mut cluster=ClusterClient::connect(&h,cluster_options.clone(),at).await.expect("cluster");assert_eq!(cluster.command(&[b"SET",b"{async}key",b"value"],at).await.expect("cluster SET").bytes(),Some(b"OK".as_slice()));assert_eq!(cluster.command(&[b"GET",b"{async}key"],at).await.expect("cluster GET").bytes(),Some(b"value".as_slice()));
        exercise_cluster(&h, &cluster_options, &mut cluster, at).await;
        let mut master=sentinel(&h,vec![Endpoint{host:"127.0.0.1".into(),port:sentinel_options.address.port()}],"turnloop".into(),&sentinel_options,&plain,at).await.expect("Sentinel ROLE");assert_eq!(master.command(&[b"PING"],at).await.expect("discovered PING").bytes(),Some(b"PONG".as_slice()));
        9
    }).expect("spawn");
    assert_eq!(drive(&mut ex, &mut task), 9);
}
async fn exercise_cluster(
    h: &ExecutorHandle<Platform>,
    options: &ConnectOptions,
    cluster: &mut ClusterClient<Platform>,
    at: Instant,
) {
    use turnloop_redis::routing::key_slot;
    let key = (0..10000)
        .map(|i| format!("async-migrate-{i}"))
        .find(|k| {
            cluster
                .slots()
                .endpoint(key_slot(k.as_bytes()))
                .expect("slot")
                .port
                != options.address.port()
        })
        .expect("foreign key");
    let slot = key_slot(key.as_bytes());
    let slot_text = slot.to_string();
    let source = cluster.slots().endpoint(slot).expect("source").clone();
    let mut source_options = options.clone();
    source_options.address.set_port(source.port);
    let mut src = Client::connect(h, &source_options, at)
        .await
        .expect("source");
    let mut dst = Client::connect(h, options, at).await.expect("destination");
    src.command(&[b"DEL", key.as_bytes()], at)
        .await
        .expect("empty slot key");
    let source_id = src
        .command(&[b"CLUSTER", b"MYID"], at)
        .await
        .expect("source id");
    let dest_id = dst
        .command(&[b"CLUSTER", b"MYID"], at)
        .await
        .expect("destination id");
    dst.command(
        &[
            b"CLUSTER",
            b"SETSLOT",
            slot_text.as_bytes(),
            b"IMPORTING",
            source_id.bytes().expect("id"),
        ],
        at,
    )
    .await
    .expect("importing");
    src.command(
        &[
            b"CLUSTER",
            b"SETSLOT",
            slot_text.as_bytes(),
            b"MIGRATING",
            dest_id.bytes().expect("id"),
        ],
        at,
    )
    .await
    .expect("migrating");
    assert_eq!(
        cluster
            .command(&[b"SET", key.as_bytes(), b"asked"], at)
            .await
            .expect("ASK routed SET")
            .bytes(),
        Some(b"OK".as_slice())
    );
    assert_eq!(
        cluster.slots().endpoint(slot).expect("ASK keeps slot").port,
        source.port
    );
    assert_eq!(
        cluster
            .command(&[b"GET", key.as_bytes()], at)
            .await
            .expect("ASK GET")
            .bytes(),
        Some(b"asked".as_slice())
    );
    dst.command(
        &[
            b"CLUSTER",
            b"SETSLOT",
            slot_text.as_bytes(),
            b"NODE",
            dest_id.bytes().expect("id"),
        ],
        at,
    )
    .await
    .expect("destination owns slot");
    src.command(
        &[
            b"CLUSTER",
            b"SETSLOT",
            slot_text.as_bytes(),
            b"NODE",
            dest_id.bytes().expect("id"),
        ],
        at,
    )
    .await
    .expect("source releases slot");
    assert_eq!(
        cluster
            .command(&[b"GET", key.as_bytes()], at)
            .await
            .expect("MOVED GET")
            .bytes(),
        Some(b"asked".as_slice())
    );
    assert_eq!(
        cluster
            .slots()
            .endpoint(slot)
            .expect("MOVED updates slot")
            .port,
        options.address.port()
    );
    // Fixture-only graceful restart (save config/data and exec). Send once via
    // raw I/O, so reconnect cannot replay a second restart.
    // https://github.com/redis/redis/blob/8.4/src/debug.c
    let before = dst
        .command(&[b"INFO", b"SERVER"], at)
        .await
        .expect("before restart");
    let run_id = |v: &Value| {
        std::str::from_utf8(v.bytes().expect("INFO"))
            .expect("utf8")
            .lines()
            .find_map(|l| l.strip_prefix("run_id:"))
            .expect("run id")
            .to_owned()
    };
    let before = run_id(&before);
    let reconnects = cluster.reconnect_count();
    assert_eq!(
        dst.command(&[b"SAVE"], at)
            .await
            .expect("persist restart data")
            .bytes(),
        Some(b"OK".as_slice())
    );
    let mut control = h
        .connect(options.address, Default::default())
        .await
        .expect("restart socket");
    write_all(
        &mut control,
        b"*3\r\n$5\r\nDEBUG\r\n$7\r\nRESTART\r\n$2\r\n50\r\n",
    )
    .await
    .expect("restart command");
    drop(control);
    h.sleep(Duration::from_secs(3))
        .await
        .expect("restart timer");
    let after = dst
        .command(&[b"INFO", b"SERVER"], at)
        .await
        .expect("restarted INFO");
    assert_ne!(before, run_id(&after));
    assert_eq!(
        cluster
            .command(&[b"GET", key.as_bytes()], at)
            .await
            .expect("cluster reconnect")
            .bytes(),
        Some(b"asked".as_slice())
    );
    assert!(cluster.reconnect_count() > reconnects);
    cluster
        .command(&[b"DEL", key.as_bytes()], at)
        .await
        .expect("cleanup key");
    src.command(
        &[
            b"CLUSTER",
            b"SETSLOT",
            slot_text.as_bytes(),
            b"NODE",
            source_id.bytes().expect("id"),
        ],
        at,
    )
    .await
    .expect("restore source");
    dst.command(
        &[
            b"CLUSTER",
            b"SETSLOT",
            slot_text.as_bytes(),
            b"NODE",
            source_id.bytes().expect("id"),
        ],
        at,
    )
    .await
    .expect("restore destination");
}
