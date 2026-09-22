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
/// Three masked client text frames, the last cut after 5 of its 11 bytes: the
/// shape a transport read that splits a frame produces.
fn three_messages_last_split() -> (Vec<u8>, Vec<u8>) {
    let mut client = Connection::new(Role::Client, config());
    let mut wire = Vec::new();
    for text in ["one", "two", "three"] {
        client.send(Message::text(text), &mut wire).unwrap();
    }
    assert_eq!(wire.len(), 9 + 9 + 11);
    let rest = wire.split_off(9 + 9 + 5);
    (wire, rest)
}
/// Drive with the documented condition, `consumed > 0 || message.is_some()`.
fn drain(server: &mut Connection, input: &mut Vec<u8>, seen: &mut Vec<(usize, Option<Message>)>) {
    let mut reply = Vec::new();
    loop {
        let step = server.receive(input, &mut reply).unwrap();
        let progressed = step.consumed > 0 || step.message.is_some();
        input.drain(..step.consumed);
        seen.push((step.consumed, step.message));
        if !progressed {
            return;
        }
    }
}
/// `Received`'s shapes (PerryTS/turnloop#86). tungstenite reads input into its
/// own buffer and parses from that buffer before it reads again, so a message
/// can complete from bytes an earlier call consumed: `consumed == 0` with a
/// message is normal, and `consumed == 0` alone does not mean "stop".
#[test]
fn received_consumed_and_message_are_independent() {
    let (mut input, rest) = three_messages_last_split();
    let mut server = Connection::new(Role::Server, config());
    let mut seen = Vec::new();
    drain(&mut server, &mut input, &mut seen);
    assert_eq!(
        seen,
        vec![
            // Everything was read in; the first message came out of it.
            (23, Some(Message::text("one"))),
            // consumed == 0 with a message: parsed from the buffer. KEEP GOING.
            (0, Some(Message::text("two"))),
            // consumed == 0, no message: 5 bytes of a frame header. STOP.
            (0, None),
        ]
    );
    assert!(input.is_empty());

    let mut input = rest;
    let mut seen = Vec::new();
    drain(&mut server, &mut input, &mut seen);
    assert_eq!(seen, vec![(6, Some(Message::text("three"))), (0, None)]);

    // consumed > 0 with no message: a partial frame, all of it taken in.
    // Calling again is harmless and returns the stop shape.
    let (mut input, _) = three_messages_last_split();
    input.truncate(5);
    let mut server = Connection::new(Role::Server, config());
    let mut seen = Vec::new();
    drain(&mut server, &mut input, &mut seen);
    assert_eq!(seen, vec![(5, None), (0, None)]);
}
/// The conditions the step contract rules out, and the one it does not.
#[test]
fn received_rejects_the_wrong_loop_conditions() {
    // "Loop while bytes were consumed" loses "two": it arrives in a step that
    // consumed nothing, which this loop reads as the stop signal.
    let (mut input, rest) = three_messages_last_split();
    let mut server = Connection::new(Role::Server, config());
    let mut reply = Vec::new();
    let mut messages = Vec::new();
    loop {
        let step = server.receive(&input, &mut reply).unwrap();
        if step.consumed == 0 {
            break;
        }
        input.drain(..step.consumed);
        messages.extend(step.message);
    }
    input.extend_from_slice(&rest);
    loop {
        let step = server.receive(&input, &mut reply).unwrap();
        if step.consumed == 0 {
            break;
        }
        input.drain(..step.consumed);
        messages.extend(step.message);
    }
    assert_eq!(
        messages,
        vec![Message::text("one"), Message::text("three")],
        "\"two\" came back with consumed == 0 and was dropped"
    );

    // "Read the transport whenever the input is empty" stalls: after "one"
    // there is no input left, but "two" is complete in the connection's own
    // buffer. A host that waits for the peer here waits forever if the peer
    // is waiting for an answer to "two".
    let (mut input, _) = three_messages_last_split();
    let mut server = Connection::new(Role::Server, config());
    let step = server.receive(&input, &mut reply).unwrap();
    input.drain(..step.consumed);
    assert_eq!(step.message, Some(Message::text("one")));
    assert!(input.is_empty(), "nothing left to hand in...");
    let step = server.receive(&input, &mut reply).unwrap();
    assert_eq!(
        (step.consumed, step.message),
        (0, Some(Message::text("two"))),
        "...and yet a message was waiting"
    );

    // "Loop while a message came back" is safe for this type, unlike
    // http2::Step: a step without a message has always taken in all of its
    // input, so nothing is stranded when such a host goes back to the
    // transport. Pinned here because it rests on how tungstenite reads.
    let (mut input, rest) = three_messages_last_split();
    let mut server = Connection::new(Role::Server, config());
    let mut messages = Vec::new();
    for chunk in [None, Some(rest)] {
        input.extend(chunk.into_iter().flatten());
        loop {
            let step = server.receive(&input, &mut reply).unwrap();
            input.drain(..step.consumed);
            let Some(message) = step.message else { break };
            messages.push(message);
        }
        assert!(input.is_empty());
    }
    assert_eq!(
        messages,
        ["one", "two", "three"].map(Message::text).to_vec()
    );
}
/// `receive` is not idempotent. It takes `consumed` bytes into its own buffer,
/// so a host that does not drain them and feeds them again gets every message
/// in them twice - silently, with no error to notice.
#[test]
fn received_is_not_idempotent() {
    let (input, _) = three_messages_last_split();
    let one = &input[..9];
    let mut server = Connection::new(Role::Server, config());
    let mut reply = Vec::new();
    let first = server.receive(one, &mut reply).unwrap();
    assert_eq!(
        (first.consumed, first.message),
        (9, Some(Message::text("one")))
    );
    let again = server.receive(one, &mut reply).unwrap();
    assert_eq!(
        (again.consumed, again.message),
        (9, Some(Message::text("one")))
    );
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
        let progressed = event.consumed > 0 || event.message.is_some();
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
        // Not `input.is_empty()`: a message can be complete in the
        // connection's own buffer with no input left to hand in.
        if !progressed {
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
