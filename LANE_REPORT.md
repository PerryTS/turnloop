# Core lane report

Branch: `lane/core`. Specification: DESIGN.md draft 0.2, read in full with LANES.md.
Only this clone is modified. No publishing or remote operations.

## Implemented

- Step 1: three-crate workspace, stable Rust edition 2024, target-selected backend cfg,
  release/bench codegen-units=1, shared unsafe lint. Dependency soak remains unchanged.
- Steps 2–9: in progress.

## Verification log

Commands below are run in this clone. PASS means the command exited successfully;
cross-checking is not execution of target tests.

- FAIL: `rustc --version` through rust-toolchain.toml auto-sync: rustup reported
  `failed to install component: 'rust-std-wasm32-unknown-unknown', detected conflict`
  for `libaddr2line-f2b6e6cbcc23e6ae.rlib`. Existing explicit pinned toolchain works.
- PASS: `cargo +nightly-2026-08-20 --version`: 1.100.0-nightly.
- PASS: `cargo +stable --version`: 1.97.1 (environment differs from advertised stable
  1.98; a separate `1.98.0` toolchain is installed).
- PASS: `rustup target list --installed --toolchain nightly-2026-08-20`: includes
  macOS arm64 and Linux/Windows/Wasm targets required by the task.

## Deviations and proposed spec changes

None yet. Public API sketch in §6 is non-normative; contract details will be documented.

## Integrator questions

None yet.

## Next steps

Publish trait-v0; implement and verify steps 3–9 in order. Linux runtime tests will
be explicitly marked UNRUN because there is no Linux host.
- PASS (step 1): `cargo +nightly-2026-08-20 fmt --all`.
- PASS (step 1): `cargo +nightly-2026-08-20 clippy --workspace --all-targets -- -D warnings`.
- PASS (step 1): `cargo +stable check --workspace`.

### Step 2: backend contract

- Implemented `backend/mod.rs` revision 0 with backend-neutral Request/Event,
  generational identities scoped to a loop, buffer contracts, capacity limits,
  native cancellation acknowledgement, close-release order, and detach/attach.
- Fixed capacities make overload explicit instead of allocating on operation paths.
- Proposed D3 clarification: leases own their pool slot until explicitly released
  (drop); safe references cannot be invalidated by a subsequent turn. Pool exhaustion
  applies backpressure. This selects the explicit-release option in §15.4.
- Proposed §5a clarification: detach cancels immediately but returns WouldBlock
  until native cancellation acknowledgements have been delivered. Caller turns and
  retries. A synchronous always-successful detach cannot honor IOCP buffer lifetimes.
- Proposed D5/D7 clarification: obtaining Integration opts into external parking;
  the notifier stays PARKED between turns in this mode, so external waiters wake.
  Otherwise RUNNING notification has no syscall and cannot make an OS fd readable.
- FAIL: `git add Cargo.toml Cargo.lock crates LANE_REPORT.md && git commit -m
  'Create the windlass workspace and target selection'`: `.git/index.lock` creation
  rejected with Operation not permitted. The session marks `.git` read-only and
  forbids escalation. Commits and trait-v0 tag are BLOCKED, not created. Files are
  available in the working tree for the other lanes; no alternate git database or
  permission workaround is used.

### Core and notifier progress (steps 3–4)

- Implemented preallocated generational handle/op tables with generation retirement,
  cross-loop identity checks, exactly-once terminal retirement, output backpressure,
  Cancelled…Closed ordering, O(1) liveness, timer reset/repeat, and bounded turn.
- Implemented indexed four-ary heap and feature-selected BTreeMap comparison;
  deterministic 1,000-timer cancellation/expiry test verifies all 666 survivors.
- Implemented atomic notifier and a bounded per-loop preallocated posting queue;
  post capacity is rounded up to a power of two. Full/closed posting returns ownership.
- Added cfg(loom) tests of parking races, zero-syscall running notifications and
  two-producer queue delivery. Native tests are still to come with kqueue.
- API clarifications: fallible timer submission (capacity exhaustion); completion
  op is optional for Closed/posts; writev accepts an inline owning WriteVectored
  with at most 8 segments to avoid per-operation descriptor allocations.
- FAIL then PASS (step 2): pinned Clippy initially rejected the 16-segment inline
  writev enum as oversized; reduced the documented inline limit to 8. No lint
  suppression or heap indirection. Formatting, workspace Clippy and stable check passed.
- FAIL then PASS (step 3): pinned Clippy found deprecated AtomicU64::fetch_update
  and a collapsible if; replaced with stable compare-exchange and simplified control
  flow. `cargo +nightly-2026-08-20 fmt --all`,
  `cargo +nightly-2026-08-20 clippy --workspace --all-targets -- -D warnings`,
  `cargo +nightly-2026-08-20 test --workspace` (2 tests),
  `cargo +nightly-2026-08-20 test -p windlass --features timer-btree` (2 tests), and
  `cargo +stable check --workspace` all PASS.
- FAIL then PASS (step 4 initial Clippy): collapsible if in notifier corrected.
- UNRUN: step 2/3/4 commits; `.git` remains read-only. `git tag trait-v0` was also
  attempted and failed to create the ref lock (Operation not permitted).

### Unix and pool progress (steps 5–7)

- kqueue: EVFILT_USER wake, EV_CLEAR read/write, nanosecond timespec waits;
  TCP connect/listen/accept and accept_start, provided/pooled reads and read_start,
  full-buffer writes/writev, shutdown, cancellation, close, SO_REUSEPORT, owning
  detach/attach, and UDP bind/send_to/recv. Pending operations use intrusive queues.
- epoll: the same Unix engine, eventfd wake, epoll_pwait2 probe at construction,
  timerfd fallback, and `epoll-timerfd` feature to force the fallback in Linux CI.
- Pool: lazy process-wide bounded queue with 4 workers by default; first submission
  fixes configuration, later mismatches return InvalidInput. Host jobs and native
  getaddrinfo resolution deliver through a dedicated per-loop completion queue.
  Cancelling a started job waits for its real return before the terminal Cancelled.
  Panics are caught so a host job cannot kill a pool worker.
- Posting queue refined to a bounded lock-free slot queue: a paused producer cannot
  block consumption of other slots. No cross-producer FIFO ordering is promised.
  Wake failure after an accepted post returns an error with payload=None (do not retry).
- PASS: step 4 initial loom command
  `RUSTFLAGS='--cfg loom' cargo +nightly-2026-08-20 test -p windlass models -- --test-threads=1`
  (3 models); subsequent queue revision is being rechecked.
- FAIL then PASS (step 5): public associated Wake initially leaked private Poller
  types (E0446); now names the public native wake type directly.
- PASS (step 5): formatting, native workspace Clippy, stable workspace check.
- PASS: `cargo +nightly-2026-08-20 test --workspace`: 2 core tests and 5 native
  contracts (bounded wait, parked/running notify, byte-verified echo at 1 and 64 TCP
  connections). The waits, wake syscall count, accepts, writes and returned bytes
  are explicitly asserted.
- PASS (step 6): `cargo +nightly-2026-08-20 fmt --all` and native workspace Clippy;
  `cargo +nightly-2026-08-20 clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`;
  `cargo +nightly-2026-08-20 check --workspace --target x86_64-unknown-linux-gnu --features windlass/epoll-timerfd`;
  `cargo +stable check --workspace`.
- UNRUN: `cargo test --workspace --target x86_64-unknown-linux-gnu` and forced timerfd
  runtime variant: no Linux machine or container. Cross-check success is not runtime proof.
- UNRUN: step 5/6 commits, blocked by the same explicit read-only `.git` permission.
- Reference checks: Apple kevent manual and Linux man-pages epoll/timerfd documentation;
  local libc 0.2.175 bindings used for ABI details. No new runtime dependency besides libc.

### Contract and measurement progress (steps 8–9)

- PASS: `cargo +nightly-2026-08-20 test --workspace -- --nocapture`: 2 core tests,
  18 native contracts and the dedicated allocation binary. Includes external kqueue
  nesting/wake, cancellation with output capacity 1, stale/foreign identifiers,
  capacity backpressure, 512 KiB vectored write + multishot read + shutdown/EOF,
  all required TCP/UDP/multi-loop/transfer/accept scenarios, pool cancellation and DNS.
- PASS: allocation counter measured 2,000 provided/pooled 64-byte transfers with
  timer start/cancel/close (128,000 bytes) and 100 warmed accepts: zero allocations.
- macOS reuse-port observation: [0, 32] accepts. This tests shared binding and all
  accepts; macOS balancing is explicitly absent in §5a. Linux/FreeBSD additionally
  assert both listeners accept >0. Four handoff workers each echoed 4 connections.
- FAIL then PASS: external waiter initially used assert_eq on a packed kevent field
  (E0793); changed to an aligned local copy. Assertion preserved.
- PASS: final queue revision and additional production WorkPort publication model:
  `RUSTFLAGS='--cfg loom' cargo +nightly-2026-08-20 test -p windlass models -- --test-threads=1`
  (5 models). Native Clippy with all features also passes.
- PASS: `cargo +nightly-2026-08-20 clippy --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings`.
- PASS: `cargo +stable check --workspace` (installed stable 1.97.1), and
  `cargo +1.98.0 check --workspace`. Rust 1.98 Cargo warns that min-publish-age is
  ignored without the unstable switch; dependency resolution was already performed
  by the pinned nightly under the soak and the lockfile is preserved.
- PASS: `cargo +nightly-2026-08-20 test -p windlass --features timer-btree` (2 tests).
- PASS: `cargo +nightly-2026-08-20 run --release -p windlass-bench` and the two timer
  runs with `-- --timers`, default and `--features timer-btree`: macOS reports actual
  ri_instructions, and all benchmark work counters and byte checks passed.
- Implemented instruction measurement via proc_pid_rusage/RUSAGE_INFO_V4 on macOS,
  perf_event_open instructions:u on Linux, and an explicitly labelled elapsed-time
  fallback. Bench release profile uses codegen-units=1. Reproducible fresh-process
  interleaving script and raw JSONL measurements are under scripts/ and benchmarks/.
- Performance refinement: core close/detach now traverse an intrusive per-handle
  operation list instead of scanning all configured operation slots; tests and
  allocation gate remained green after this change.
- An externally created checkpoint appeared at `a1556bb` (`checkpoint(core): Codex
  lane work in progress (28 files)`). It was not created by this session. Current
  later work remains uncommitted and no trait tag exists. No remotes were added by
  this session (an origin/main reference also appeared externally).

### Trait revision 1: host clock and callback scheduling

- Found and fixed a core integration issue: std::time::Instant::now is unsupported
  on browser Wasm. Backend now supplies `now()`. Native Instant remains an alias to
  std::time::Instant; web Instant is a monotonic Duration timestamp constructed by
  the host adapter. `Driver::now()` constructs portable deadlines.
- Added default `validate_timeout` and `deadline_changed` hooks. Web validates the
  original timeout even when queued completions avoid polling, and arms one host
  callback for the earliest core timer. Without the deadline hook, a browser could
  never schedule an otherwise idle core timer. Native defaults are no-ops.
- Backend is now an **unsafe trait**: implementers explicitly acknowledge the
  buffer-quiescence guarantee that core safety relies on. Unix's implementation
  includes its SAFETY rationale. Other lanes must change to `unsafe impl Backend`,
  implement `now`, and supply host timeout/deadline hooks where applicable.
- No callback backend file or other lane clone was modified. The generic contract
  scenarios now use Driver::now for deadlines. A deterministic host-like core test
  checks timer arming/disarming, expiry on the injected clock, and queued-work
  timeout validation.
- PASS: `cargo +nightly-2026-08-20 check --workspace --target wasm32-unknown-unknown`;
  initially warned about unused native pool fields on Wasm, subsequently cfg-gated.
- FAIL: `git tag trait-v1`: cannot create `.git/refs/tags/trait-v1.lock` (Operation
  not permitted). Revision 1 exists in the working tree, NOT as a git tag.
- Added a retained-lease/backpressure contract and a separate fd-lifetime test
  process (32 connected loops, including stale Notifier/Poster use after drop).
- FAIL: first final verification attempt found Clippy suspicious_map in fd_count;
  replaced map+count with inspect+count, preserving the per-entry success assertion.

### Final validation refinements

- PASS: `sh scripts/verify-core.sh`, including all native tests, five loom models,
  both timer comparison configurations, Linux all-feature cross-Clippy and locked
  stable checking. Final native count is 24 tests: 3 pure core, 19 shared scenarios,
  1 allocation gate, 1 fd-lifetime gate.
- PASS: Clippy `--workspace --all-targets` for wasm32-unknown-unknown,
  wasm32-wasip2 and x86_64-pc-windows-msvc, both ordinary and all-feature builds.
  These check the common core/contract/benchmark code, NOT the other lanes' adapters.
- PASS: `RUSTFLAGS='--cfg loom' cargo +nightly-2026-08-20 clippy -p windlass --all-targets -- -D warnings`.
- PASS: `cargo +1.98.0 check --workspace --locked` (same expected Cargo soak warning).
- PASS: `cargo +nightly-2026-08-20 run --release -p windlass-bench -- --portable --timers`:
  all nine workloads ran, explicitly reported in nanoseconds rather than instructions.
- PASS: `rustup component add miri --toolchain nightly-2026-08-20`;
  `MIRI_SYSROOT="$PWD/target/miri-sysroot" cargo +nightly-2026-08-20 miri setup`;
  `MIRI_SYSROOT="$PWD/target/miri-sysroot" cargo +nightly-2026-08-20 miri test -p windlass --lib`
  (3 tests). Sysroot cache stayed under this clone; toolchain/cache writes used the
  explicitly writable Rust directories. No Homebrew or other lane files changed.
- PASS: workflow YAML parsing with Ruby YAML.load_file and two-job assertion;
  Python Cargo.lock audit found no tokio/tokio-* package; source inspection found no
  unwrap() in library I/O and checked SAFETY comments at the unsafe operations.
- Timer precision gate strengthened: 20 additional 250-us timers, median lateness
  below 500 us, plus the existing no-early-fire and individual scheduler bounds.
  `cargo +nightly-2026-08-20 test -p windlass-contract native::timer_bounds -- --nocapture`
  PASS, and the subsequent complete suite PASS.
- Close ordering test permits arbitrary order among cancellation IDs, while still
  asserting both unique IDs precede Closed. This is required for native completion
  backends, which need not preserve cancellation order.
- The initial allocation gate also PASSed with BTreeMap (one timer reuses its root).
  Strengthening the unchanged zero-allocation assertion to ten warmed batches of
  1,000 timers exposed the actual allocation cost:
  **FAIL:** `cargo +nightly-2026-08-20 test -p windlass-contract --test allocations --features windlass/timer-btree`:
  `steady batches of 1000 timers allocate nothing`, `left: 1660`, `right: 0`.
  This disqualifies BTreeMap as the production driver timer structure.
- Final implementation always uses the indexed heap in Driver. `timer-btree` selects
  the separate benchmark queue alias and its real conformance test, so both timer
  candidates remain measurable without changing production semantics. The widened
  allocation assertion remains in place. **PASS:** `cargo +nightly-2026-08-20 test
  --workspace --all-features` and `cargo +nightly-2026-08-20 test --workspace`, both
  including 10,000 batched timers, 128,000 bytes and 100 accepts with zero allocations.
- FAIL then PASS during timer extraction: all-feature Clippy found unused heap len/
  is_empty methods when the benchmark alias selected BTreeMap; exported the concrete
  heap alongside the comparison alias. No lint suppression.
- PASS: `python3 scripts/benchmark-core.py --rounds 5`. This builds two cgu=1 binaries
  with `cargo +nightly-2026-08-20 build --release -p windlass-bench --target-dir
  target/bench-{heap,btree}` (BTree adds `--features timer-btree`) and runs 15 fresh
  processes in rotated order. Final source fingerprint is in the raw JSONL metadata.
- Earlier 10k-iteration control showed noise ([9.59, 11.86] instructions/iteration).
  Preserved those results in `benchmarks/core-macos-arm64-initial.jsonl`. The final
  control does 1,000,000 actual iterations to amortize counter/page-fault overhead;
  it is stable at 9.01 instructions/iteration at the reported precision. No samples
  were discarded or thresholds loosened. Other operation ranges remain visible.
