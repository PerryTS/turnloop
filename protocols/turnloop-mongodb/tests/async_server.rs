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
use turnloop_mongodb::{asynchronous::{Client,ConnectOptions},uri::Options,operation::OperationKind,bson::{doc,Document,raw::RawDocumentBuf}};
fn raw(d:Document)->RawDocumentBuf{RawDocumentBuf::try_from(&d).expect("BSON")}
#[test]
#[ignore="private MongoDB fixture, after real_mongodb bootstrap"]
fn real_async_auth_tls_sdam_pool_cursor_retry_and_primary_stepdown(){
    let port=std::env::var("TURNLOOP_TEST_MONGODB_PORT").expect("standalone port");
    let tls_port=std::env::var("TURNLOOP_TEST_MONGODB_TLS_PORT").expect("TLS port");
    let replica=std::env::var("TURNLOOP_TEST_MONGODB_REPLICA_PORTS").expect("replica ports");let seeds=replica.split(',').map(|p|format!("127.0.0.1:{p}")).collect::<Vec<_>>().join(",");
    use turnloop_tls::rustls::pki_types::{CertificateDer,pem::PemObject};
    let certs=std::fs::read(std::path::PathBuf::from(std::env::var_os("TURNLOOP_TEST_MONGODB_TOOLS").expect("tools")).join("cert.pem")).expect("certs");
    let ca=CertificateDer::pem_slice_iter(&certs).collect::<Result<Vec<_>,_>>().expect("certificates");
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("wall time").as_secs();
    let tls=turnloop_tls::asynchronous::ClientTls{config:turnloop_tls::ClientConfig::new(turnloop_tls::ClientOptions{ca:Some(ca),alpn:vec![],..Default::default()},now).expect("TLS config"),server_name:"localhost".try_into().expect("name"),unix_seconds:now};
    let mut ex=LocalExecutor::<Platform>::new(LoopConfig::default()).expect("executor");let h=ex.handle();
    let mut task=ex.spawn_local(async move{
        let at=h.now()+Duration::from_secs(75);
        for (uri,tls) in [(format!("mongodb://lane:pencil@127.0.0.1:{port}/admin?directConnection=true"),None),(format!("mongodb://lane:pencil@127.0.0.1:{tls_port}/admin?directConnection=true&tls=true"),Some(tls))]{
            let mut c=Client::connect(&h,ConnectOptions{protocol:Options::parse(&uri).expect("options"),tls},at).await.expect("client");
            let mut auth=false;c.command(&raw(doc!{"connectionStatus":1,"$db":"admin"}),&[],OperationKind::RunCommand,at,|r|{let r=Document::try_from(r).expect("document");assert_eq!(r.get_document("authInfo").expect("authInfo").get_array("authenticatedUsers").expect("users").len(),1);auth=true;Ok(())}).await.expect("authenticated command");assert!(auth);
        }
        let mut options=Options::parse(&format!("mongodb://lane:pencil@{seeds}/admin?replicaSet=turnloop_test&heartbeatFrequencyMS=500")).expect("replica options");options.max_pool_size=2;
        let mut c=Client::connect(&h,ConnectOptions{protocol:options,tls:None},at).await.expect("replica client");
        c.command(&raw(doc!{"dropDatabase":1,"$db":"async_lane"}),&[],OperationKind::RunCommand,at,|_|Ok(())).await.expect("drop database");
        c.command(&raw(doc!{"insert":"items","documents":[{"_id":1,"value":42},{"_id":2,"value":43},{"_id":3,"value":44}],"writeConcern":{"w":"majority"},"$db":"async_lane"}),&[],OperationKind::Write,at,|r|{assert_eq!(r.get_i32("n").expect("n"),3);Ok(())}).await.expect("insert");
        let mut cursor=c.cursor(&raw(doc!{"find":"items","filter":{},"batchSize":1,"$db":"async_lane"}),at).await.expect("cursor");let mut rows=0;while let Some(row)=cursor.next(at).await.expect("cursor next"){assert!(row.get_i32("value").expect("value")>=42);rows+=1;}assert_eq!(rows,3);cursor.close(at).await.expect("close cursor");
        let before=c.heartbeat_count();h.sleep(Duration::from_millis(1100)).await.expect("heartbeat timer");assert!(c.heartbeat_count()>before);assert_eq!(c.server_count(),3);
        let mut before_primary=String::new();c.command(&raw(doc!{"hello":1,"$db":"admin"}),&[],OperationKind::RunCommand,at,|r|{before_primary=r.get_str("me").expect("primary me").to_owned();Ok(())}).await.expect("hello");
        let step=c.command(&raw(doc!{"replSetStepDown":5,"force":true,"$db":"admin"}),&[],OperationKind::RunCommand,at,|_|Ok(())).await;if let Err(e) = step {
            let mongo = e.get_ref().and_then(|e| e.downcast_ref::<turnloop_mongodb::Error>()).expect("MongoDB step-down error");
            assert!(matches!(mongo.kind, turnloop_mongodb::ErrorKind::Network) || matches!(mongo.code,Some(91|189|11600|11602)), "unexpected step-down failure: {e}");
        }
        h.sleep(Duration::from_secs(1)).await.expect("election wait");
        let mut after_primary=String::new();c.command(&raw(doc!{"hello":1,"$db":"admin"}),&[],OperationKind::Read,at,|r|{assert!(r.get_bool("isWritablePrimary").expect("primary"));after_primary=r.get_str("me").expect("me").to_owned();Ok(())}).await.expect("reselection");assert_ne!(before_primary,after_primary);
        let mut rows=0;c.command(&raw(doc!{"find":"items","filter":{},"$db":"async_lane"}),&[],OperationKind::Read,at,|r|{rows=turnloop_mongodb::command::CursorBatch::parse(r).expect("batch").rows().count();Ok(())}).await.expect("persisted read");assert_eq!(rows,3);
    }).expect("spawn");drive(&mut ex,&mut task);
}
