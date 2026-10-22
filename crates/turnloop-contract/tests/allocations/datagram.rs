use super::*;
use std::task::{Context, Poll, Waker};

#[test]
fn warmed_datagram_packets_allocate_nothing() {
    let mut ex = LocalExecutor::<backend::Platform>::new(Config::default()).expect("executor");
    let a = ex
        .driver()
        .udp_bind("127.0.0.1:0".parse().expect("address"), &UdpOpts::default())
        .expect("bind a");
    let b = ex
        .driver()
        .udp_bind("127.0.0.1:0".parse().expect("address"), &UdpOpts::default())
        .expect("bind b");
    let source = ex.driver().local_addr(a).expect("source");
    let destination = ex.driver().local_addr(b).expect("destination");
    let mut a = ex.handle().datagram(a);
    let mut b = ex.handle().datagram(b);
    let mut cx = Context::from_waker(Waker::noop());
    let mut completed = 0;
    for round in 0..1001 {
        if round == 1 {
            ALLOCS.with(|v| v.set(0));
            ACTIVE.with(|v| v.set(true));
        }
        let payload = [39; 64];
        let payload = if round % 2 == 0 { &payload[..] } else { &[] };
        let mut bytes = [0xcc; 64];
        let Poll::Ready(result) = a.poll_send_to(&mut cx, payload, destination) else {
            panic!("fresh send must be accepted")
        };
        assert_eq!(result.expect("send"), payload.len());
        assert!(a.poll_flush(&mut cx).is_pending());
        assert!(b.poll_recv_from(&mut cx, &mut bytes).is_pending());
        let mut sent = false;
        let mut received = false;
        let until = ex.driver().now() + Duration::from_secs(2);
        while !sent || !received {
            assert!(ex.driver().now() < until, "packet stalled");
            ex.turn(Timeout::Until(until)).expect("turn");
            if !sent && let Poll::Ready(result) = a.poll_flush(&mut cx) {
                result.expect("flush");
                sent = true;
            }
            if !received && let Poll::Ready(result) = b.poll_recv_from(&mut cx, &mut bytes) {
                let (n, from) = result.expect("receive");
                assert_eq!(from, source);
                assert_eq!(&bytes[..n], payload);
                received = true;
            }
        }
        completed += 1;
    }
    ACTIVE.with(|v| v.set(false));
    assert_eq!(completed, 1001);
    assert_eq!(
        ALLOCS.with(Cell::get),
        0,
        "warmed datagrams allocate nothing"
    );
}
