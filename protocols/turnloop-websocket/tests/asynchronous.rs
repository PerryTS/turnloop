#![cfg(all(feature="turnloop",not(all(target_arch="wasm32",target_os="unknown"))))]
use std::{future::Future,pin::Pin,task::{Context,Poll,Waker},time::Duration};
use turnloop_io::{*,turnloop::{backend::Platform,LocalExecutor,Config,Timeout}};
use turnloop_websocket::{WebSocketStream,Message};
fn finish<T>(task:&mut turnloop::executor::JoinHandle<T>)->T{match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())){Poll::Ready(Ok(v))=>v,_=>panic!("task incomplete")}}
#[test]
fn async_upgrade_echo_ping_and_clean_close(){
    let mut executor=LocalExecutor::<Platform>::new(Config::default()).expect("executor");let h=executor.handle();let h2=h.clone();
    let listener=Listener::bind(&h,"127.0.0.1:0".parse().expect("address")).expect("listener");let address=listener.local_addr().expect("address");let end=h.now()+Duration::from_secs(5);
    let mut server=executor.spawn_local(async move{
        let stream=listener.accept().await.expect("accept");let(mut ws,protocol)=WebSocketStream::accept(stream,&["echo"],&h,end).await.expect("upgrade");assert_eq!(protocol.as_deref(),Some("echo"));let mut echoed=0;
        while let Some(message)=ws.receive().await.expect("receive"){match message{Message::Text(text)=>{echoed+=1;ws.send(Message::Text(text)).await.expect("echo");},Message::Close(_)=>break,_=>{}}}echoed
    }).expect("spawn");
    let mut client=executor.spawn_local(async move{
        let stream=h2.connect(address,Default::default()).await.expect("connect");let(mut ws,protocol)=WebSocketStream::connect(stream,&address.to_string(),"/",[7;16],vec!["echo".into()],&h2,end).await.expect("upgrade");assert_eq!(protocol.as_deref(),Some("echo"));
        ws.send(Message::Ping(b"probe".as_slice().into())).await.expect("ping");assert_eq!(ws.receive().await.expect("pong"),Some(Message::Pong(b"probe".as_slice().into())));
        for _ in 0..100{ws.send(Message::Text("hello".into())).await.expect("send");assert_eq!(ws.receive().await.expect("echo"),Some(Message::Text("hello".into())));}
        ws.close(&h2,end).await.expect("close");100
    }).expect("spawn");
    while !server.is_finished()||!client.is_finished(){assert!(executor.driver().now()<end);executor.turn(Timeout::Until(end)).expect("turn");}
    assert_eq!(finish(&mut server),finish(&mut client));
}
#[cfg(not(target_arch="wasm32"))]
#[test]
fn node_websocket_against_async_server(){
    use std::process::Command;
    let mut executor=LocalExecutor::<Platform>::new(Config::default()).expect("executor");let h=executor.handle();
    let listener=Listener::bind(&h,"127.0.0.1:0".parse().expect("address")).expect("listener");let address=listener.local_addr().expect("address");let end=h.now()+Duration::from_secs(10);
    let mut task=executor.spawn_local(async move{
        let stream=listener.accept().await.expect("accept");let(mut ws,_)=WebSocketStream::accept(stream,&[],&h,end).await.expect("upgrade");let mut count=0;
        while let Some(message)=ws.receive().await.expect("receive"){match message{Message::Text(text)=>{assert_eq!(text.as_str(),"node async adapter");ws.send(Message::Text(text)).await.expect("echo");count+=1;},Message::Close(_)=>break,_=>{}}}count
    }).expect("spawn");
    let mut node=Command::new("node").args(["-e",&format!("const ws=new WebSocket('ws://{address}');let count=0;ws.onopen=()=>ws.send('node async adapter');ws.onmessage=e=>{{if(e.data!=='node async adapter')process.exit(2);count++;ws.close()}};ws.onclose=e=>{{if(count!==1||!e.wasClean)process.exit(3)}};ws.onerror=()=>process.exit(4);setTimeout(()=>process.exit(5),8000).unref();")]).spawn().expect("Node 26 required");
    while !task.is_finished(){assert!(executor.driver().now()<end);executor.turn(Timeout::Until(end)).expect("turn");}
    assert_eq!(finish(&mut task),1);assert!(node.wait().expect("Node exit").success());
}
