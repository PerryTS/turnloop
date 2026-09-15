# tl-i06 — typed file operations and filesystem watches

Status: implemented for every platform in scope. macOS runtime, WASI 0.2 and WASI
0.3 (Wasmtime 46) are verified here; Linux, Windows and FreeBSD compile cleanly
(strict cross-clippy) but their runtime is **UNRUN** and must come from CI.
Base: `main` 63f5eee (the Codex checkpoint was rebased onto it, then replaced).
Commits are on `lane/tl-i06`; nothing was pushed. No dependency, lockfile or soak
policy change.

## 1. What the checkpoint had, and what was kept

The Codex checkpoint (`2fc61e7`, originally `d96ba84`) was a useful sketch but was
not kept as code:

- Watches ran one helper thread per watch on Linux, FreeBSD and Windows. DESIGN
  §5a.4 moves fs-watch off dedicated threads, and each turn scanned every watch
  slot (O(max_handles) per turn). Events carried no file names, which Node's
  `fs.watch(eventType, filename)` needs.
- The pool path returned a post-acceptance `ResourceLimit` when the shared queue
  was full while a FIFO successor started, used `expect` on a withdrawn job
  (loop drop would panic a pool thread) and forbade non-regular files.
- Directory listing re-opened and skipped `cookie` entries per page (O(n²), and
  wrong under concurrent mutation). There was no WASI implementation.
- It overwrote `LANE_REPORT.md` (the adapters-db report); that file is restored.

Kept ideas: prepared `FsPath`, handle-scoped FIFOs, `Arc<Slot>` reusable pool work,
the error-kind additions, and FSEvents on a dispatch queue.

## 2. Request surface (API)

Everything is in `turnloop::` (module `crates/turnloop/src/fs/mod.rs`).

```rust
impl Loop {
    // Native: shared blocking pool. WASI: wasi:filesystem preopens. Web: Unsupported.
    pub fn fs(&mut self, request: FsRequest, token: Token) -> Result<OpId>;
    // inotify / FSEvents+kqueue / kqueue / ReadDirectoryChangesW. WASI, web: Unsupported.
    pub fn fs_watch(&mut self, path: &FsPath, options: WatchOptions, token: Token) -> Result<Handle>;
    pub fn fs_watch_stop(&mut self, watch: Handle, token: Token) -> Result<()>; // Stopped, Closed
}
pub struct FsPath;                  // prepared once (CString / UTF-16 / UTF-8); clone = Arc bump
pub enum FsRequest {
    Open { path, options: FileOptions }, OpenDir { path },     // -> Opened(Handle)
    Close { file },                                             // native close with its error
    Read { file, buffer: ReadBuf, offset: Option<u64> },       // None = handle cursor
    Write { file, buffer: WriteBuf, offset: Option<u64> },     // one write, may be short
    Sync { file, data_only }, Truncate { file, size },
    Stat { path, follow_symlinks }, Fstat { file },            // -> Metadata
    ReadDir { dir, buffer: ReadBuf },                          // -> Directory records
    Mkdir { path, mode }, Rmdir { path }, Unlink { path }, Rename { from, to },
    Link { existing, link }, Symlink { target, link, kind }, ReadLink { path, buffer },
    RealPath { path, buffer },                                  // -> Bytes
    Access { path, mode: AccessMode }, Chmod { target: FsTarget, mode },
    Chown { target, uid, gid }, SetTimes { target, accessed: TimeChange, modified },
    CopyFile { from, to, exclusive },
}
pub enum FsResult { Opened(Handle), Read { n, lease }, Wrote(usize), Metadata(Metadata),
                    Directory { n, lease, eof }, Bytes { n, lease }, Done }
OpResult::Fs(FsResult)
OpResult::Watch { events: BufLease, overflow: bool }            // nonterminal batches
```

- **FileOptions** covers Node's string and numeric flags: read, write, create,
  exclusive, truncate, append, sync (O_SYNC), data_sync (O_DSYNC),
  follow_symlinks (O_NOFOLLOW), mode (umask-filtered; Windows maps the owner write
  bit to the read-only attribute as libuv does).
- **FileMetadata**: kind, size, mode, device, inode, links, uid, gid, rdev,
  block_size, blocks, accessed/modified/changed/created with nanoseconds. A field
  the platform does not report is `None`. Linux uses `statx` for birth time.
  Windows mode follows libuv (type bits | 0o666, or 0o444 when read-only).
  `Metadata` is a lease into per-loop storage (dereferences to `FileMetadata`) so
  a 232-byte value never enters every `Completion`; steady-state `stat` is
  allocation-free.
- **Records** (`RECORD_HEADER` = 4 bytes: tag, 0, u16 length): `DirEntries`
  yields `(FileType, name)`, `WatchEvents` yields `(WatchKind::{Rename, Change},
  name)`. Names are native bytes on Unix, WTF-8 on Windows, UTF-8 on WASI. A page
  never splits a name; a buffer too small for the next name is `ResourceLimit`
  and the entry is kept for the next read.
- **Open handles** are reserved at submission but stay hidden until `Opened`; a
  failed or cancelled open never exposes a handle (its reservation is released).
  `Close` closes the native object and reports its error; `Loop::close` releases
  the handle (closing a still-open descriptor on the loop thread).
- **Errors**: new `ErrorKind`s `PermissionDenied`, `AlreadyExists`,
  `NotADirectory`, `IsADirectory`, `DirectoryNotEmpty`; `os` keeps errno /
  `GetLastError`, and on WASI the wasi-libc errno for the `error-code`.
- **Watch semantics** follow libuv: every event but content/attribute change is
  `Rename`; names are relative to the watched directory with native separators;
  an event on the watched root (or a watched file) carries its final component.
  `overflow: true` means events were lost (kernel overflow, or the bounded
  16 KiB per-watch store filled while leases were held); the host rescans.
  FSEvents reports cumulative flags per path, so a change to a recently created
  file can read as `Rename` on macOS — the same as libuv/Node there.

### Perry mapping (consumer survey of `perry-runtime/src/fs`)

Perry performs every fs operation synchronously on the JS thread today. The
surface covers what its promise/callback/FileHandle/stream paths call:
open/close/read/write (positional and cursor), readv/writev (as repeated
Read/Write), stat/lstat/fstat (all Stats fields incl. ns and bigint), opendir,
readdir (withFileTypes from record tags; `Unknown` falls back to lstat as Node
does), mkdir, rmdir, rename, unlink, fsync/fdatasync, truncate (Open+Truncate),
ftruncate, access/exists, chmod/fchmod/lchmod, chown family, utimes/futimes/
lutimes, symlink/readlink/realpath/link, copyFile(COPYFILE_EXCL). Host
compositions: recursive mkdir/rm/readdir/cp, mkdtemp (random name + Mkdir retry
on `AlreadyExists`), writeFile/appendFile (Write loop), `fs.watchFile`
(host timer + `Stat`, no driver thread), recursive `fs.watch` on Linux (one watch
per directory, as Node 20+ does). Not in the surface: statfs and COPYFILE_FICLONE
(host blocking jobs remain available).

## 3. Decisions

- **D8 resolved (DESIGN.md amended):** typed requests use the one shared bounded
  pool on Linux, macOS/BSD **and Windows**. Windows per-handle synchronous workers
  remain only for adopted descriptors (stdio, `Detached::from_handle`), whose reads
  can block indefinitely on pipes/consoles and must not occupy the shared pool.
- **Queue reservations:** each accepted request reserves one pool queue slot
  (`blocking::reserve`), so a full queue rejects *before* acceptance and a FIFO
  successor always starts (`push_reserved` never fails or grows the queue).
  Boxed host jobs honour reservations too. Only a FIFO head is ever queued.
- **Watches are loop handles, no turnloop threads (§5a.4):** Linux: one inotify fd
  per loop registered with epoll (shared watch descriptors for the same inode);
  kqueue: `EVFILT_VNODE` on the loop's kqueue (files on macOS; everything on
  BSD/iOS, carried through a new `Ready::vnode` field); macOS directories:
  FSEvents streams (CoreServices loaded lazily with `dlopen`, no launch-time
  framework dependency) on one private serial dispatch queue — the only OS-owned
  threads, now listed as an exception in §5a.4; Windows: overlapped
  `ReadDirectoryChangesW` associated with the loop's IOCP under a never-reused
  key, re-armed per completion, storage retired until the kernel acknowledges.
- **WASI:** no threads, so requests run inside `poll`, at most the event budget per
  turn, against preopens only (no ambient path authority). 0.2 lowers reads
  directly into caller memory through the backend's scratch-arena
  `cabi_realloc`. 0.3 lowers every import by hand and blocks on a private
  waitable set, preserving the shadow-stack context around each raw call: the
  generated bindings trap in debug on the pinned toolchain (reproduced with a bare
  `wasip3::filesystem::preopens::get_directories()` call, outside turnloop).
  Unsupported before acceptance on WASI: RealPath, Chmod, Chown, Access with
  permission bits, and watches.
- **Web:** Unsupported; no OPFS host mapping was invented.
- **Pool vs backend selection** is a `Backend::FILESYSTEM` constant (`Pool`,
  `Backend`, `Unsupported`), so the driver branches statically.

## 4. Implementation map

| Area | Files |
|---|---|
| Surface, records, metadata leases | `crates/turnloop/src/fs/mod.rs` |
| Native pool service (FIFO, reservations, withdrawal on drop) | `fs/service.rs` |
| Unix / Windows system calls | `fs/unix.rs`, `fs/windows.rs` |
| Bounded watch record store | `fs/watch.rs` |
| inotify + kqueue vnode + FSEvents | `backend/watch.rs`, `backend/fsevents.rs` |
| ReadDirectoryChangesW on IOCP | `backend/iocp/watch.rs` |
| WASI core (queues, preopen resolution) | `backend/wasi_fs.rs` |
| WASI 0.2 / 0.3 bindings | `backend/wasi_p2/fs.rs` (+ `abi::file_read`), `backend/wasi_p3/fs.rs` |
| Driver wiring | `driver.rs` (`fs`, `fs_watch`, hidden handles), `backend/mod.rs` contract |
| Contracts | `turnloop-contract/src/filesystem.rs`, `tests/filesystem.rs`, `tests/wasi.rs`, `tests/allocations.rs` |
| Wasmtime fixture preopen | `scripts/ci/wasmtime-runner.sh` (fresh `/turnloop-fs`, removed on exit) |

## 5. What the tests prove

Shared contracts (native and both WASI versions):

- `bytes_metadata_namespace`: cursor and positional writes and reads (provided and
  pooled) with bytes verified by `std::fs`, EOF, sync, fstat/stat kind and size
  (inode, device, link count and type bits natively), truncate, nanosecond `SetTimes` round trip, `Close` then rejected
  requests, hard link count, symlink + lstat + readlink, realpath (native), access,
  exclusive copy + `AlreadyExists`, rename, `DirectoryNotEmpty`, directory pages at
  9/16/4096 bytes alternating provided/pooled buffers, full cleanup, `!alive()`.
- `errors`: `NotFound`, `AlreadyExists`, `DirectoryNotEmpty` and (not on Windows,
  which reports a missing path) `NotADirectory`, each with its native code; a 3-byte page is `ResourceLimit` and the entry is delivered next.
- `fifo_cancel_close`: close before any turn: every request terminates exactly once
  before `Closed`; an unstarted read never touches its buffer; file content matches
  whether the head write won or was cancelled.
- `pooled_lease_wait`: with the only lease held, a pooled read waits across three
  timer expiries with at most two turns each (no spin), then completes with bytes.
- WASI `capability_scope`: paths outside preopens are `NotFound`, unsupported
  requests reject before acceptance, watches are `Unsupported`.

Native only:

- `pool_backpressure_rejects_before_acceptance`: with all pool threads held, exactly
  `queue_capacity` requests are accepted, the next is `ResourceLimit`, host jobs are
  bounded by the same reservations, every accepted request completes exactly once.
- `cancellation_withdraws_unstarted_and_reports_started`: a queued cancel completes
  immediately without touching its buffer; the queued head reports `Cancelled`; the
  FIFO successor then runs with correct bytes.
- `cancelled_open_and_loop_drop_never_touch_buffers`: a cancelled open exposes no
  handle; dropping the loop withdraws its queued pool jobs, buffers mutated after
  drop are never touched when the pool threads later dequeue the stale entries.
- `permission_denied_is_reported`: read-only file rejects writers and `Access(write)`
  with `PermissionDenied` (requires an unprivileged runner).
- `watch_directory`: liveness barrier, content change (`Change`; FSEvents may
  report `Rename` for a recently created file), rename then delete with the
  old name before the new, no nested names, `Stopped` before `Closed`, nothing after.
- `watch_file`: file watch reports only its base name, `Change` then `Rename`;
  close gives `Cancelled` then `Closed`.
- `watch_backpressure`: with the only lease held, 3000 events produce no batch and
  no spin (idle turns without an OS wait ≤ 1 per timer), and loss is reported.
- `watch_recursive`: nested names with native separators on macOS/Windows, none in a
  flat watch; `Unsupported` on inotify/kqueue.
- Allocation gates (`tests/allocations.rs`): 200 rounds of typed write, provided
  read, pooled read, fstat, stat and truncate allocate **zero** times (loop thread;
  all threads on Windows; release WASI 0.2 and 0.3), after proving the counter
  observes an allocation; 100 watch batches allocate zero times natively.

## 6. Verification ledger

Every command, in order, with its result. `CARGO_BUILD_JOBS=4`. Clippy flags are
`-D warnings -D clippy::undocumented_unsafe_blocks` throughout.

| # | Command | Result |
|---|---|---|
| 1 | Inspection: `cat`/`sed`/`grep`/`git show` of the brief, DESIGN.md, REMAINING_WORK.md I06, checkpoint diff and log tail, core sources; read-only survey of Perry `perry-runtime/src/fs` and `perry-stdlib` | PASS |
| 2 | `git fetch origin main && git rebase origin/main` (checkpoint onto 63f5eee) | PASS |
| 3 | `cargo check -p turnloop` (first build of the new surface) | FAIL (missing `fs/watch.rs`, non-exhaustive `Operation::WatchFs`), fixed, then PASS |
| 4 | `cargo clippy -p turnloop --all-targets -- $FLAGS` | FAIL (dead `Service::has_work`, unannotated `transmute`, large enum variants), fixed, then PASS |
| 5 | `cargo test -p turnloop-contract --test filesystem -- --test-threads=1` (macOS) | FAIL (directory page sort by type; an FSEvents stream left running after a failed test's loop drop, UB check on the dispatch thread), fixed: sort by name, stream teardown moved into `Stream::drop` |
| 6 | same, rerun | FAIL (`watch_directory`: FSEvents reported the append to a new file as Rename; `watch_backpressure`: 10 turns counted as spin although each had an OS wait), fixed: FSEvents cumulative-flag tolerance on macOS only, spin measured as turns with no OS wait and no output |
| 7 | same (after fixes) | PASS 11/11, repeated 3 times PASS |
| 8 | `cargo fmt --all && cargo fmt --all --check` | PASS |
| 9 | `cargo clippy --target x86_64-unknown-linux-gnu -p turnloop -p turnloop-contract --all-targets -- $FLAGS` | FAIL (`Ready::vnode` unread on epoll, `Ring::empty` unused, `unnecessary_cast` of mode bits), fixed, then PASS |
| 10 | `cargo clippy --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract --all-targets -- $FLAGS` | FAIL (`BOOL` import; dead watch store before IOCP watches existed), fixed, then PASS |
| 11 | clippy: macOS, Linux, Windows (`-p turnloop -p turnloop-contract --all-targets`) after IOCP watches | PASS ×3 |
| 12 | `cargo test -p turnloop-contract --test allocations -- --test-threads=1 steady_typed steady_watch` | FAIL (watch subject stalled: macOS reports content changes on close, not on write), fixed (reopen per round on Unix), then PASS |
| 13 | `cargo clippy --target wasm32-wasip2 -p turnloop -p turnloop-contract --all-targets -- $FLAGS` | FAIL (`BufferPool::available` cfg, `WatchFs` match arms), fixed; FAIL (`large_enum_variant` of `FsResult` on 32-bit), fixed by metadata leases; PASS |
| 14 | clippy macOS / wasm32-wasip2 / Linux / Windows after metadata leases | FAIL (`vec_box`), allowed with reason, then PASS ×4 |
| 15 | `CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-08-20 test --target wasm32-wasip2 -p turnloop-contract --test wasi --all-features -- --test-threads=1 filesystem` | PASS 5/5 |
| 16 | same with `--release --test allocations ... steady_typed` | PASS (zero allocations) |
| 17 | `cargo +nightly-2026-09-07 clippy --target wasm32-wasip3 -p turnloop -p turnloop-contract --all-targets --features turnloop/wasi-p3-experimental,turnloop-contract/wasi-p3-experimental -- $FLAGS` (generated-binding version) | FAIL (unused import), fixed, then PASS |
| 18 | `cargo +nightly-2026-09-07 test --target wasm32-wasip3 -p turnloop-contract --test wasi --all-features -- --test-threads=1 filesystem` (generated-binding version) | FAIL: wasm trap (out-of-bounds) in `wasip3 get_directories` lifting, debug |
| 19 | same `--release` (generated-binding version) | PASS 5/5 (not accepted: debug must pass) |
| 20 | Probe test calling bare `wasip3::filesystem::preopens::get_directories()` in the p3 debug `wasi` binary (temporary `wasip3` dev-dependency, reverted) | FAIL (same trap): toolchain issue, not turnloop |
| 21 | p3 clippy (#17) after hand-lowering every import | PASS |
| 22 | p3 `--test wasi ... filesystem`, debug and `--release` | PASS 5/5 and 5/5 |
| 23 | p3 `--release --test allocations ... steady_typed` | PASS (zero allocations) |
| 24 | `cargo fmt --all --check` | PASS |
| 25 | `cargo test -p turnloop -p turnloop-contract -- --test-threads=1` | PASS (lib 15, contract lib 22, allocations 12, filesystem 11, native_surface 17, lifetimes 1, others 0 on macOS) |
| 26 | `cargo test --workspace -- --test-threads=1` (foreground) | UNRUN: killed by the tool's 10-minute limit while compiling |
| 27 | `cargo test --workspace -- --test-threads=1` (background) | PASS, 0 failed. Correction: the log also captured the still-running cargo from #26 (every doc-test group appears twice), so the "80 groups / 339 tests" first recorded here was double-counted; a clean single run is #47 |
| 28 | `cargo test -p turnloop-contract --test filesystem -- --test-threads=1` (with the loop-drop test) | PASS 12/12 |
| 29 | `cargo clippy --workspace --all-targets -- $FLAGS` | PASS |
| 30 | `cargo clippy --workspace --all-targets --all-features -- $FLAGS` | PASS |
| 31 | `cargo clippy --target x86_64-unknown-linux-gnu -p turnloop -p turnloop-contract -p turnloop-io --all-targets [--all-features] -- $FLAGS` | PASS ×2 |
| 32 | `cargo clippy --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets [--all-features] -- $FLAGS` | PASS ×2 |
| 33 | `cargo clippy --target wasm32-wasip2 -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- $FLAGS` | PASS |
| 34 | `cargo +nightly-2026-09-07 clippy --target wasm32-wasip3 -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- $FLAGS` | PASS |
| 35 | `cargo clippy --target wasm32-unknown-unknown -p turnloop -p turnloop-contract --all-targets --all-features -- $FLAGS` | FAIL (`FsRequest::read_buffer` unused on web), fixed |
| 36 | web clippy (#35) default and `--all-features` | PASS ×2 |
| 37 | `cargo clippy --target wasm32-wasip1-threads -p turnloop --all-targets --all-features -- $FLAGS` (unsupported backend) | FAIL (same dead code), fixed, then PASS |
| 38 | `bash scripts/ci/no-tokio.sh` | PASS (all 9 policy targets, default and all features) |
| 39 | `python3 scripts/ci/soak.py` | PASS (251 locked versions; the existing rustls exception) |
| 40 | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS (core lib 8; `wasi` debug 27 and release 27; `allocations` release 10) |
| 41 | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | FAIL: core lib 10, `wasi` debug 28 and `allocations` release 11 passed (all filesystem tests passed in debug and release); release `wasi` aborted in the pre-existing `timer_precision` contract: median lateness 2.31 ms > 2 ms bound, samples 1.07–6.44 ms, host load average 148–182 (shared machine). Not filesystem code: no fs request runs in that test. |
| 41a | `cargo +nightly-2026-09-07 test --locked --release --target wasm32-wasip3 -p turnloop-contract --test wasi --all-features -- --nocapture --test-threads=1 timer_precision` ×3 | PASS ×3 (median 1.18, 1.86, 1.17 ms) |
| 41b | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` (rerun) | PASS (core lib 10; `wasi` debug 28 and release 28, median 1.17 ms; `allocations` release 11) |
| 42 | `cargo test -p turnloop -p turnloop-contract --all-features -- --test-threads=1` | PASS: 88 tests (lib 15, contract lib 22, allocations 13, executor 6, filesystem 12, native_surface 17, lifetimes 1, doc 2) |
| 43 | Linux runtime (epoll/inotify), Windows runtime (IOCP/RDCW), FreeBSD | UNRUN (no host) |
| 44 | `git fetch origin` | PASS, but this clone's `origin` (the local `windlass` clone) still had `main` at 63f5eee; `git fetch gh main` fetched GitHub `main` f6fc128 containing #20 and #16 |
| 45 | `git merge --no-ff gh/main` | 2 conflicts (see §10), resolved keeping both sides; merge-only tree `cargo check -p turnloop -p turnloop-contract --all-targets` PASS; committed `75011b4` |
| 46 | Rule 3 fixes: `cargo test -p turnloop --lib -- --test-threads=1 backend_path_request natively_accepted queued_core` | PASS 3/3; sabotage (flag disabled): FAIL as expected |
| 46a | `cargo test -p turnloop-contract --test filesystem -- --test-threads=1` | FAIL: `watch_backpressure` (2 idle turns: I01 now reports a notification-driven zero-timeout poll as `discovery_polls`, not `os_waits`); spin redefined as a turn with no native call and no output, plus ≤ 64 turns per 20 ms window; then PASS 13/13, `watch_backpressure` ×3 PASS; sabotage (watch `has_work` ignoring lease availability): FAIL as expected |
| 46b | wasip2 `--test wasi ... filesystem` | first version of the starvation contract passed even with the flag disabled (an open handle's request made the backend native); split into a path-only phase: PASS 6/6, sabotage FAIL ("starved behind queued posts after 480525 turns") |
| 47 | Post-merge `/scratchpad/verify2.sh`: `cargo fmt --all --check` | PASS |
| 48 | clippy `--workspace --all-targets` default and `--all-features` | PASS ×2 |
| 49 | clippy `--target x86_64-unknown-linux-gnu -p turnloop -p turnloop-contract -p turnloop-io --all-targets` default and `--all-features` | PASS ×2 |
| 50 | clippy `--target x86_64-pc-windows-msvc` (same packages) default and `--all-features` | PASS ×2 |
| 51 | clippy `--target wasm32-wasip2` (same packages) `--all-features`; `+nightly-2026-09-07 --target wasm32-wasip3` `--all-features` | PASS ×2 |
| 52 | clippy `--target wasm32-unknown-unknown -p turnloop -p turnloop-contract --all-targets` default and `--all-features` | PASS ×2 |
| 53 | `cargo test --workspace -- --test-threads=1` | PASS: 64 result groups, 278 tests, 0 failed |
| 54 | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS (core lib 11; `wasi` debug 33, release 33 with median lateness 1.66 ms; `allocations` release 11) |
| 55 | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | PASS (core lib 13; `wasi` debug 33, release 33 with median 1.11 ms; `allocations` release 12) |
| 56 | `bash scripts/ci/no-tokio.sh` | PASS (18 target/feature graphs) |
| 57 | `python3 scripts/ci/soak.py` | PASS (251 locked versions, 1 existing exception) |

`$FLAGS` = `-D warnings -D clippy::undocumented_unsafe_blocks`. Wasmtime 46.0.0 was
linked into this clone's ignored `.tools/bin` from the `wasm2` clone (read only).
Clippy runs through #37 used the default `nightly-2026-08-20` toolchain except the
p3 target (`nightly-2026-09-07`).

## 7. Commits on `lane/tl-i06`

| Commit | Content |
|---|---|
| `2fc61e7` | Codex checkpoint, rebased onto `main` 63f5eee (superseded by the commits below) |
| `b19e786` | Typed surface, pool service with reservations, Unix/Windows calls, inotify/kqueue/FSEvents watches, native contracts; restores `LANE_REPORT.md` |
| `1417d44` | ReadDirectoryChangesW on IOCP; filesystem and watch allocation gates |
| `284349d` | WASI core and 0.2 bindings; metadata leases; Wasmtime runner preopen |
| `5d83e8c` | WASI 0.3 hand-lowered bindings |
| `84adc74` | DESIGN D8/§5a.4/§6/§7.6 clarification; loop-drop buffer test; IOCP watch terminal cleanup; web/unsupported dead-code allowance |
| `58e6bda` | this report |
| `75011b4` | merge of GitHub `main` f6fc128 (#20 I01, #16 Windows semantics) |
| `d0d35b3` | DESIGN §10 rule 3 applied to filesystem requests; regressions |
| (last) | report update for the merge |

## 8. Needs CI (runtime not available here)

- **Linux (epoll):** `tests/filesystem.rs` (inotify paths, statx, fdatasync,
  permission test as non-root), `tests/allocations.rs` fs/watch gates.
- **Windows (IOCP):** the same binaries: `fs/windows.rs` (CreateFileW dispositions,
  OVERLAPPED offsets, FindFirstFileW pages, reparse tags, CopyFileW, symlink
  privilege on the runner), `iocp/watch.rs` (RDCW completions, cancellation
  acknowledgement, retired storage on release/drop), all-thread allocation gate.
- **FreeBSD (kqueue, best effort):** vnode watches and `st_birthtime` compile paths
  were not cross-checked (no FreeBSD target installed).
- **WASI:** the required `run-tests.py wasi` jobs for both targets; this lane ran
  them locally (see ledger) with the updated Wasmtime runner.

## 9. Open items

- WASI 0.3 remains experimental (feature-gated). Each request blocks the agent on a
  private waitable set inside `poll`; this is an additional wait beyond the one
  counted in `PollInfo`, like 0.2's synchronous host calls. Directory entry and link
  names are owned strings from the host on both WASI versions (not gated).
- Linux recursive watches are left to the host (Node's own approach); a built-in
  tree-walking option would need pool-backed scans.
- Vectored `readv`/`writev`, `statfs` and clone-copy are not in the surface.
- I01 is merged and applied (§10).

## 10. Merge of `main` with I01 (#20) and Windows semantics (#16)

Merged GitHub `main` f6fc128 with a merge commit (`75011b4`). This clone's `origin`
remote is the local `windlass` clone, whose `main` was still 63f5eee, so the merge
source was `gh/main`.

Conflicts, both resolved by keeping both sides:

- `crates/turnloop/src/driver.rs`, `struct Op` and its initializer in `new_op`:
  this lane's `fs: bool` (typed filesystem request) and I01's `native: bool`
  (counted in `native_pending`) are both kept.
- `crates/turnloop-contract/tests/allocations.rs`, end of file: this lane's
  filesystem/watch gates and helpers, then #16's
  `windows_sync_pipe_character_and_disk_workers_reuse_classification_without_allocating`.

Auto-merged without conflict and reviewed: `backend/kqueue.rs` (the new
`Ready::vnode` field next to I01's `PollInfo::native`), `backend/epoll.rs`,
`backend/mod.rs`, `backend/unix.rs`, `backend/wasi_p2.rs`, `backend/wasi_p3.rs`,
`backend/web.rs`, `completion.rs`, `DESIGN.md`, and `backend/iocp/mod.rs`, where
#16 made sync workers a per-direction pair and treats `port::STOP` as a wake; the
watch keys stay below `STOP`, so watch completions route as before.

Rule 3 applied (`d0d35b3`):

- `Driver::fs`: once a `Filesystem::Backend` (WASI) backend accepts a request, the
  operation is native (`native = true`, `native_pending += 1`), as I01 does for
  lookups. Handle requests and opens were already native through their
  `Kind::Socket` handle; path requests were not, so a producer posting before every
  turn starved them (the sabotage run spun for 480,525 turns). Terminal completion
  retires the flag through `retire`.
- Native pool requests are **not** native operations, matching I01's definition of
  queued work (blocking-pool results): their results arrive through the work port,
  so a queued turn with only pool requests makes no native call.
  §10 rule 3 now names both cases.
- Regressions: `driver::clock_contract::backend_path_request_is_a_pending_native_operation`
  (synthetic backend: one native step while pending, `os_waits == 0` on queued
  turns, flag retired) and the shared contract `queued_posts_do_not_starve_requests`
  (path phase, open, handle request under a post before every turn; asserts
  `os_waits == 0`, `discovery_polls <= 1`, and zero discovery polls for pool
  requests) run natively and on WASI 0.2/0.3.
- Counters: `filesystem::wait` asserts `os_waits + discovery_polls <= 1`;
  `watch_backpressure` counts a spin only for a turn with neither a blocking wait
  nor a discovery poll and no output, and bounds all turns per 20 ms window.
