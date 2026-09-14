#![cfg(all(
    target_os = "wasi",
    any(
        target_env = "p2",
        all(target_env = "p3", feature = "wasi-p3-experimental")
    )
))]
use turnloop::backend::Platform;
use turnloop_contract as contract;
#[test]
fn bounded_wait() {
    contract::bounded_turn::<Platform>();
}
#[test]
fn running_notify() {
    contract::notify_running::<Platform>();
}
#[test]
fn tcp_one() {
    contract::tcp_echo::<Platform>(1);
}
#[test]
fn tcp_64() {
    contract::tcp_echo::<Platform>(64);
}
#[test]
fn cancel_close() {
    contract::cancel_close_ordering::<Platform>();
}
#[test]
fn refused_connect() {
    contract::refused_connect_once::<Platform>();
}
#[test]
fn ref_unref() {
    contract::ref_unref::<Platform>();
}
#[test]
fn timer_precision() {
    contract::timer_precision::<Platform>();
}
#[test]
fn udp() {
    contract::udp_round_trip::<Platform>();
}
#[test]
fn writev_shutdown() {
    contract::writev_and_shutdown::<Platform>();
}
#[test]
fn capacity_stale_ids() {
    contract::capacity_and_stale_ids::<Platform>();
}
#[test]
fn pooled_backpressure() {
    contract::pooled_lease_backpressure::<Platform>();
}
#[test]
fn timer_liveness() {
    contract::ready_timer_liveness::<Platform>();
}
#[test]
fn timers_io_posts() {
    contract::io_and_posts_progress_with_repeating_timers::<Platform>();
}
#[test]
fn no_spin() {
    contract::no_spin::<Platform>();
}

#[test]
fn repeated_eof_shutdown_and_empty_datagram() {
    use std::time::Duration;
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("loop");
    let (_, a, b) = contract::pair(&mut l);
    let mut out = Completions::with_capacity(1);
    let mut shutdowns = 0;
    let mut eofs = 0;
    for _ in 0..3 {
        l.shutdown(a, Token(1)).expect("shutdown");
        let until = l.now() + Duration::from_secs(2);
        while shutdowns <= eofs {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out)
                .expect("shutdown turn");
            for c in out.drain() {
                assert!(matches!(c.result, OpResult::Shutdown));
                shutdowns += 1;
            }
        }
        l.read(b, ReadBuf::Pooled, Token(2)).expect("EOF read");
        while eofs < shutdowns {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("EOF turn");
            for c in out.drain() {
                assert!(matches!(c.result, OpResult::Eof));
                eofs += 1;
            }
        }
    }
    assert_eq!((shutdowns, eofs), (3, 3));
    let addr = "127.0.0.1:0".parse().expect("address");
    let a = l.udp_bind(addr, &UdpOpts::default()).expect("UDP");
    let b = l.udp_bind(addr, &UdpOpts::default()).expect("UDP");
    l.recv(b, ReadBuf::Pooled, Token(3)).expect("receive");
    l.send_to(
        a,
        WriteBuf::Owned(Vec::new()),
        l.local_addr(b).expect("address"),
        Token(4),
    )
    .expect("send empty");
    let until = l.now() + Duration::from_secs(2);
    let mut received = 0;
    let mut written = 0;
    while received + written < 2 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("UDP turn");
        for c in out.drain() {
            match c.result {
                OpResult::RecvFrom {
                    n: 0,
                    lease: Some(b),
                    ..
                } => {
                    assert!(b.as_slice().is_empty());
                    received += 1;
                }
                OpResult::Wrote(0) => written += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert_eq!((received, written), (1, 1));
}

#[test]
fn cancelled_head_restarts_queued_read() {
    use std::time::Duration;
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("loop");
    let (_, a, b) = contract::pair(&mut l);
    let mut out = Completions::with_capacity(1);
    let mut reads = 0;
    for _ in 0..32 {
        let first = l.read(b, ReadBuf::Pooled, Token(1)).expect("head");
        let second = l.read(b, ReadBuf::Pooled, Token(2)).expect("successor");
        l.turn(Timeout::Now, &mut out).expect("arm head");
        assert!(out.is_empty());
        assert!(l.cancel(first));
        l.turn(Timeout::Now, &mut out).expect("cancel ack");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op, Some(first));
        assert!(matches!(out[0].result, OpResult::Cancelled));
        l.write(a, WriteBuf::Owned(vec![0x61]), Token(3))
            .expect("write");
        let until = l.now() + Duration::from_secs(1);
        let mut got = false;
        let mut wrote = false;
        while !got || !wrote {
            assert!(l.now() < until, "cancelled head stranded successor");
            l.turn(Timeout::Until(until), &mut out)
                .expect("successor turn");
            for c in out.drain() {
                match c.result {
                    OpResult::Read {
                        n: 1,
                        lease: Some(bytes),
                    } => {
                        assert_eq!(c.op, Some(second));
                        assert_eq!(bytes.as_slice(), [0x61]);
                        got = true;
                        reads += 1;
                    }
                    OpResult::Wrote(1) => wrote = true,
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
    }
    assert_eq!(reads, 32);
}
