//! Blocking conformance driver only; protocol crate contains no I/O.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use turnloop_http::{
    http1::Header,
    http2::{Connection, Event, Role},
};
const BODY: [u8; 16384] = [b'x'; 16384];

fn serve(mut socket: TcpStream) -> std::io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut engine =
        Connection::new(Role::Server, Default::default()).map_err(std::io::Error::other)?;
    let mut input = Vec::new();
    let mut pending: Vec<(u32, usize)> = Vec::new();
    loop {
        let step = engine.receive(&input);
        let mut stop = false;
        let mut consumed = 0;
        let mut reply = None;
        match step {
            Ok(step) => {
                consumed = step.consumed;
                match step.event {
                    Some(Event::Headers {
                        stream,
                        end_stream: true,
                        ..
                    }) => reply = Some(stream),
                    Some(Event::Data {
                        stream,
                        bytes,
                        end_stream,
                    }) => {
                        let _ = engine.release_capacity(stream, bytes.len() as u32);
                        if end_stream {
                            reply = Some(stream);
                        }
                    }
                    _ => {}
                }
            }
            Err(_) => stop = true,
        }
        if let Some(id) = reply {
            let _ = engine.send_headers(
                id,
                &[
                    Header::new(":status", "200"),
                    Header::new("content-length", "16384"),
                ],
                false,
            );
            pending.push((id, 0));
        }
        pending.retain_mut(
            |(id, pos)| match engine.send_data(*id, &BODY[*pos..], true) {
                Ok(n) => {
                    *pos += n;
                    *pos < BODY.len()
                }
                Err(_) => false,
            },
        );
        if socket.write_all(engine.output()).is_err() {
            break;
        }
        let n = engine.output().len();
        engine.consume_output(n).map_err(std::io::Error::other)?;
        if stop {
            break;
        }
        input.drain(..consumed);
        if consumed == 0 {
            let mut bytes = [0; 32768];
            match socket.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(n) => input.extend_from_slice(&bytes[..n]),
            }
        }
    }
    Ok(())
}
fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    println!("{}", listener.local_addr()?.port());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || {
                    let _ = serve(stream);
                });
            }
            Err(_) => break,
        }
    }
    Ok(())
}
