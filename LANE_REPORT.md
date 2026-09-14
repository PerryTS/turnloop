# Core lane report

Branch: `lane/core`. Specification: DESIGN.md draft 0.2, read in full with LANES.md.
Only this clone is modified. No publishing or remote operations.

## Handoff status

The core lane's nine implementation steps are present. macOS execution is verified;
Linux is cross-checked only. The other lanes' IOCP/WASI/web adapters are not present
in this clone. There is no Perry integration or performance-comparison claim yet.

**Git delivery is incomplete:** this session's permissions make `.git` read-only.
The requested commits and `trait-v0`/`trait-v1` tags failed with `Operation not
permitted`; escalation is unavailable. External integrator checkpoints appeared
during the work (including `3398983`), but this session did not create them. The
integrator must checkpoint remaining files and tag the current revision-1 trait.
No alternate Git database, permission bypass, push, remote addition or publishing
was performed. Source files were made available directly in this clone throughout.

## Implemented by requested step

1. Three-crate workspace: `windlass`, `windlass-contract`, `windlass-bench`.
   Target-selected `windlass_backend` cfg covers the six platform names. Native
   `Loop` currently selects kqueue/epoll; portable `Driver<B>` is available elsewhere.
   Stable-compatible edition 2024, cgu=1 measurements, shared unsafe lint, unchanged
   dependency soak. Runtime dependency is pinned libc; loom is a model-test dependency.
2. Thoroughly documented backend-neutral unsafe Backend trait, **revision 1** in
   `crates/windlass/src/backend/mod.rs`. Full generational IDs, operation/buffer
   ownership, terminal acknowledgement, cancellation, close-release order, capacity,
   readiness/native/callback completion rules, transfer, injected clock and host
   deadline scheduling. Tags are blocked as described above.
3. Preallocated generational handle/op tables, intrusive per-handle op lists,
   exactly-once terminal completion and Cancelled…Closed ordering, bounded host
   output, O(1) alive counter, ref/unref including queued terminal results, four-ary
   indexed timer heap, BTreeMap comparison, Timeout and bounded turn. Independent
   reserves for native multishot events, repeating timers and posts preserve terminal
   capacity and progress under output backpressure. No host callback runs in turn.
4. Atomic RUNNING/PARKED/NOTIFIED notifier (plus CLOSED lifetime state), a wake syscall
   only on the parked transition, and bounded lock-free per-loop Poster. Five loom
   models exercise the production atomics/queues, including worker result publication.
5. Runnable kqueue: EVFILT_USER, EV_CLEAR readiness cached to EAGAIN, ns waits; TCP
   connect/listen/accept/accept_start, provided/pooled reads/read_start, write/writev,
   shutdown/cancel/close, reuse-port, owning detach/attach; UDP bind/send_to/recv.
6. epoll uses the same Unix engine: EPOLLET, eventfd, construction-time epoll_pwait2
   probe, timerfd fallback and a feature to force that fallback. Cross-Clippy passes;
   Linux runtime behavior remains **UNRUN**.
7. Lazy process-wide bounded blocking pool, default four workers, per-loop completion
   routing, native blocking(f) and getaddrinfo resolution, panic isolation, and started
   job cancellation acknowledged only after the function returns. No driver thread.
8. Reusable Backend-generic contract scenarios plus native integration runners and
   dedicated allocation/lifetime binaries. **27 native tests PASS** (3 pure core,
   21 shared native scenarios, 2 allocation gates, 1 descriptor-lifetime gate).
   Required TCP 1/64 echo, UDP, cancellation/error/close, timing/ref, multi-loop load,
   detach/attach, reuse-port, accept handoff and external-waiter scenarios are covered.
9. Real macOS ri_instructions counters, Linux perf_event_open instructions:u behind
   cfg, and explicitly labelled portable nanoseconds fallback. Five fresh-process
   rounds, rotated workload order, raw JSONL, source SHA-256 and ranges below.

## Final verification matrix

All commands run in this clone. No ignored or filtered tests count toward the 27.
The script exports `RUSTUP_TOOLCHAIN=nightly-2026-08-20`; rows use explicit `+nightly`
spelling for reproducibility. In this table `+nightly` means **`+nightly-2026-08-20`**,
not a moving nightly channel. Earlier verification attempts and failures follow.

| Command | Result |
|---|---|
| `sh scripts/verify-core.sh` | PASS, including both complete native feature configurations and the commands below |
| `cargo +nightly fmt --all -- --check` | PASS; `cargo +nightly fmt --all` also applied |
| `cargo +nightly clippy --workspace --all-targets --all-features -- -D warnings` | PASS macOS |
| `cargo +nightly test --workspace` | PASS, 27 tests, all subjects asserted |
| `cargo +nightly test --workspace --all-features` | PASS, 27 tests; production heap allocation gates plus BTreeMap comparison conformance |
| `RUSTFLAGS='--cfg loom' cargo +nightly test -p windlass models -- --test-threads=1` | PASS, 5 models |
| `RUSTFLAGS='--cfg loom' cargo +nightly clippy -p windlass --all-targets -- -D warnings` | PASS |
| `cargo +nightly clippy --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings` | PASS, includes forced timerfd code; default epoll_pwait2 path also checked in earlier no-feature command |
| `cargo +nightly clippy --workspace --all-targets --target wasm32-unknown-unknown --all-features -- -D warnings` | PASS, common core only |
| `cargo +nightly clippy --workspace --all-targets --target wasm32-wasip2 --all-features -- -D warnings` | PASS, common core only |
| `cargo +nightly clippy --workspace --all-targets --target x86_64-pc-windows-msvc --all-features -- -D warnings` | PASS, common core only |
| `cargo +stable check --workspace --locked` | PASS, installed stable is 1.97.1 |
| `cargo +1.98.0 check --workspace --locked` | PASS; Cargo warns min-publish-age requires the unstable switch. Resolution used pinned nightly under the unchanged soak; this command reused the lockfile |
| `MIRI_SYSROOT="$PWD/target/miri-sysroot" cargo +nightly miri test -p windlass --lib --all-features` | PASS, 3 pure-core tests; native syscalls are not covered by Miri |
| `python3 scripts/benchmark-core.py --rounds 5` | PASS, 15 fresh processes; all operation and byte checks passed; source fingerprint verified |
| `cargo +nightly run --release -p windlass-bench -- --portable --timers` | PASS, nine actual workloads explicitly measured in nanoseconds |
| Cargo.lock audit, unsafe/I/O source audit, workflow YAML parse | PASS: no Tokio; SAFETY comments checked; no library I/O unwrap(); two configured CI jobs |

The allocation gates verified 128,000 bytes over 2,000 provided/pooled transfers,
10,000 batched timer cancellations plus 10,000 Closed results, and 100 warmed accepts:
**zero allocations**. The separate backlog gate delivered all 80 posts and all 16
Cancelled/16 Closed results while preserving zero allocations under backpressure.
Multi-loop load delivered 16,000 uniquely identified posts on four owning threads.
The descriptor test returned to its starting fd count after 32 connected loops,
including stale notifier/poster use. Timer tests assert no early expiry, individual
scheduler bounds, and median lateness below 500 us over twenty 250-us timers.

### UNRUN verification

| Command / verification | Status and reason |
|---|---|
| `cargo +nightly-2026-08-20 test --workspace --target x86_64-unknown-linux-gnu` | UNRUN: no Linux host/container; every Linux instantiation of the shared contracts and allocation/lifetime gates is unrun |
| Same command with `--features windlass/epoll-timerfd` | UNRUN: fallback runtime needs real Linux |
| `cargo +nightly-2026-08-20 run --release -p windlass-bench --target x86_64-unknown-linux-gnu` | UNRUN: Linux perf instruction counter requires Linux and its perf permissions |
| `.github/workflows/core.yml` native macOS/Linux x86_64/Linux arm64 jobs | UNRUN hosted CI: workflow authored locally, not pushed or executed on hosted runners |
| Other-lane Windows/WASI 0.2/WASI 0.3/web backend runtime contracts | UNRUN in this lane: adapters belong to other clones; portable checks do not validate their behavior |
| FreeBSD, iOS, Android native execution; WASI 0.3 compilation | UNRUN: outside local checked-target/runtime coverage |
| DESIGN long-duration soak, exhaustive fault injection, Perry A/B regression budgets | UNRUN: broader integration milestones; current tests are bounded contracts, not those campaigns |
| Requested per-step commits and final report commit | UNRUN after the initial permission failure; `.git` stays read-only and escalation is forbidden |

## Instruction baselines (macOS arm64)

Measured on `macOS-26.5-arm64-arm-64bit-Mach-O`, `arm64`, pinned nightly,
release `codegen-units=1`. Values are min/max **instructions per operation** across
five fresh processes for each configuration. macOS counters are process-reported
`proc_pid_rusage(RUSAGE_INFO_V4).ri_instructions`; they are not labelled Linux
user-only counts. No baseline subtraction or speedup claim is made.

Raw data: [core-macos-arm64.jsonl](benchmarks/core-macos-arm64.jsonl).
Source SHA-256: `a0aa1e9a8e896ecc1df4cdc5b75b64b23c41830c3170519f1bce75825457282b` (manifests, lockfile, Rust sources, toolchain and
Cargo configuration); recomputation matched after final verification.

| Workload | Instructions per operation |
|---|---:|
| `control` | [9.01, 9.04] |
| `idle_turn` | [11,032.67, 11,245.36] |
| `notify_turn` | [11,057.84, 11,196.78] |
| `timer_start_cancel_deliver_close` | [1,957.86, 1,975.50] |
| `timer_start_cancel` | [726.56, 727.89] |
| `tcp_batch_64` | [44,175.37, 46,785.82] |
| `tcp_batch_4096` | [51,332.94, 55,440.99] |
| `accept` | [27,453.22, 27,991.75] |

`notify_turn` is a running notify followed by a nonblocking turn, with zero wake
syscalls asserted. Parked notification is separately exercised by the contract.
`timer_start_cancel` excludes result delivery/close; the lifecycle row includes
both. TCP batches verify their 64/4,096 bytes and both operation completions.
Accept measures 640 server accepts per process, excluding client setup and teardown.
The control performs 1,000,000 iterations; all measured work counters are positive.

| Queue size | Operation | Four-ary heap | BTreeMap |
|---:|---|---:|---:|
| 10 | insert | [66.35, 66.54] | [191.39, 193.11] |
| 10 | cancel | [165.47, 165.59] | [180.37, 180.69] |
| 10 | expire | [217.26, 217.36] | [236.47, 236.47] |
| 1,000 | insert | [104.98, 105.11] | [440.47, 443.25] |
| 1,000 | cancel | [126.80, 127.05] | [465.08, 474.67] |
| 1,000 | expire | [673.36, 676.16] | [398.79, 408.76] |
| 100,000 | insert | [94.83, 95.21] | [627.95, 634.50] |
| 100,000 | cancel | [140.92, 142.37] | [679.00, 687.57] |
| 100,000 | expire | [1,129.94, 1,135.42] | [464.31, 464.36] |

Each queue workload performs 100,000 counted operations per process. BTreeMap stays
as a comparison behind `timer-btree`; production uses the heap in all feature
configurations because the BTree allocation failure is reproducible. These are
initial baselines, not calibrated regression budgets. The earlier noisy control run
is preserved in [core-macos-arm64-initial.jsonl](benchmarks/core-macos-arm64-initial.jsonl).


## Deviations, decisions and proposed specification clarifications

- D3/§15.4: pooled leases remain valid across turns until explicit drop/release.
  Pool exhaustion supplies backpressure. This selects the spec's explicit-release
  option, avoiding safe references being invalidated by a later turn.
- D4/§8: terminal operation credits and references persist until host delivery;
  cancellation results precede Closed, but cancellation IDs need not be ordered
  among themselves. A fired one-shot timer retains its handle until explicit close.
- §5a detach: cancellation may require later native acknowledgement, so detach
  returns WouldBlock until affected completions are delivered. Turn and retry;
  transfer then owns the resource and attach registers it on the destination.
- D5/D7: requesting Integration opts into external parking between turns. Without
  that mode, a running notification uses zero syscalls and cannot make an OS fd
  readable. External hosts combine the fd with next_deadline, as in D7.
- D1/§7 web: Backend is unsafe to express its buffer-quiescence obligation; revision
  1 adds now(), validate_timeout(original timeout) and deadline_changed(). The web
  clock must be host-supplied because std::time::Instant::now is unsupported there.
  No synchronous ready result may bypass web main-thread timeout validation.
- D6: production always uses the indexed four-ary heap. The cfg-selected BTreeMap
  remains a benchmark candidate, not a production feature: warmed 1,000-timer
  batches proved it allocates (1,660 allocations/10,000 transactions). Heap wins
  insert/cancel in measured cases; BTreeMap wins large expiry but fails allocation.
- D8: this scope implements the requested process-shared pool only; first PoolConfig
  wins and conflicting later configuration is rejected. Wasm native blocking/resolve
  return Unsupported. Host async jobs and WASI name-lookup routing need an extension
  when the other lanes integrate; the native closure queue cannot implement those.
- Non-normative §6 API: submission is fallible on fixed-capacity exhaustion; Closed
  and unsolicited posts use op=None; writev owns up to eight inline segments; timer
  reset operates on an active timer, and repeats coalesce missed intervals. Poster
  is bounded lock-free without a FIFO promise. A post wake error with payload=None
  means ownership was accepted, so callers must not retry it.
- The requested scope excludes processes/signals/TTY/protocols. Unix pipe APIs,
  explicit file APIs, optional executor/adapters and Perry ABI changes are not
  implemented by these nine steps; file closures can use the native pool.

## Integrator questions and next steps

1. Preserve the current working files, create the final core commit, and publish the
   local trait revision tag in a session allowed to write Git metadata. The current
   contract is revision 1; earlier trait-v0/v1 attempts created no tags here.
2. Adapt the other backends to unsafe Backend plus the revision-1 clock/scheduler
   hooks; merge their module declarations, dependencies and Platform/Loop aliases.
   Reuse the generic contract functions without weakening assertions. Agree on a
   host-async extension for WASI/web jobs and name lookup.
3. Run both Linux wait paths and Linux perf measurements on real hosts, followed by
   other-lane runtime suites. macOS reuse-port accepted [0, 32]; §5a explicitly does
   not promise macOS balancing. The Linux/FreeBSD test still requires both listeners
   to accept >0; accept handoff verified four workers each echoed four connections.
4. Confirm the documented API clarifications, then integrate Perry's turn/notifier/
   deadline boundary. Collect Perry baselines and set regression budgets; perform
   long soak/fault tests before claiming production coverage. Review rare native
   accept errors and transfer during an unfinished connect during that campaign.

## Historical verification log

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

### Step 1
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
  stable checking. At that checkpoint the native count was 24 tests: 3 pure core, 19 shared scenarios,
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
  the final range is recorded in the baseline table above. No samples
  were discarded or thresholds loosened. Other operation ranges remain visible.
