#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd"
    )
))]
use turnloop::*;
fn fd_count() -> usize {
    let path = if cfg!(any(target_os = "linux", target_os = "android")) {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    std::fs::read_dir(path)
        .expect("fd directory")
        .inspect(|entry| assert!(entry.is_ok(), "fd entry"))
        .count()
}
#[test]
fn loop_drop_and_stale_wakers_release_all_descriptors() {
    let baseline = fd_count();
    let mut connections = 0;
    for _ in 0..32 {
        let mut l = Loop::new(Config::default()).expect("loop");
        let (_, _, _) = turnloop_contract::pair(&mut l);
        connections += 1;
        let notifier = l.notifier();
        let poster = l.poster();
        drop(l);
        assert!(matches!(
            notifier.notify(),
            Err(Error {
                kind: ErrorKind::NotFound,
                ..
            })
        ));
        let error = poster
            .post(Token(1), Payload::U64(3))
            .expect_err("closed poster");
        assert_eq!(error.error.kind, ErrorKind::NotFound);
        assert!(matches!(error.payload, Some(Payload::U64(3))));
        drop(notifier);
        drop(poster);
    }
    assert_eq!(connections, 32);
    assert_eq!(fd_count(), baseline, "all native fds must be released");
}
