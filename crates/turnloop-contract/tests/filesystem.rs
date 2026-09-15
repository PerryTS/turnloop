//! Native typed filesystem requests and watches (DESIGN D8, §7.6 Files).
#![cfg(all(not(loom), any(unix, windows)))]
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use turnloop::{backend::Platform, *};
use turnloop_contract::filesystem as contract;

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turnloop-fs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("test root");
    dir
}

#[test]
fn bytes_metadata_namespace() {
    contract::bytes_metadata_namespace::<Platform>(&root("namespace"));
}
#[test]
fn errors() {
    contract::errors::<Platform>(&root("errors"));
}
#[test]
fn fifo_cancel_close() {
    contract::fifo_cancel_close::<Platform>(&root("cancel"));
}
#[test]
fn pooled_lease_wait() {
    contract::pooled_lease_wait::<Platform>(&root("leases"));
}
#[test]
fn watch_directory() {
    contract::watch_directory::<Platform>(&root("watch-dir"));
}
#[test]
fn watch_file() {
    contract::watch_file::<Platform>(&root("watch-file"));
}
#[test]
fn watch_backpressure() {
    contract::watch_backpressure::<Platform>(&root("watch-pressure"));
}
#[test]
fn watch_recursive() {
    contract::watch_recursive::<Platform>(&root("watch-tree"));
}

/// Hold every shared pool thread so queued requests provably have not started.
struct Hold {
    released: Arc<AtomicBool>,
    jobs: Vec<OpId>,
}
impl Hold {
    fn new(l: &mut Loop) -> Self {
        let entered = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let threads = Config::default().blocking_pool.threads;
        let jobs = (0..threads)
            .map(|_| {
                let (entered, released) = (entered.clone(), released.clone());
                l.blocking(
                    move || {
                        entered.fetch_add(1, Ordering::Release);
                        while !released.load(Ordering::Acquire) {
                            std::thread::park_timeout(Duration::from_millis(1));
                        }
                        Ok(Payload::U64(1))
                    },
                    Token(1000),
                )
                .expect("pool job")
            })
            .collect();
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while entered.load(Ordering::Acquire) != threads {
            assert!(std::time::Instant::now() < until, "pool threads started");
            std::thread::yield_now();
        }
        Self { released, jobs }
    }
}
impl Drop for Hold {
    fn drop(&mut self) {
        self.released.store(true, Ordering::Release);
    }
}

/// Queue exhaustion rejects before acceptance; every accepted request completes once.
#[test]
fn pool_backpressure_rejects_before_acceptance() {
    let dir = root("backpressure");
    let mut l = Loop::new(Config::default()).expect("loop");
    let hold = Hold::new(&mut l);
    let target = FsPath::new(&dir).expect("path");
    let capacity = Config::default().blocking_pool.queue_capacity;
    let mut accepted = Vec::new();
    let rejected = loop {
        match l.fs(
            FsRequest::Stat {
                path: target.clone(),
                follow_symlinks: true,
            },
            Token(accepted.len() as u64),
        ) {
            Ok(op) => accepted.push(op),
            Err(e) => break e,
        }
        assert!(accepted.len() <= capacity, "queue bound enforced");
    };
    assert_eq!(rejected.kind, ErrorKind::ResourceLimit);
    assert_eq!(
        accepted.len(),
        capacity,
        "every reserved queue slot was usable"
    );
    assert!(
        l.blocking(|| Ok(Payload::U64(2)), Token(2000)).is_err(),
        "reservations also bound host jobs"
    );
    let jobs = hold.jobs.clone();
    drop(hold);
    let mut out = Completions::default();
    let mut seen = vec![false; capacity];
    let mut blocking = 0;
    let until = l.now() + Duration::from_secs(30);
    while seen.iter().any(|s| !s) || blocking < jobs.len() {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Fs(FsResult::Metadata(m)) => {
                    assert_eq!(m.kind, FileType::Directory);
                    let i = c.token.0 as usize;
                    assert_eq!(c.op, Some(accepted[i]));
                    assert!(!seen[i], "exactly once");
                    seen[i] = true;
                }
                OpResult::Blocking(Payload::U64(1)) => blocking += 1,
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    assert!(!l.alive());
    // Capacity is available again.
    let op = l
        .fs(
            FsRequest::Stat {
                path: target,
                follow_symlinks: true,
            },
            Token(1),
        )
        .expect("released reservations");
    assert!(contract::wait(&mut l, op).is_ok());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// Cancelling an unstarted request completes it at once, releases its queue
/// reservation and starts its FIFO successor; the started head reports Cancelled.
#[test]
fn cancellation_withdraws_unstarted_and_reports_started() {
    let dir = root("withdraw");
    let file = FsPath::new(dir.join("file")).expect("path");
    std::fs::write(file.as_path(), b"0123456789").expect("fixture");
    let mut l = Loop::new(Config::default()).expect("loop");
    let op = l
        .fs(
            FsRequest::Open {
                path: file.clone(),
                options: FileOptions::default(),
            },
            Token(1),
        )
        .expect("open");
    let Ok(FsResult::Opened(h)) = contract::wait(&mut l, op) else {
        panic!("open")
    };
    let hold = Hold::new(&mut l);
    let mut head_bytes = [0u8; 4];
    let mut middle_bytes = [0xee; 4];
    let mut tail_bytes = [0u8; 4];
    let read = |bytes: &mut [u8], offset| FsRequest::Read {
        file: h,
        // SAFETY: each array outlives its request and is read only after completion.
        buffer: ReadBuf::Provided(unsafe {
            IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len())
        }),
        offset: Some(offset),
    };
    let head = l.fs(read(&mut head_bytes, 0), Token(2)).expect("head");
    let middle = l.fs(read(&mut middle_bytes, 4), Token(3)).expect("middle");
    let tail = l.fs(read(&mut tail_bytes, 6), Token(4)).expect("tail");
    assert!(l.cancel(middle), "unstarted request cancels");
    assert!(!l.cancel(middle), "cancellation is idempotent");
    assert!(l.cancel(head), "queued head cancels");
    let mut out = Completions::default();
    l.turn(Timeout::Now, &mut out).expect("withdrawn results");
    let mut results: Vec<_> = out.drain().map(|c| (c.op, c.result)).collect();
    // The head was handed to the (held) pool; only the middle request is withdrawn.
    assert_eq!(results.len(), 1, "{results:?}");
    let (op, result) = results.pop().expect("middle");
    assert_eq!(op, Some(middle));
    assert!(matches!(result, OpResult::Cancelled));
    drop(hold);
    let until = l.now() + Duration::from_secs(10);
    let mut order = Vec::new();
    while order.len() < 6 {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                OpResult::Blocking(_) => order.push(None),
                OpResult::Cancelled => order.push(Some(c.op.expect("head"))),
                OpResult::Fs(FsResult::Read { n: 4, .. }) => {
                    assert_eq!(c.op, Some(tail));
                    order.push(c.op);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    let requests: Vec<_> = order.into_iter().flatten().collect();
    assert_eq!(
        requests,
        [head, tail],
        "FIFO successor runs after the cancelled head"
    );
    assert_eq!(
        middle_bytes, [0xee; 4],
        "withdrawn request never touched its buffer"
    );
    assert_eq!(&tail_bytes, b"6789");
    l.close(h, Token(5)).expect("close");
    l.turn(Timeout::Now, &mut out).expect("closed");
    assert!(matches!(out[0].result, OpResult::Closed));
    assert!(!l.alive());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// Permission errors are real OS denials (requires an unprivileged runner).
#[test]
fn permission_denied_is_reported() {
    let dir = root("permission");
    let file = dir.join("locked");
    std::fs::write(&file, b"secret").expect("fixture");
    let mut l = Loop::new(contract::config()).expect("loop");
    let locked = FsPath::new(&file).expect("path");
    let r = contract::run(
        &mut l,
        FsRequest::Chmod {
            target: FsTarget::Path {
                path: locked.clone(),
                follow_symlinks: true,
            },
            mode: 0o444,
        },
    );
    assert!(matches!(r, Ok(FsResult::Done)), "{r:?}");
    let e = contract::run(
        &mut l,
        FsRequest::Open {
            path: locked.clone(),
            options: FileOptions {
                read: false,
                write: true,
                ..FileOptions::default()
            },
        },
    )
    .expect_err("read-only file rejects writers");
    assert_eq!(e.kind, ErrorKind::PermissionDenied, "{e:?}");
    assert!(e.os.is_some());
    let e = contract::run(
        &mut l,
        FsRequest::Access {
            path: locked.clone(),
            mode: AccessMode {
                write: true,
                ..AccessMode::default()
            },
        },
    )
    .expect_err("access reports the denial");
    assert_eq!(e.kind, ErrorKind::PermissionDenied, "{e:?}");
    let r = contract::run(
        &mut l,
        FsRequest::Chmod {
            target: FsTarget::Path {
                path: locked,
                follow_symlinks: true,
            },
            mode: 0o644,
        },
    );
    assert!(matches!(r, Ok(FsResult::Done)), "{r:?}");
    assert!(!l.alive());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// A cancelled open exposes no handle, and dropping a loop withdraws requests
/// still queued on the shared pool: their buffers are never accessed afterwards.
#[test]
fn cancelled_open_and_loop_drop_never_touch_buffers() {
    let dir = root("drop");
    let file = FsPath::new(dir.join("file")).expect("path");
    std::fs::write(file.as_path(), b"0123456789").expect("fixture");
    let mut l = Loop::new(Config::default()).expect("loop");
    let op = l
        .fs(
            FsRequest::Open {
                path: file.clone(),
                options: FileOptions::default(),
            },
            Token(1),
        )
        .expect("open");
    let Ok(FsResult::Opened(h)) = contract::wait(&mut l, op) else {
        panic!("open")
    };
    let hold = Hold::new(&mut l);
    let hidden = l
        .fs(
            FsRequest::Open {
                path: file.clone(),
                options: FileOptions::default(),
            },
            Token(2),
        )
        .expect("queued open");
    assert!(l.cancel(hidden));
    let mut head = [0x11u8; 8];
    let mut queued = [0x22u8; 8];
    let read = |bytes: &mut [u8]| FsRequest::Read {
        file: h,
        // SAFETY: both arrays outlive the loop, which is dropped below.
        buffer: ReadBuf::Provided(unsafe {
            IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len())
        }),
        offset: Some(0),
    };
    l.fs(read(&mut head), Token(3)).expect("head read");
    l.fs(read(&mut queued), Token(4)).expect("queued read");
    drop(l);
    head.fill(0x33);
    queued.fill(0x44);
    let jobs = hold.jobs.len();
    drop(hold);
    // Give the released pool threads time to dequeue the withdrawn job.
    let mut probe = Loop::new(Config::default()).expect("probe loop");
    let mut out = Completions::default();
    let op = probe
        .fs(
            FsRequest::Stat {
                path: file.clone(),
                follow_symlinks: true,
            },
            Token(5),
        )
        .expect("probe request queues behind the withdrawn job");
    let until = probe.now() + Duration::from_secs(10);
    let mut done = false;
    while !done {
        assert!(probe.now() < until);
        probe.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert_eq!(c.op, Some(op));
            assert!(matches!(c.result, OpResult::Fs(FsResult::Metadata(_))));
            done = true;
        }
    }
    assert_eq!(jobs, Config::default().blocking_pool.threads);
    assert_eq!(head, [0x33; 8], "withdrawn head never ran after drop");
    assert_eq!(queued, [0x44; 8], "queued successor never ran after drop");
    drop(probe);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
