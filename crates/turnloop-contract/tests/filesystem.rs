#![cfg(all(not(loom), any(unix, windows)))]
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use turnloop::*;
fn config() -> Config {
    Config {
        max_handles: 16,
        max_operations: 32,
        events_per_turn: 8,
        ..Config::default()
    }
}
fn next(l: &mut Loop, out: &mut Completions) -> Completion {
    let until = l.now() + Duration::from_secs(5);
    loop {
        assert!(l.now() < until, "filesystem completion deadline");
        let info = l.turn(Timeout::Until(until), out).expect("turn");
        assert!(info.os_waits <= 1);
        if let Some(c) = out.drain().next() {
            return c;
        }
    }
}
fn done(l: &mut Loop, out: &mut Completions, op: OpId) -> FsResult {
    let c = next(l, out);
    assert_eq!(c.op, Some(op));
    assert!(c.terminal);
    match c.result {
        OpResult::Fs(r) => r,
        r => panic!("unexpected {r:?}"),
    }
}
fn path(name: &str) -> FsPath {
    FsPath::new(std::env::temp_dir().join(format!("turnloop-i06-{}-{name}", std::process::id())))
        .expect("path")
}
fn write(l: &mut Loop, h: Handle, bytes: &'static [u8], offset: Option<u64>) -> OpId {
    // SAFETY: immutable static bytes outlive all operations.
    let buffer = WriteBuf::Provided(unsafe { IoBuf::from_raw_parts(bytes.as_ptr(), bytes.len()) });
    l.fs(
        FsRequest::Write {
            file: h,
            buffer,
            offset,
        },
        Token(2),
    )
    .expect("write")
}
#[test]
fn bytes_metadata_cursor_namespace_and_directory_pages() {
    let mut l = Loop::new(config()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let dir = path("directory");
    let p = FsPath::new(dir.as_path().join("file")).expect("path");
    let q = FsPath::new(dir.as_path().join("renamed")).expect("path");
    let op = l
        .fs(
            FsRequest::Mkdir {
                path: dir.clone(),
                mode: 0o700,
            },
            Token(1),
        )
        .expect("mkdir");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Done));
    let (h, op) = l
        .file_open(
            p.clone(),
            FileOptions {
                write: true,
                create: true,
                exclusive: true,
                ..FileOptions::default()
            },
            Token(1),
        )
        .expect("open");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Opened));
    let op = write(&mut l, h, b"abc", None);
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Wrote(3)));
    let op = write(&mut l, h, b"Z", Some(1));
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Wrote(1)));
    let op = write(&mut l, h, b"def", None);
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Wrote(3)));
    let mut bytes = [0; 6];
    // SAFETY: bytes stays live and is inspected only after its terminal completion.
    let buffer = unsafe { IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len()) };
    let op = l
        .fs(
            FsRequest::Read {
                file: h,
                buffer,
                offset: Some(0),
            },
            Token(3),
        )
        .expect("read");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Read(6)));
    assert_eq!(&bytes, b"aZcdef");
    for data_only in [true, false] {
        let op = l
            .fs(FsRequest::Sync { file: h, data_only }, Token(4))
            .expect("sync");
        assert!(matches!(done(&mut l, &mut out, op), FsResult::Done));
    }
    let op = l.fs(FsRequest::Fstat(h), Token(5)).expect("fstat");
    let FsResult::Metadata(m) = done(&mut l, &mut out, op) else {
        panic!("metadata")
    };
    assert_eq!(m.size, 6);
    assert_eq!(m.kind, FileType::File);
    assert!(m.modified.is_some());
    assert!(m.inode.is_some());
    let op = l
        .fs(FsRequest::Truncate { file: h, size: 2 }, Token(6))
        .expect("truncate");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Done));
    let op = l
        .fs(
            FsRequest::Stat {
                path: p.clone(),
                follow_symlinks: true,
            },
            Token(7),
        )
        .expect("stat");
    let FsResult::Metadata(m) = done(&mut l, &mut out, op) else {
        panic!("stat")
    };
    assert_eq!(m.size, 2);
    let mut listing = [0; 64];
    // SAFETY: listing is retained exclusively until directory completion.
    let buffer = unsafe { IoBufMut::from_raw_parts(listing.as_mut_ptr(), listing.len()) };
    let op = l
        .fs(
            FsRequest::ReadDir {
                path: dir.clone(),
                buffer,
                cookie: 0,
            },
            Token(8),
        )
        .expect("readdir");
    let FsResult::Directory { bytes, cookie, eof } = done(&mut l, &mut out, op) else {
        panic!("directory")
    };
    assert_eq!(cookie, 1);
    assert!(eof);
    assert_eq!(&listing[..bytes], b"\x04\0\x01file");
    l.close(h, Token(9)).expect("close");
    assert!(matches!(next(&mut l, &mut out).result, OpResult::Closed));
    let op = l
        .fs(
            FsRequest::Rename {
                from: p,
                to: q.clone(),
            },
            Token(10),
        )
        .expect("rename");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Done));
    assert_eq!(std::fs::read(q.as_path()).expect("actual bytes"), b"aZ");
    let op = l.fs(FsRequest::Unlink(q), Token(11)).expect("unlink");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Done));
    let op = l.fs(FsRequest::Rmdir(dir), Token(12)).expect("rmdir");
    assert!(matches!(done(&mut l, &mut out, op), FsResult::Done));
    assert!(!l.alive());
}
#[test]
fn fifo_close_cancellation_and_no_access_after_terminal() {
    let mut l = Loop::new(config()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let p = path("cancel");
    let (h, op) = l
        .file_open(
            p.clone(),
            FileOptions {
                write: true,
                create: true,
                truncate: true,
                ..FileOptions::default()
            },
            Token(1),
        )
        .expect("open");
    done(&mut l, &mut out, op);
    // Hold every shared pool thread, proving the read never starts before cancellation.
    let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let released = Arc::new(AtomicBool::new(false));
    let mut jobs = Vec::new();
    for _ in 0..4 {
        let e = entered.clone();
        let r = released.clone();
        jobs.push(
            l.blocking(
                move || {
                    e.fetch_add(1, Ordering::Release);
                    while !r.load(Ordering::Acquire) {
                        std::thread::park_timeout(Duration::from_millis(1));
                    }
                    Ok(Payload::U64(1))
                },
                Token(0),
            )
            .expect("pool job"),
        );
    }
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let guard = Release(released.clone());
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while entered.load(Ordering::Acquire) != 4 {
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    }
    let first = write(&mut l, h, b"unwritten", None);
    let mut bytes = [0x5a; 32];
    // SAFETY: bytes is retained untouched through the cancellation and loop drop.
    let buffer = unsafe { IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len()) };
    let second = l
        .fs(
            FsRequest::Read {
                file: h,
                buffer,
                offset: None,
            },
            Token(3),
        )
        .expect("read");
    l.close(h, Token(4)).expect("close");
    assert!(l.fs(FsRequest::Fstat(h), Token(5)).is_err());
    drop(guard);
    let mut cancellations = Vec::new();
    let mut closed = false;
    let mut blocks = 0;
    while !closed || blocks < 4 {
        let c = next(&mut l, &mut out);
        match c.result {
            OpResult::Cancelled => cancellations.push(c.op.expect("op")),
            OpResult::Closed => {
                assert_eq!(cancellations, [first, second]);
                closed = true;
            }
            OpResult::Blocking(Payload::U64(1)) => blocks += 1,
            r => panic!("{r:?}"),
        }
    }
    assert_eq!(bytes, [0x5a; 32]);
    bytes.fill(0xa5);
    drop(l);
    assert_eq!(bytes, [0xa5; 32]);
    assert_eq!(std::fs::read(p.as_path()).expect("file"), b"");
    std::fs::remove_file(p.as_path()).expect("cleanup");
}
#[cfg(unix)]
#[test]
fn permission_errors_and_lstat_are_real() {
    use std::os::unix::fs::PermissionsExt;
    let p = path("permission");
    let link = path("symlink");
    std::fs::write(p.as_path(), b"secret").expect("fixture");
    std::os::unix::fs::symlink(p.as_path(), link.as_path()).expect("symlink");
    let mut l = Loop::new(config()).expect("loop");
    let mut out = Completions::with_capacity(1);
    let op = l
        .fs(
            FsRequest::Stat {
                path: link.clone(),
                follow_symlinks: false,
            },
            Token(1),
        )
        .expect("lstat");
    let FsResult::Metadata(m) = done(&mut l, &mut out, op) else {
        panic!("metadata")
    };
    assert_eq!(m.kind, FileType::Symlink);
    std::fs::set_permissions(p.as_path(), std::fs::Permissions::from_mode(0)).expect("permissions");
    let (h, op) = l
        .file_open(p.clone(), FileOptions::default(), Token(2))
        .expect("open accepted");
    let c = next(&mut l, &mut out);
    assert_eq!(c.op, Some(op));
    // This suite requires an unprivileged runner; root must not count this as a permission test pass.
    assert!(
        matches!(
            c.result,
            OpResult::Err(Error {
                kind: ErrorKind::PermissionDenied,
                os: Some(_)
            })
        ),
        "{c:?}"
    );
    l.close(h, Token(3)).expect("close");
    assert!(matches!(next(&mut l, &mut out).result, OpResult::Closed));
    std::fs::set_permissions(p.as_path(), std::fs::Permissions::from_mode(0o600)).expect("restore");
    std::fs::remove_file(p.as_path()).expect("cleanup");
    std::fs::remove_file(link.as_path()).expect("cleanup link");
}
