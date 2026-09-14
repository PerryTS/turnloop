#![deny(unsafe_op_in_unsafe_fn)]
//! Real WASI 0.3 futures/streams, packaged using Rust's wasip2 standard library.
use std::{
    cell::Cell,
    future::{Future, IntoFuture, poll_fn},
    pin::pin,
    task::Poll,
};
use wasip3::{
    clocks::monotonic_clock as clock,
    sockets::types::{ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket},
};

async fn join<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let mut ar = None;
    let mut br = None;
    poll_fn(|cx| {
        if ar.is_none()
            && let Poll::Ready(x) = a.as_mut().poll(cx)
        {
            ar = Some(x);
        }
        if br.is_none()
            && let Poll::Ready(x) = b.as_mut().poll(cx)
        {
            br = Some(x);
        }
        if ar.is_some() && br.is_some() {
            Poll::Ready((ar.take().expect("ready A"), br.take().expect("ready B")))
        } else {
            Poll::Pending
        }
    })
    .await
}
async fn timers() {
    let early = Cell::new(0);
    let slow = async {
        clock::wait_for(20_000_000).await;
        assert_eq!(early.get(), 8);
    };
    let fast = async {
        for _ in 0..8 {
            let start = clock::now();
            clock::wait_for(100_000).await;
            let elapsed = clock::now() - start;
            assert!(elapsed >= 100_000);
            assert!(elapsed < 100_000_000);
            early.set(early.get() + 1);
            println!("p3 timer requested_ns=100000 elapsed_ns={elapsed}");
        }
    };
    join(slow, fast).await;
    assert_eq!(early.get(), 8);
    println!("p3 multiwait PASS short_completions=8 long_completions=1");
}
async fn send(socket: &TcpSocket, bytes: Vec<u8>) -> Result<(), ErrorCode> {
    let (mut writer, reader) = wasip3::wit_stream::new::<u8>();
    let done = socket.send(reader);
    let writing = async move {
        let remaining = writer.write_all(bytes).await;
        assert!(remaining.is_empty());
        drop(writer);
    };
    let ((), result) = join(writing, done.into_future()).await;
    result
}
async fn receive(socket: &TcpSocket) -> Result<Vec<u8>, ErrorCode> {
    let (reader, done) = socket.receive();
    let (bytes, result) = join(reader.collect(), done.into_future()).await;
    result?;
    Ok(bytes)
}
async fn echo() -> Result<(), ErrorCode> {
    let listener = TcpSocket::create(IpAddressFamily::Ipv4)?;
    listener.bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
        port: 0,
        address: (127, 0, 0, 1),
    }))?;
    let mut incoming = listener.listen()?;
    let address = listener.get_local_address()?;
    let payload: Vec<u8> = (0..257).map(|i| (i * 73 + 19) as u8).collect();
    let server = async {
        let socket = incoming
            .next()
            .await
            .expect("accept stream must yield a connection");
        let bytes = receive(&socket).await?;
        assert_eq!(bytes, payload);
        let n = bytes.len();
        send(&socket, bytes).await?;
        Ok::<usize, ErrorCode>(n)
    };
    let client = async {
        let socket = TcpSocket::create(IpAddressFamily::Ipv4)?;
        socket.connect(address).await?;
        send(&socket, payload.clone()).await?;
        let bytes = receive(&socket).await?;
        assert_eq!(bytes, payload);
        Ok::<usize, ErrorCode>(bytes.len())
    };
    let (server, client) = join(server, client).await;
    assert_eq!(server?, 257);
    assert_eq!(client?, 257);
    println!("p3 TCP echo PASS server_bytes=257 client_bytes_verified=257");
    Ok(())
}
struct Command;
impl wasip3::exports::cli::run::Guest for Command {
    async fn run() -> Result<(), ()> {
        let network = async {
            echo().await.map_err(|e| {
                eprintln!("p3 echo error: {e:?}");
            })
        };
        let ((), result) = join(timers(), network).await;
        result
    }
}
wasip3::cli::command::export!(Command);
