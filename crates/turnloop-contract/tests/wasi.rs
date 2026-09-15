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
fn queued_core_work() {
    contract::queued_core_work::<Platform>();
}
#[test]
fn queued_post_idle_io() {
    contract::queued_post_idle_io::<Platform>();
}
#[test]
fn queued_terminals_idle_io() {
    contract::queued_terminals_idle_io::<Platform>();
}
#[test]
fn sustained_posts_idle_io() {
    contract::sustained_posts_idle_io::<Platform>();
}
/// DESIGN §10.3: a natively accepted WASI 0.2 lookup is a pending native
/// operation, so queued posts cannot starve its discovery.
#[cfg(target_env = "p2")]
#[test]
fn queued_posts_preserve_native_dns_discovery() {
    use std::time::Duration;
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("loop");
    let lookup = l
        .resolve(
            DnsRequest {
                host: "localhost".into(),
                port: 8080,
            },
            Token(1),
        )
        .expect("native lookup");
    let poster = l.poster();
    let mut out = Completions::with_capacity(1);
    let until = l.now() + Duration::from_secs(2);
    let (mut turns, mut posts, mut resolved, mut discovery_polls) = (0, 0, 0, 0);
    while turns < 64 || resolved == 0 {
        assert!(
            l.now() < until,
            "queued posts starved DNS: turns={turns}, posts={posts}, discovery_polls={discovery_polls}"
        );
        poster.post(Token(2), Payload::U64(7)).expect("replenish");
        let info = l.turn(Timeout::Now, &mut out).expect("turn");
        assert_eq!(info.os_waits, 0, "queued posts forbid blocking waits");
        assert!(info.discovery_polls <= 1);
        if resolved != 0 {
            assert_eq!(info.discovery_polls, 0, "no native operation remains");
        }
        discovery_polls += info.discovery_polls;
        turns += 1;
        assert_eq!(out.len(), 1);
        for c in out.drain() {
            match c.result {
                OpResult::Posted(Payload::U64(7)) => posts += 1,
                OpResult::Resolved(addresses) => {
                    assert_eq!(c.op, Some(lookup));
                    assert!(!addresses.is_empty());
                    assert!(
                        addresses
                            .iter()
                            .all(|a| a.ip().is_loopback() && a.port() == 8080)
                    );
                    resolved += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    eprintln!(
        "queued posts with WASI DNS: turns={turns}, posts={posts}, discovery_polls={discovery_polls}"
    );
    assert_eq!((resolved, posts), (1, turns - 1));
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
/// Socket options (issue #34). `wasi:sockets` exposes keep-alive, buffer sizes
/// and the hop limit and nothing else, so those round-trip through the OS and the
/// rest must report Unsupported instead of being accepted and dropped.
#[test]
fn socket_option_keep_alive() {
    contract::sockopts::keep_alive_round_trip::<Platform>();
}
#[test]
fn socket_option_buffer_sizes() {
    contract::sockopts::buffer_sizes_round_trip::<Platform>();
}
#[test]
fn socket_option_ttl() {
    contract::sockopts::ttl_round_trip::<Platform>();
}
#[test]
fn socket_option_accept_defaults() {
    contract::sockopts::accept_defaults_keep_alive::<Platform>();
}
#[test]
fn socket_option_handle_validation() {
    contract::sockopts::option_handle_validation::<Platform>();
}
#[test]
fn socket_options_without_a_wasi_interface_are_unsupported() {
    use std::time::Duration;
    use turnloop::{MulticastGroup, SocketOption, SocketOptionKind};
    let group = MulticastGroup {
        group: std::net::Ipv4Addr::new(224, 0, 0, 251).into(),
        interface: 0,
    };
    contract::sockopts::unsupported_options_are_reported::<Platform>(&[
        (SocketOption::NoDelay(true), Some(SocketOptionKind::NoDelay)),
        (
            SocketOption::Linger(Some(Duration::ZERO)),
            Some(SocketOptionKind::Linger),
        ),
        (
            SocketOption::Ipv6Only(true),
            Some(SocketOptionKind::Ipv6Only),
        ),
        (
            SocketOption::Broadcast(true),
            Some(SocketOptionKind::Broadcast),
        ),
        (
            SocketOption::MulticastTtl(4),
            Some(SocketOptionKind::MulticastTtl),
        ),
        (
            SocketOption::MulticastLoop(false),
            Some(SocketOptionKind::MulticastLoop),
        ),
        (SocketOption::MulticastJoin(group), None),
        (SocketOption::MulticastLeave(group), None),
    ]);
}
/// `wasi:sockets` has no Nagle control, so a listener asking for it as a
/// per-connection default is refused when it is created.
#[test]
fn nodelay_connect_hint_is_rejected() {
    contract::sockopts::unsupported_connect_nodelay_is_rejected::<Platform>();
}
#[test]
fn nodelay_accept_default_rejects_the_listener() {
    contract::sockopts::unsupported_accept_default_rejects_the_listener::<Platform>(
        turnloop::AcceptDefaults {
            nodelay: true,
            ..turnloop::AcceptDefaults::EMPTY
        },
    );
}
#[test]
fn no_spin() {
    contract::no_spin::<Platform>();
}
/// DESIGN §10.4a and the `PollInfo` contract: the private WASI deadline
/// pollable (0.2) / subtask (0.3) is this wait's timeout, not native work.
#[test]
fn quiet_deadline_accounting() {
    contract::quiet_deadline_accounting::<Platform>();
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
    let from = l.local_addr(a).expect("source address");
    let to = l.local_addr(b).expect("destination address");
    assert_ne!(from, to, "default UDP endpoints must be distinct");
    l.recv(b, ReadBuf::Pooled, Token(3)).expect("receive");
    l.send_to(a, WriteBuf::Owned(Vec::new()), to, Token(4))
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
                    n,
                    from: actual,
                    lease: Some(b),
                } => {
                    assert_eq!(actual, from, "unexpected UDP sender");
                    assert_eq!(n, 0);
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

#[cfg(target_env = "p3")]
#[test]
fn wasi_random_fills_both_getrandom_generations_and_bson() {
    let mut generated = 0;
    for len in [1, 7, 8, 9, 31, 64, 257] {
        let mut first = vec![0; len];
        let mut second = vec![0; len];
        turnloop_wasi_random::fill_v03(&mut first).expect("WASI entropy 0.3");
        turnloop_wasi_random::fill_v04(&mut second).expect("WASI entropy 0.4");
        // Single bytes can legitimately be zero/equal. Assert on long samples.
        if len >= 31 {
            assert!(first.iter().any(|&b| b != 0));
            assert!(first.windows(2).any(|b| b[0] != b[1]));
            assert!(second.windows(2).any(|b| b[0] != b[1]));
            assert_ne!(first, second);
        }
        generated += 2 * len;
    }
    assert_eq!(generated, 754);
    turnloop_wasi_random::fill_v03(&mut []).expect("empty entropy");
    turnloop_wasi_random::fill_v04(&mut []).expect("empty entropy");
    let a = bson::oid::ObjectId::new();
    let b = bson::oid::ObjectId::new();
    assert_ne!(a, b);
    assert!(a.bytes()[4..9].iter().any(|&b| b != 0));
    println!("entropy subject: {generated} bytes, two getrandom generations, two BSON ObjectIds");
}

#[test]
fn revision_two_unsupported_native_capabilities() {
    contract::single_agent::unsupported_native::<Platform>();
}
#[test]
fn external_wait_routing_cancellation_and_capacity() {
    contract::single_agent::waits::<Platform>();
}
#[test]
fn external_wait_deadlines_do_not_spin() {
    use std::time::Duration;
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("loop");
    let (_, _a, b) = contract::pair(&mut l);
    let idle = l.read(b, ReadBuf::Pooled, Token(1)).expect("idle read");
    let condition = WaitCondition::new(0).expect("condition");
    let mut out = Completions::default();
    let mut expiries = 0;
    for micros in [500, 2000, 10000] {
        for _ in 0..20 {
            let at = l.now() + Duration::from_micros(micros);
            let op = l
                .external_wait(&condition, 0, Some(at), Token(2))
                .expect("wait");
            assert_eq!(l.next_deadline(), Some(at));
            let (mut turns, mut empty, mut waits) = (0, 0, 0);
            loop {
                let info = l
                    .turn(Timeout::After(Duration::from_secs(1)), &mut out)
                    .expect("wait turn");
                turns += 1;
                empty += info.zero_event_waits;
                waits += info.os_waits + info.discovery_polls;
                assert!(
                    turns <= 2 && empty <= 1,
                    "external deadline spun: micros={micros}, expiry={expiries}, turns={turns}, empty={empty}, remaining={:?}",
                    at.saturating_duration_since(l.now())
                );
                if !out.is_empty() {
                    assert_eq!(out.len(), 1);
                    assert_eq!(out[0].op, Some(op));
                    assert!(matches!(
                        out[0].result,
                        OpResult::ExternalWait(WaitResult::TimedOut)
                    ));
                    assert!(l.now() >= at);
                    assert!(l.now() - at < Duration::from_millis(100));
                    assert!(waits > 0, "wait executed");
                    expiries += 1;
                    break;
                }
            }
        }
    }
    assert_eq!(expiries, 60);
    assert!(l.cancel(idle));
}

#[cfg(feature = "executor")]
#[test]
fn executor_on_one_wasi_agent() {
    use contract::executor_contract as executor;
    executor::sleep_timeout_and_drop_cancel_pending_io::<Platform>();
    executor::pending_future_buffers_may_move_and_shrink::<Platform>();
    executor::pending_writes_may_replace_the_caller_slice::<Platform>();
}

#[test]
fn stdio_streams_preserve_host_ownership() {
    use std::time::Duration;
    use turnloop::*;
    let mut l = Loop::new(Config::default()).expect("loop");
    let stdin = l.open_stdio(Stdio::Stdin).expect("stdin");
    let stdout = l.open_stdio(Stdio::Stdout).expect("stdout");
    let stderr = l.open_stdio(Stdio::Stderr).expect("stderr");
    let expected = b"turnloop revision two stdin fixture\n";
    let mut actual = Vec::new();
    let mut out = Completions::default();
    let until = l.now() + Duration::from_secs(5);
    while actual.len() < expected.len() {
        l.read(stdin, ReadBuf::Pooled, Token(1))
            .expect("stdin read");
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("read turn");
            if !out.is_empty() {
                assert_eq!(out.len(), 1);
                let OpResult::Read {
                    n,
                    lease: Some(ref bytes),
                } = out[0].result
                else {
                    panic!("stdin fixture bytes missing")
                };
                assert!(n > 0);
                actual.extend_from_slice(bytes.as_slice());
                break;
            }
        }
    }
    assert_eq!(actual, expected);
    for (h, bytes) in [
        (stdout, b"turnloop stdio stdout verified\n".as_slice()),
        (stderr, b"turnloop stdio stderr verified\n".as_slice()),
    ] {
        l.write(h, WriteBuf::Owned(bytes.to_vec()), Token(2))
            .expect("stdio write");
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out).expect("write turn");
            if !out.is_empty() {
                assert!(matches!(out[0].result, OpResult::Wrote(n) if n == bytes.len()));
                break;
            }
        }
        l.shutdown(h, Token(3)).expect("stdio shutdown");
        loop {
            assert!(l.now() < until);
            l.turn(Timeout::Until(until), &mut out)
                .expect("shutdown turn");
            if !out.is_empty() {
                assert!(matches!(out[0].result, OpResult::Shutdown));
                break;
            }
        }
    }
    for h in [stdin, stdout, stderr] {
        l.close(h, Token(4)).expect("close stdio");
    }
    l.turn(Timeout::Now, &mut out).expect("close turn");
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|c| matches!(c.result, OpResult::Closed)));
    assert!(!l.alive());
    // libtest still prints via the host stdout after the owned streams close.
    println!(
        "stdio subject: {} stdin bytes, two writes, three Closed",
        actual.len()
    );
}

/// Filesystem contracts over the `/turnloop-fs` preopen (scripts/ci/wasmtime-runner.sh).
mod filesystem {
    use std::path::Path;
    use turnloop::backend::Platform;
    use turnloop_contract::filesystem as contract;
    fn root() -> &'static Path {
        let root = Path::new("/turnloop-fs");
        assert!(root.is_dir(), "the runner must preopen /turnloop-fs");
        root
    }
    #[test]
    fn bytes_metadata_namespace() {
        contract::bytes_metadata_namespace::<Platform>(root());
    }
    #[test]
    fn errors() {
        contract::errors::<Platform>(root());
    }
    #[test]
    fn fifo_cancel_close() {
        contract::fifo_cancel_close::<Platform>(root());
    }
    #[test]
    fn pooled_lease_wait() {
        contract::pooled_lease_wait::<Platform>(root());
    }
    #[test]
    fn queued_posts_do_not_starve_requests() {
        contract::queued_posts_do_not_starve_requests::<Platform>(root());
    }
    #[test]
    fn capability_scope_and_unsupported_surface() {
        contract::capability_scope::<Platform>(root());
    }
}
