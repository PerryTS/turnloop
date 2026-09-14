#![cfg(not(target_arch = "wasm32"))]
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use turnloop_http::http1::{self, BodyLength, Encoder};
use turnloop_websocket::*;
fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_message_size(Some(1024))
        .max_frame_size(Some(1024))
}
#[test]
fn handshakes_subprotocol_and_bad_key() {
    let (client, request) =
        ClientHandshake::new("localhost", "/ws", [7; 16], vec!["chat".into()]).unwrap();
    let (mut response, protocol) = accept(&request, &["chat"]).unwrap();
    assert_eq!(protocol.as_deref(), Some("chat"));
    assert_eq!(client.verify(&response).unwrap().as_deref(), Some("chat"));
    response
        .headers
        .iter_mut()
        .find(|h| h.name == "sec-websocket-accept")
        .unwrap()
        .value = b"bad".to_vec();
    assert!(client.verify(&response).is_err());
    assert!(ClientHandshake::new("localhost", "/", [0; 16], vec!["a".into(), "a".into()]).is_err());
}
#[test]
fn masking_fragmentation_ping_pong_close_and_limits() {
    let mut client = Connection::new(Role::Client, config());
    let mut server = Connection::new(Role::Server, config());
    let mut wire = Vec::new();
    client.send(Message::text("hello"), &mut wire).unwrap();
    assert_ne!(wire[1] & 128, 0);
    let mut reply = Vec::new();
    let mut got = None;
    for b in &wire {
        let event = server.receive(&[*b], &mut reply).unwrap();
        assert_eq!(event.consumed, 1);
        if event.message.is_some() {
            got = event.message;
        }
    }
    assert_eq!(got, Some(Message::text("hello")));
    wire.clear();
    client
        .send(Message::Ping(b"ping".as_slice().into()), &mut wire)
        .unwrap();
    assert!(matches!(
        server.receive(&wire, &mut reply).unwrap().message,
        Some(Message::Ping(_))
    ));
    server.flush(&mut reply).unwrap();
    assert_eq!(
        client.receive(&reply, &mut Vec::new()).unwrap().message,
        Some(Message::Pong(b"ping".as_slice().into()))
    );
    use tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    wire.clear();
    server
        .send(
            Message::Frame(Frame::message(
                b"frag".as_slice(),
                OpCode::Data(Data::Text),
                false,
            )),
            &mut wire,
        )
        .unwrap();
    server
        .send(
            Message::Frame(Frame::message(
                b"ment".as_slice(),
                OpCode::Data(Data::Continue),
                true,
            )),
            &mut wire,
        )
        .unwrap();
    assert_eq!(
        client.receive(&wire, &mut Vec::new()).unwrap().message,
        Some(Message::text("fragment"))
    );
    let mut limited = Connection::new(Role::Server, config().max_message_size(Some(2)));
    let mut client = Connection::new(Role::Client, config());
    wire.clear();
    client.send(Message::text("too big"), &mut wire).unwrap();
    let error = limited.receive(&wire, &mut Vec::new()).err().unwrap();
    assert_eq!(node_error_code(&error), "WS_ERR_UNSUPPORTED_MESSAGE_LENGTH");
    wire.clear();
    let deadline = Instant::now();
    client.close(None, deadline, &mut wire).unwrap();
    assert!(!wire.is_empty());
    assert_eq!(client.handle_timeout(deadline), Some(1006));
    assert_eq!(client.handle_timeout(deadline), None);
}
fn serve(mut socket: TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut decoder = http1::Decoder::new(http1::Mode::Request, Default::default());
    let mut input = Vec::new();
    let head = loop {
        let step = decoder.receive(&input).unwrap();
        if let Some(http1::Event::Head(head)) = step.event {
            input.drain(..step.consumed);
            break head;
        }
        let mut b = [0; 1024];
        let n = socket.read(&mut b).unwrap();
        assert!(n > 0);
        input.extend_from_slice(&b[..n]);
    };
    let (response, protocol) = accept(&head, &["chat"]).unwrap();
    assert_eq!(protocol.as_deref(), Some("chat"));
    let mut output = Vec::new();
    Encoder::start(&response, BodyLength::Empty, &mut output).unwrap();
    socket.write_all(&output).unwrap();
    output.clear();
    let mut ws = Connection::new(Role::Server, config());
    let mut messages = 0;
    loop {
        let event = ws.receive(&input, &mut output).unwrap();
        input.drain(..event.consumed);
        if let Some(message) = event.message {
            match message {
                Message::Text(text) => {
                    assert_eq!(text, "hello from node");
                    messages += 1;
                    ws.send(Message::text("hello from rust"), &mut output)
                        .unwrap();
                }
                Message::Close(frame) => {
                    assert_eq!(
                        frame.unwrap().code,
                        tungstenite::protocol::frame::coding::CloseCode::Normal
                    );
                    let result = ws.flush(&mut output);
                    assert!(result.is_ok() || matches!(result, Err(Error::ConnectionClosed)));
                    socket.write_all(&output).unwrap();
                    break;
                }
                _ => panic!("unexpected message"),
            }
        }
        socket.write_all(&output).unwrap();
        output.clear();
        if input.is_empty() {
            let mut b = [0; 1024];
            let n = socket.read(&mut b).unwrap();
            assert!(n > 0);
            input.extend_from_slice(&b[..n]);
        }
    }
    assert_eq!(messages, 1);
}
#[test]
fn node26_websocket_client_against_native_server() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || serve(listener.accept().unwrap().0));
    let output=Command::new("node").args(["--input-type=module","-e",r#"setTimeout(()=>process.exit(70),10000).unref();const ws=new WebSocket(process.argv[1],['chat']);ws.onopen=()=>{if(ws.protocol!=='chat')process.exit(2);ws.send('hello from node');};ws.onmessage=e=>{if(e.data!=='hello from rust')process.exit(3);ws.close(1000,'done');};ws.onclose=e=>{if(e.code!==1000||!e.wasClean)process.exit(4);console.log('websocket verified');};ws.onerror=e=>{console.error(e);process.exit(5);};"#,&format!("ws://{address}/ws")]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "websocket verified"
    );
    server.join().unwrap();
}
