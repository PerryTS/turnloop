//! Typed filesystem contract scenarios, shared by native and WASI backends.
//! Each scenario creates its own directory below `root` and checks real bytes on
//! disk with `std::fs` (which WASI resolves through the same preopens).
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use turnloop::{backend::Backend, *};

pub fn config() -> Config {
    Config {
        max_handles: 32,
        max_operations: 64,
        events_per_turn: 16,
        ..Config::default()
    }
}
fn path(p: impl AsRef<Path>) -> FsPath {
    FsPath::new(p).expect("prepared path")
}
/// Wait for the terminal completion of `op`; no other completion may arrive.
pub fn wait<B: Backend>(l: &mut Driver<B>, op: OpId) -> std::result::Result<FsResult, Error> {
    let mut out = Completions::with_capacity(4);
    let until = l.now() + Duration::from_secs(10);
    loop {
        assert!(l.now() < until, "filesystem completion deadline");
        let info = l.turn(Timeout::Until(until), &mut out).expect("turn");
        assert!(
            info.os_waits + info.discovery_polls <= 1,
            "one native call per turn"
        );
        let mut drained = out.drain();
        let Some(c) = drained.next() else {
            continue;
        };
        assert!(drained.next().is_none(), "one completion per request");
        assert_eq!(c.op, Some(op));
        assert!(c.terminal);
        return match c.result {
            OpResult::Fs(r) => Ok(r),
            OpResult::Err(e) => Err(e),
            other => panic!("unexpected {other:?}"),
        };
    }
}
pub fn run<B: Backend>(
    l: &mut Driver<B>,
    request: FsRequest,
) -> std::result::Result<FsResult, Error> {
    let op = l.fs(request, Token(7)).expect("accepted request");
    wait(l, op)
}
fn done<B: Backend>(l: &mut Driver<B>, request: FsRequest) {
    let r = run(l, request).expect("request succeeds");
    assert!(matches!(r, FsResult::Done), "{r:?}");
}
fn kind<B: Backend>(l: &mut Driver<B>, request: FsRequest) -> ErrorKind {
    let e = run(l, request).expect_err("request fails");
    assert!(e.os.is_some(), "native error code retained: {e:?}");
    e.kind
}
fn stat<B: Backend>(l: &mut Driver<B>, p: &Path, follow_symlinks: bool) -> FileMetadata {
    match run(
        l,
        FsRequest::Stat {
            path: path(p),
            follow_symlinks,
        },
    ) {
        Ok(FsResult::Metadata(m)) => *m,
        other => panic!("stat {p:?}: {other:?}"),
    }
}
fn open<B: Backend>(l: &mut Driver<B>, p: &Path, options: FileOptions) -> Handle {
    match run(
        l,
        FsRequest::Open {
            path: path(p),
            options,
        },
    ) {
        Ok(FsResult::Opened(h)) => h,
        other => panic!("open {p:?}: {other:?}"),
    }
}
fn close<B: Backend>(l: &mut Driver<B>, h: Handle) {
    l.close(h, Token(99)).expect("close handle");
    let mut out = Completions::with_capacity(1);
    let until = l.now() + Duration::from_secs(5);
    loop {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        if let Some(c) = out.drain().next() {
            assert_eq!(c.handle, Some(h));
            assert!(matches!(c.result, OpResult::Closed), "{c:?}");
            return;
        }
    }
}
fn write_buf(bytes: &'static [u8]) -> WriteBuf {
    // SAFETY: static immutable bytes outlive every operation.
    WriteBuf::Provided(unsafe { IoBuf::from_raw_parts(bytes.as_ptr(), bytes.len()) })
}
fn provided(bytes: &mut [u8]) -> ReadBuf {
    // SAFETY: callers keep the region in place and read it only after the terminal completion.
    ReadBuf::Provided(unsafe { IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len()) })
}
fn fresh(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
fn read_names<B: Backend>(l: &mut Driver<B>, dir: &Path, page: usize) -> Vec<(FileType, String)> {
    let d = match run(l, FsRequest::OpenDir { path: path(dir) }) {
        Ok(FsResult::Opened(h)) => h,
        other => panic!("opendir: {other:?}"),
    };
    let mut names = Vec::new();
    let mut storage = vec![0u8; page];
    let mut pooled = false;
    loop {
        let buffer = if pooled {
            ReadBuf::Pooled
        } else {
            provided(&mut storage)
        };
        let (n, lease, eof) = match run(l, FsRequest::ReadDir { dir: d, buffer }) {
            Ok(FsResult::Directory { n, lease, eof }) => (n, lease, eof),
            other => panic!("readdir: {other:?}"),
        };
        let bytes = match &lease {
            Some(lease) => lease.as_slice(),
            None => &storage[..n],
        };
        assert_eq!(bytes.len(), n);
        for (t, name) in DirEntries::new(bytes) {
            names.push((
                t,
                String::from_utf8(name.to_vec()).expect("test names are UTF-8"),
            ));
        }
        if eof {
            break;
        }
        assert!(n > 0, "a non-final page carries at least one entry");
        pooled = !pooled;
    }
    close(l, d);
    names.sort_by(|a, b| a.1.cmp(&b.1));
    names
}

/// Real bytes, cursor and positional I/O, metadata, links, copies and directory pages.
pub fn bytes_metadata_namespace<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "namespace");
    done(
        &mut l,
        FsRequest::Mkdir {
            path: path(&dir),
            mode: 0o755,
        },
    );
    let file = dir.join("file");
    let create = FileOptions {
        read: true,
        write: true,
        create: true,
        exclusive: true,
        ..FileOptions::default()
    };
    let h = open(&mut l, &file, create);
    assert_eq!(
        kind(
            &mut l,
            FsRequest::Open {
                path: path(&file),
                options: create
            }
        ),
        ErrorKind::AlreadyExists
    );
    for (bytes, offset, n) in [
        (&b"abc"[..], None, 3),
        (&b"Z"[..], Some(1), 1),
        (&b"def"[..], None, 3),
    ] {
        let r = run(
            &mut l,
            FsRequest::Write {
                file: h,
                buffer: write_buf(bytes),
                offset,
            },
        );
        assert!(matches!(r, Ok(FsResult::Wrote(w)) if w == n), "{r:?}");
    }
    let mut bytes = [0u8; 8];
    let r = run(
        &mut l,
        FsRequest::Read {
            file: h,
            buffer: provided(&mut bytes),
            offset: Some(0),
        },
    );
    assert!(
        matches!(r, Ok(FsResult::Read { n: 6, lease: None })),
        "{r:?}"
    );
    assert_eq!(&bytes[..6], b"aZcdef");
    let r = run(
        &mut l,
        FsRequest::Read {
            file: h,
            buffer: ReadBuf::Pooled,
            offset: Some(2),
        },
    );
    let Ok(FsResult::Read {
        n: 4,
        lease: Some(lease),
    }) = r
    else {
        panic!("pooled positional read: {r:?}")
    };
    assert_eq!(lease.as_slice(), b"cdef");
    drop(lease);
    let r = run(
        &mut l,
        FsRequest::Read {
            file: h,
            buffer: provided(&mut bytes),
            offset: None,
        },
    );
    assert!(
        matches!(r, Ok(FsResult::Read { n: 0, .. })),
        "cursor is at end of file: {r:?}"
    );
    for data_only in [true, false] {
        done(&mut l, FsRequest::Sync { file: h, data_only });
    }
    let Ok(FsResult::Metadata(m)) = run(&mut l, FsRequest::Fstat { file: h }) else {
        panic!("fstat")
    };
    assert_eq!((m.kind, m.size), (FileType::File, 6));
    assert!(m.modified.is_some());
    #[cfg(not(target_os = "wasi"))]
    assert!(m.inode.is_some() && m.device.is_some() && m.links == Some(1));
    #[cfg(unix)]
    assert_eq!(
        m.mode.map(|mode| mode & 0o170000),
        Some(0o100000),
        "regular type bits"
    );
    done(&mut l, FsRequest::Truncate { file: h, size: 2 });
    assert_eq!(stat(&mut l, &file, true).size, 2);
    let at = FileTime {
        seconds: 1_000_000_000,
        nanoseconds: 123_456_700,
    };
    done(
        &mut l,
        FsRequest::SetTimes {
            target: FsTarget::File(h),
            accessed: TimeChange::Keep,
            modified: TimeChange::At(at),
        },
    );
    assert_eq!(stat(&mut l, &file, true).modified, Some(at));
    done(&mut l, FsRequest::Close { file: h });
    assert!(
        run(&mut l, FsRequest::Fstat { file: h }).is_err(),
        "closed descriptor rejects later requests"
    );
    close(&mut l, h);
    assert!(
        l.fs(FsRequest::Fstat { file: h }, Token(1)).is_err(),
        "released handle is rejected before acceptance"
    );
    assert_eq!(std::fs::read(&file).expect("real bytes"), b"aZ");

    let hard = dir.join("hard");
    done(
        &mut l,
        FsRequest::Link {
            existing: path(&file),
            link: path(&hard),
        },
    );
    assert_eq!(stat(&mut l, &file, true).links, Some(2));
    let link = dir.join("link");
    done(
        &mut l,
        FsRequest::Symlink {
            target: path("file"),
            link: path(&link),
            kind: SymlinkKind::File,
        },
    );
    assert_eq!(stat(&mut l, &link, false).kind, FileType::Symlink);
    let through = stat(&mut l, &link, true);
    assert_eq!((through.kind, through.size), (FileType::File, 2));
    let r = run(
        &mut l,
        FsRequest::ReadLink {
            path: path(&link),
            buffer: ReadBuf::Pooled,
        },
    );
    let Ok(FsResult::Bytes {
        n: 4,
        lease: Some(content),
    }) = r
    else {
        panic!("readlink: {r:?}")
    };
    assert_eq!(content.as_slice(), b"file");
    drop(content);
    #[cfg(not(target_os = "wasi"))]
    {
        let mut canonical = [0u8; 4096];
        let r = run(
            &mut l,
            FsRequest::RealPath {
                path: path(&link),
                buffer: provided(&mut canonical),
            },
        );
        let Ok(FsResult::Bytes { n, lease: None }) = r else {
            panic!("realpath: {r:?}")
        };
        let expected = std::fs::canonicalize(&file).expect("std canonical path");
        // std keeps the Windows verbatim prefix; RealPath reports the DOS form.
        #[cfg(windows)]
        let expected = PathBuf::from(
            expected
                .to_str()
                .expect("UTF-8 path")
                .strip_prefix(r"\\?\")
                .expect("verbatim canonical path"),
        );
        assert_eq!(
            Path::new(std::str::from_utf8(&canonical[..n]).expect("UTF-8 path")),
            expected
        );
    }
    done(
        &mut l,
        FsRequest::Access {
            path: path(&file),
            mode: AccessMode::default(),
        },
    );
    let copy = dir.join("copy");
    done(
        &mut l,
        FsRequest::CopyFile {
            from: path(&file),
            to: path(&copy),
            exclusive: true,
        },
    );
    assert_eq!(std::fs::read(&copy).expect("copied bytes"), b"aZ");
    assert_eq!(
        kind(
            &mut l,
            FsRequest::CopyFile {
                from: path(&file),
                to: path(&copy),
                exclusive: true
            }
        ),
        ErrorKind::AlreadyExists
    );
    let moved = dir.join("moved");
    done(
        &mut l,
        FsRequest::Rename {
            from: path(&copy),
            to: path(&moved),
        },
    );
    assert_eq!(
        kind(
            &mut l,
            FsRequest::Stat {
                path: path(&copy),
                follow_symlinks: true
            }
        ),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(&mut l, FsRequest::Rmdir { path: path(&dir) }),
        ErrorKind::DirectoryNotEmpty
    );
    let expected = [
        (FileType::File, "file"),
        (FileType::File, "hard"),
        (FileType::Symlink, "link"),
        (FileType::File, "moved"),
    ];
    for page in [9, 16, 4096] {
        let names = read_names(&mut l, &dir, page);
        let names: Vec<_> = names.iter().map(|(t, n)| (*t, n.as_str())).collect();
        assert_eq!(names, expected, "page size {page}");
    }
    for name in ["file", "hard", "link", "moved"] {
        done(
            &mut l,
            FsRequest::Unlink {
                path: path(dir.join(name)),
            },
        );
    }
    done(&mut l, FsRequest::Rmdir { path: path(&dir) });
    assert!(!dir.exists());
    assert!(!l.alive());
}

/// Portable error kinds with native codes, and a page too small for any entry.
pub fn errors<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "errors");
    std::fs::create_dir(&dir).expect("fixture");
    std::fs::write(dir.join("plain"), b"x").expect("fixture");
    let missing = dir.join("missing");
    assert_eq!(
        kind(
            &mut l,
            FsRequest::Open {
                path: path(&missing),
                options: FileOptions::default()
            }
        ),
        ErrorKind::NotFound
    );
    assert_eq!(
        kind(
            &mut l,
            FsRequest::Mkdir {
                path: path(&dir),
                mode: 0o755
            }
        ),
        ErrorKind::AlreadyExists
    );
    assert_eq!(
        kind(&mut l, FsRequest::Rmdir { path: path(&dir) }),
        ErrorKind::DirectoryNotEmpty
    );
    #[cfg(not(windows))]
    assert_eq!(
        kind(
            &mut l,
            FsRequest::Stat {
                path: path(dir.join("plain").join("child")),
                follow_symlinks: true
            }
        ),
        ErrorKind::NotADirectory
    );
    let d = match run(&mut l, FsRequest::OpenDir { path: path(&dir) }) {
        Ok(FsResult::Opened(h)) => h,
        other => panic!("opendir: {other:?}"),
    };
    let mut tiny = [0u8; 3];
    let e = run(
        &mut l,
        FsRequest::ReadDir {
            dir: d,
            buffer: provided(&mut tiny),
        },
    )
    .expect_err("no entry fits");
    assert_eq!(e.kind, ErrorKind::ResourceLimit);
    let mut page = [0u8; 64];
    let r = run(
        &mut l,
        FsRequest::ReadDir {
            dir: d,
            buffer: provided(&mut page),
        },
    );
    let Ok(FsResult::Directory { n, .. }) = r else {
        panic!("retained entry: {r:?}")
    };
    assert_eq!(
        DirEntries::new(&page[..n]).collect::<Vec<_>>(),
        [(FileType::File, &b"plain"[..])],
        "the entry that did not fit is delivered next"
    );
    close(&mut l, d);
    assert!(
        l.fs(
            FsRequest::Read {
                file: d,
                buffer: ReadBuf::Pooled,
                offset: None
            },
            Token(1)
        )
        .is_err()
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// Close cancels queued requests before Closed; unstarted buffers are never touched.
pub fn fifo_cancel_close<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "cancel");
    std::fs::create_dir(&dir).expect("fixture");
    let file = dir.join("file");
    let h = open(
        &mut l,
        &file,
        FileOptions {
            read: true,
            write: true,
            create: true,
            truncate: true,
            ..FileOptions::default()
        },
    );
    let write = l
        .fs(
            FsRequest::Write {
                file: h,
                buffer: write_buf(b"unwritten"),
                offset: None,
            },
            Token(1),
        )
        .expect("write");
    let mut bytes = [0x5a; 32];
    let read = l
        .fs(
            FsRequest::Read {
                file: h,
                buffer: provided(&mut bytes),
                offset: Some(0),
            },
            Token(2),
        )
        .expect("queued read");
    let fstat = l.fs(FsRequest::Fstat { file: h }, Token(3)).expect("fstat");
    l.close(h, Token(4)).expect("close");
    assert!(l.fs(FsRequest::Fstat { file: h }, Token(5)).is_err());
    let mut out = Completions::with_capacity(1);
    let until = l.now() + Duration::from_secs(10);
    let mut terminal = Vec::new();
    let mut wrote = false;
    loop {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        let Some(c) = out.drain().next() else {
            continue;
        };
        if matches!(c.result, OpResult::Closed) {
            break;
        }
        let op = c.op.expect("request completion");
        assert!(c.terminal && !terminal.contains(&op), "exactly once");
        match c.result {
            OpResult::Fs(FsResult::Wrote(9)) if op == write => wrote = true,
            OpResult::Cancelled => {}
            other => panic!("unexpected {other:?}"),
        }
        if op != write {
            assert!(matches!(c.token, Token(2 | 3)));
        }
        terminal.push(op);
    }
    terminal.sort();
    let mut expected = vec![write, read, fstat];
    expected.sort();
    assert_eq!(terminal, expected, "every request precedes Closed");
    assert_eq!(
        bytes, [0x5a; 32],
        "an unstarted read never touched its buffer"
    );
    for _ in 0..4 {
        l.turn(Timeout::Now, &mut out).expect("quiet");
        assert!(out.is_empty());
    }
    let content = std::fs::read(&file).expect("file");
    if wrote {
        assert_eq!(content, b"unwritten");
    } else {
        // D8: a write already running on a pool thread when close cancelled it
        // runs to its end and still reports Cancelled; mutations are not rolled
        // back. It never runs partially, and a write that never started never
        // writes (the queued read above proves unstarted buffers stay untouched).
        assert!(
            content.is_empty() || content == b"unwritten",
            "cancelled write left partial content: {content:?}"
        );
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// DESIGN §10 rule 3: a producer that queues a post before every turn cannot
/// starve filesystem requests. Queued turns never block. Backend-executed (WASI)
/// requests are native operations and get their discovery step; pool requests
/// are not, so those turns make no native call at all.
pub fn queued_posts_do_not_starve_requests<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "starvation");
    std::fs::create_dir(&dir).expect("fixture");
    let file = dir.join("file");
    std::fs::write(&file, b"payload").expect("fixture");
    let poster = l.poster();
    let mut out = Completions::with_capacity(4);
    let mut posts = 0u64;
    let mut serve = |l: &mut Driver<B>, pending: &mut Vec<OpId>, phase: &str| {
        let until = l.now() + Duration::from_secs(2);
        let mut turns = 0;
        let mut results = Vec::new();
        while !pending.is_empty() {
            assert!(
                l.now() < until,
                "{phase}: starved behind queued posts after {turns} turns: {pending:?}"
            );
            poster
                .post(Token(90), Payload::U64(posts))
                .expect("replenish");
            let info = l.turn(Timeout::Now, &mut out).expect("turn");
            turns += 1;
            assert_eq!(info.os_waits, 0, "a queued turn never blocks");
            assert!(info.discovery_polls <= 1);
            if B::FILESYSTEM == backend::Filesystem::Pool {
                assert_eq!(info.discovery_polls, 0, "pool results are queued work");
            }
            for c in out.drain() {
                match c.result {
                    OpResult::Posted(Payload::U64(_)) => posts += 1,
                    OpResult::Fs(result) => {
                        let op = c.op.expect("request");
                        assert!(pending.contains(&op), "exactly once");
                        pending.retain(|p| *p != op);
                        results.push((op, result));
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
        }
        results
    };
    // Phase 1: only path requests are pending. On WASI they are the backend's
    // native operations; nothing else would give the backend a turn.
    let stat = l
        .fs(
            FsRequest::Stat {
                path: path(&file),
                follow_symlinks: true,
            },
            Token(1),
        )
        .expect("path request");
    let mkdir = l
        .fs(
            FsRequest::Mkdir {
                path: path(dir.join("made")),
                mode: 0o755,
            },
            Token(2),
        )
        .expect("path mutation");
    for (op, result) in serve(&mut l, &mut vec![stat, mkdir], "path requests") {
        match result {
            FsResult::Metadata(m) if op == stat => assert_eq!(m.size, 7),
            FsResult::Done if op == mkdir => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(dir.join("made").is_dir(), "the mutation ran");
    // Phase 2: an open and a handle request under the same producer.
    let open_op = l
        .fs(
            FsRequest::Open {
                path: path(&file),
                options: FileOptions::default(),
            },
            Token(3),
        )
        .expect("open");
    let results = serve(&mut l, &mut vec![open_op], "open");
    let [(_, FsResult::Opened(handle))] = results.as_slice() else {
        panic!("open: {results:?}")
    };
    let handle = *handle;
    let fstat = l
        .fs(FsRequest::Fstat { file: handle }, Token(4))
        .expect("handle request");
    let results = serve(&mut l, &mut vec![fstat], "handle request");
    assert!(matches!(results.as_slice(), [(_, FsResult::Metadata(m))] if m.size == 7));
    assert!(posts >= 3, "the producer's posts were delivered too");
    while l
        .turn(Timeout::Now, &mut out)
        .expect("drain posts")
        .completions
        != 0
    {}
    close(&mut l, handle);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// A pooled read waits for a lease without spinning, then completes with real bytes.
pub fn pooled_lease_wait<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(Config {
        pooled_buffers: 1,
        pooled_buffer_size: 64,
        ..config()
    })
    .expect("loop");
    let dir = fresh(root, "leases");
    std::fs::create_dir(&dir).expect("fixture");
    let file = dir.join("file");
    std::fs::write(&file, [7u8; 64]).expect("fixture");
    let h = open(&mut l, &file, FileOptions::default());
    let request = |offset| FsRequest::Read {
        file: h,
        buffer: ReadBuf::Pooled,
        offset: Some(offset),
    };
    let Ok(FsResult::Read {
        n: 64,
        lease: Some(held),
    }) = run(&mut l, request(0))
    else {
        panic!("first pooled read")
    };
    let waiting = l
        .fs(request(32), Token(8))
        .expect("accepted while no lease");
    let mut out = Completions::with_capacity(4);
    for _ in 0..3 {
        let at = l.now() + Duration::from_millis(5);
        let timer = l.timer(at, None, Token(9)).expect("timer");
        let mut polls = 0;
        loop {
            let info = l.turn(Timeout::Until(at), &mut out).expect("turn");
            polls += 1;
            if !out.is_empty() {
                assert_eq!(out.len(), 1);
                assert!(matches!(out[0].result, OpResult::Timer), "{:?}", out[0]);
                break;
            }
            assert_eq!(info.completions, 0);
        }
        assert!(
            polls <= 2,
            "no spin while waiting for a lease ({polls} turns)"
        );
        close(&mut l, timer);
    }
    drop(held);
    let r = wait(&mut l, waiting);
    let Ok(FsResult::Read {
        n: 32,
        lease: Some(bytes),
    }) = r
    else {
        panic!("released lease starts the read: {r:?}")
    };
    assert_eq!(bytes.as_slice(), [7u8; 32]);
    drop(bytes);
    close(&mut l, h);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// Collect watch records until `done` accepts the accumulated list.
#[cfg(not(target_arch = "wasm32"))]
pub fn watch_until<B: Backend>(
    l: &mut Driver<B>,
    h: Handle,
    records: &mut Vec<(WatchKind, String)>,
    mut done: impl FnMut(&[(WatchKind, String)]) -> bool,
) -> bool {
    let mut out = Completions::with_capacity(8);
    let until = l.now() + Duration::from_secs(10);
    let mut overflow = false;
    while !done(records) {
        assert!(
            l.now() < until,
            "watch deadline; records so far: {records:?}"
        );
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            assert_eq!(c.handle, Some(h));
            let OpResult::Watch {
                events,
                overflow: lost,
            } = c.result
            else {
                panic!("unexpected {:?}", c.result);
            };
            assert!(!c.terminal);
            overflow |= lost;
            for (kind, name) in WatchEvents::new(events.as_slice()) {
                records.push((kind, String::from_utf8_lossy(name).into_owned()));
            }
        }
    }
    overflow
}
#[cfg(not(target_arch = "wasm32"))]
fn stop<B: Backend>(l: &mut Driver<B>, h: Handle, stopped: bool) {
    if stopped {
        l.fs_watch_stop(h, Token(50)).expect("stop");
    } else {
        l.close(h, Token(50)).expect("close watch");
    }
    let mut out = Completions::with_capacity(4);
    let until = l.now() + Duration::from_secs(10);
    let mut terminal = false;
    loop {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        for c in out.drain() {
            match c.result {
                // Batches published before the stop request may still arrive first.
                OpResult::Watch { .. } => assert!(!terminal),
                OpResult::Stopped if stopped => terminal = true,
                OpResult::Cancelled if !stopped => terminal = true,
                OpResult::Closed => {
                    assert!(terminal, "terminal operation precedes Closed");
                    return;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn names(records: &[(WatchKind, String)], name: &str) -> Vec<WatchKind> {
    records
        .iter()
        .filter(|(_, n)| n == name)
        .map(|(k, _)| *k)
        .collect()
}

/// Directory watch: liveness barrier, content change, rename ordering, stop.
#[cfg(not(target_arch = "wasm32"))]
pub fn watch_directory<B: Backend>(root: &Path) {
    use std::io::Write;
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "watched");
    std::fs::create_dir(&dir).expect("fixture");
    std::fs::write(dir.join("existing"), b"x").expect("fixture");
    let h = l
        .fs_watch(&path(&dir), WatchOptions::default(), Token(1))
        .expect("watch");
    let mut records = Vec::new();
    std::fs::write(dir.join("sentinel"), b"").expect("sentinel");
    watch_until(&mut l, h, &mut records, |r| {
        !names(r, "sentinel").is_empty()
    });
    let barrier = records.len();
    let mut existing = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("existing"))
        .expect("open existing");
    existing.write_all(b"appended").expect("append");
    existing.sync_all().expect("flush");
    drop(existing);
    // FSEvents reports cumulative per-path flags, so a content change to a
    // recently created file can read as Rename there (as in libuv and Node).
    let fsevents = cfg!(target_os = "macos");
    watch_until(&mut l, h, &mut records, |r| {
        let kinds = names(&r[barrier..], "existing");
        kinds.contains(&WatchKind::Change) || (fsevents && !kinds.is_empty())
    });
    let changed = records.len();
    std::fs::rename(dir.join("existing"), dir.join("renamed")).expect("rename");
    std::fs::remove_file(dir.join("renamed")).expect("remove");
    watch_until(&mut l, h, &mut records, |r| {
        let r = &r[changed..];
        names(r, "existing").contains(&WatchKind::Rename) && !names(r, "renamed").is_empty()
    });
    let after = &records[changed..];
    let old = after
        .iter()
        .position(|(_, n)| n == "existing")
        .expect("old name");
    let new = after
        .iter()
        .position(|(_, n)| n == "renamed")
        .expect("new name");
    assert!(old < new, "native order is preserved: {after:?}");
    assert!(
        records
            .iter()
            .all(|(_, n)| !n.contains('/') && !n.contains('\\')),
        "non-recursive watch reports direct children: {records:?}"
    );
    stop(&mut l, h, true);
    std::fs::write(dir.join("late"), b"").expect("after stop");
    let mut out = Completions::with_capacity(4);
    let at = l.now() + Duration::from_millis(50);
    while l.now() < at {
        l.turn(Timeout::Until(at), &mut out).expect("quiet");
        assert!(
            out.is_empty(),
            "no delivery after Closed: {:?}",
            out.iter().collect::<Vec<_>>()
        );
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// Watching a single file reports its base name; close cancels before Closed.
#[cfg(not(target_arch = "wasm32"))]
pub fn watch_file<B: Backend>(root: &Path) {
    use std::io::Write;
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "watched-file");
    std::fs::create_dir(&dir).expect("fixture");
    let file = dir.join("subject");
    std::fs::write(&file, b"x").expect("fixture");
    let h = l
        .fs_watch(&path(&file), WatchOptions::default(), Token(1))
        .expect("file watch");
    let mut records = Vec::new();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .expect("open");
    f.write_all(b"more").expect("append");
    f.sync_all().expect("flush");
    drop(f);
    watch_until(&mut l, h, &mut records, |r| {
        names(r, "subject").contains(&WatchKind::Change)
    });
    std::fs::rename(&file, dir.join("elsewhere")).expect("rename away");
    watch_until(&mut l, h, &mut records, |r| {
        names(r, "subject").contains(&WatchKind::Rename)
    });
    assert!(
        records.iter().all(|(_, n)| n == "subject"),
        "a file watch names only its subject: {records:?}"
    );
    stop(&mut l, h, false);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// Held leases defer delivery without spinning; loss beyond the bound is reported.
#[cfg(not(target_arch = "wasm32"))]
pub fn watch_backpressure<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(Config {
        pooled_buffers: 1,
        ..config()
    })
    .expect("loop");
    let dir = fresh(root, "watch-pressure");
    std::fs::create_dir(&dir).expect("fixture");
    let h = l
        .fs_watch(&path(&dir), WatchOptions::default(), Token(1))
        .expect("watch");
    std::fs::write(dir.join("first"), b"").expect("first");
    let mut out = Completions::with_capacity(8);
    let until = l.now() + Duration::from_secs(10);
    let held = loop {
        assert!(l.now() < until);
        l.turn(Timeout::Until(until), &mut out).expect("turn");
        if let Some(c) = out.drain().next() {
            let OpResult::Watch { events, .. } = c.result else {
                panic!("unexpected {:?}", c.result)
            };
            break events;
        }
    };
    // Enough distinct names to exceed the bounded per-watch storage.
    let count = 3000;
    for i in 0..count {
        std::fs::write(dir.join(format!("n{i:05}")), b"").expect("event");
    }
    for _ in 0..3 {
        let at = l.now() + Duration::from_millis(20);
        let timer = l.timer(at, None, Token(2)).expect("timer");
        // Native deliveries (FSEvents notifies from its queue) may wake the loop:
        // such turns block (`os_waits`) or, when the notification landed while the
        // loop was running, make one zero-timeout discovery poll (`discovery_polls`).
        // A spin is a turn that made no native call and produced nothing: the
        // backend claiming work it cannot perform while every lease is held.
        let (mut spins, mut turns) = (0, 0);
        loop {
            let info = l.turn(Timeout::Until(at), &mut out).expect("turn");
            assert!(info.os_waits + info.discovery_polls <= 1);
            turns += 1;
            if let Some(c) = out.drain().next() {
                assert!(
                    matches!(c.result, OpResult::Timer),
                    "no batch without a lease: {c:?}"
                );
                assert!(l.now() >= at);
                break;
            }
            spins += usize::from(info.os_waits + info.discovery_polls == 0);
        }
        assert!(
            spins <= 1,
            "no spin while every lease is held ({spins} turns without a native call)"
        );
        assert!(
            turns <= 64,
            "wakes are bounded by native deliveries, not a poll loop ({turns} turns in 20 ms)"
        );
        close(&mut l, timer);
    }
    drop(held);
    let mut records = Vec::new();
    let overflow = watch_until(&mut l, h, &mut records, |r| {
        r.iter().any(|(_, n)| n.starts_with('n'))
    });
    let mut overflowed = overflow;
    let quiet = l.now() + Duration::from_millis(500);
    while l.now() < quiet {
        l.turn(Timeout::Until(quiet), &mut out).expect("turn");
        for c in out.drain() {
            if let OpResult::Watch { overflow, .. } = c.result {
                overflowed |= overflow;
            }
        }
    }
    assert!(
        overflowed,
        "{count} events while the only lease was held must report loss"
    );
    stop(&mut l, h, false);
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// Recursive scope where native (FSEvents, ReadDirectoryChangesW), Unsupported elsewhere.
#[cfg(not(target_arch = "wasm32"))]
pub fn watch_recursive<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(config()).expect("loop");
    let dir = fresh(root, "watch-tree");
    std::fs::create_dir_all(dir.join("sub")).expect("fixture");
    let result = l.fs_watch(&path(&dir), WatchOptions { recursive: true }, Token(1));
    if cfg!(any(target_os = "macos", windows)) {
        let h = result.expect("native recursive watch");
        std::fs::write(dir.join("sub").join("deep"), b"").expect("nested event");
        let mut records = Vec::new();
        let nested = if cfg!(windows) {
            "sub\\deep"
        } else {
            "sub/deep"
        };
        watch_until(&mut l, h, &mut records, |r| !names(r, nested).is_empty());
        stop(&mut l, h, true);
        let flat = l
            .fs_watch(&path(&dir), WatchOptions::default(), Token(2))
            .expect("flat watch");
        std::fs::write(dir.join("sub").join("hidden"), b"").expect("nested");
        std::fs::write(dir.join("top"), b"").expect("direct");
        let mut records = Vec::new();
        watch_until(&mut l, flat, &mut records, |r| !names(r, "top").is_empty());
        assert!(
            records.iter().all(|(_, n)| !n.contains("hidden")),
            "{records:?}"
        );
        stop(&mut l, flat, true);
    } else {
        assert_eq!(
            result.expect_err("no native recursion").kind,
            ErrorKind::Unsupported
        );
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
    assert!(!l.alive());
}

/// WASI: paths outside every preopen are not reachable, capabilities without a
/// `wasi:filesystem` equivalent reject before acceptance, and watches are Unsupported.
#[cfg(target_os = "wasi")]
pub fn capability_scope<B: Backend>(root: &Path) {
    let mut l = Driver::<B>::new(config()).expect("loop");
    let e = run(
        &mut l,
        FsRequest::Stat {
            path: path("/etc/passwd"),
            follow_symlinks: true,
        },
    )
    .expect_err("no ambient authority");
    assert_eq!(e.kind, ErrorKind::NotFound);
    let e = run(
        &mut l,
        FsRequest::Stat {
            path: path("relative/without/dot/preopen"),
            follow_symlinks: true,
        },
    )
    .expect_err("relative paths need a '.' preopen");
    assert_eq!(e.kind, ErrorKind::NotFound);
    let r = run(
        &mut l,
        FsRequest::Stat {
            path: path(root),
            follow_symlinks: true,
        },
    );
    assert!(
        matches!(&r, Ok(FsResult::Metadata(m)) if m.kind == FileType::Directory),
        "{r:?}"
    );
    for request in [
        FsRequest::RealPath {
            path: path(root),
            buffer: ReadBuf::Pooled,
        },
        FsRequest::Chmod {
            target: FsTarget::Path {
                path: path(root),
                follow_symlinks: true,
            },
            mode: 0o700,
        },
        FsRequest::Access {
            path: path(root),
            mode: AccessMode {
                read: true,
                ..AccessMode::default()
            },
        },
    ] {
        assert_eq!(
            l.fs(request, Token(1)).expect_err("rejected").kind,
            ErrorKind::Unsupported
        );
    }
    assert_eq!(
        l.fs_watch(&path(root), WatchOptions::default(), Token(2))
            .expect_err("no wasi watch API")
            .kind,
        ErrorKind::Unsupported
    );
    assert!(!l.alive());
}
