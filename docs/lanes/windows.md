# Windows lane report

Updated 2026-09-14. Clone: `/Users/amlug/projects/perry/windlass-lanes/windows`.
Branch: `lane/windows`. Development host: macOS arm64.

**Status:** standalone Windows mechanism spikes, compiled backend draft, compio
source evaluation and Windows contract plan are present. Windows cross-checks and
Clippy pass. **All 18 Windows runtime tests are UNRUN.** This session cannot write
`.git`. An external checkpoint commit `2984635` appeared during final review;
four files have final changes after that checkpoint. This is not a shipping core backend.

## Implemented

- Read DESIGN.md draft 0.2 and LANES.md completely. No applicable AGENTS.md found.
  Other lanes were read only via the authorized core trait-tag checks; no other
  clone was modified.
- `spikes/iocp/` is a standalone workspace, not a root workspace member. It has only
  windows-sys 0.61.2 (transitively windows-link 0.2.1); the seven-day soak remains on.
- One IOCP per loop, reserved wake key, stack batches, one bounded GQCSEx wait,
  per-entry status, explicit timeout/APC outcomes and cross-thread wake isolation.
- Both high-resolution timers: SetWaitableTimer APC with alertable GQCSEx, and
  dynamically resolved NtCreate/Associate/CancelWaitCompletionPacket. Relative
  deadlines round up to 100ns units. Tests measure 100 expirations per route and
  check APC cancellation and 100 NT cancel/rearm cycles. No timeBeginPeriod.
- TCP AcceptEx/ConnectEx with provider-specific function lookup, bind and context
  updates, IFS provider check, skip-success notification mode, zero-byte idle
  WSARecv followed by nonblocking recv, WSASend, 128 echo cycles, cancellation
  completion and drain-before-close. Fixture operation storage/events are reused.
- Overlapped named-pipe server/client with both pending and client-first connect
  paths, 128 transfers per path, actual payload assertions, cancel/drain/close.
- Synchronous stdio pipe reader thread posting byte completions and EOF; validates
  the received payload and joins after writer close.
- Opt-in Event helper: sole GQCSEx consumer, bounded preallocated queue, backpressure,
  auto-reset event, partial-drain re-signal, failure forwarding and shutdown. Tests
  forward 1,024 unique packets and a timer through MsgWaitForMultipleObjectsEx;
  a separate test proves the queue reached capacity before shutdown.
- Child process: explicit inherited handle list, three parent-overlapped/child-
  synchronous pipe pairs, suspended creation, Job Object assignment before resume,
  one-shot RegisterWaitForSingleObject and joining UnregisterWaitEx. Tests check
  stdin/stdout/stderr, exit code 23, registration after exit, and termination of a
  demonstrably live child **and grandchild** through the Job Object.
- Console: process-global C/BREAK/CLOSE mapping, active-callback lifetime guard,
  no allocation/host callback in handler. Real C/BREAK events run in a dedicated
  CREATE_NEW_CONSOLE child; CLOSE mapping is checked without triggering OS termination.
- `EVALUATION.md`: source review of compio-driver 0.12.5 at commit
  `1ff9aedeaf69939459fa45aef664d0a7eec856a0`, with source links and archive checksum.
  Recommendation: own the backend and borrow techniques. Public submission
  allocates ThinCell/Box per op, IOCP poll allocates a Vec per call, and cancellation/
  token delivery need adaptation. No performance comparison was run.
- `backend_draft/`: compiled from the standalone crate because no `trait-v0` tag
  appeared. Preallocated, separate kernel/metadata slabs; generational IDs; provided
  socket/pipe read/write; internal zero-byte read stage; pipe connect; ready queue;
  cancel-before-Closed under output backpressure; one-wait turns; APC deadline;
  parked notifier; opt-in Event. Tests count allocations over 256 warm transfers,
  notifier syscalls, cancellation ordering, and event wake before the first turn.
- `CONTRACT_TEST_PLAN.md`: common contracts, Windows variants and precision bounds;
  distinguishes implemented probes from planned core tests. `README.md` maps all
  ten Cargo test binaries. A separate handle-count test verifies 128 port cycles.
- Every unsafe block has a SAFETY comment and each Rust crate root denies
  unsafe_op_in_unsafe_fn. The stronger Clippy unsafe-comment gate passes.

## Verification commands and results

The commands below are the complete build/lint/source verification ledger. Repeated
successful runs of the same command occurred at each described implementation
boundary; failures and their diagnostics are recorded separately below. Cargo test
was deliberately not run on macOS to avoid calling cfg-excluded tests coverage.

| Command | Result |
| --- | --- |
| `rustc --version` | PASS: 1.100.0-nightly, pinned nightly-2026-08-20 (compiler date 2026-08-19) |
| `cargo +stable --version` | PASS: cargo 1.97.1, not the 1.98 stated in the task environment |
| `rustc +stable --version` | PASS: rustc 1.97.1 (2026-07-14); this is the stable toolchain actually checked |
| `git -C ../core tag -l 'trait-v*'` | PASS, repeatedly: no tags initially, during spikes, after evaluation, draft and final review |
| `cargo fmt --manifest-path spikes/iocp/Cargo.toml` | PASS after each code step and final changes |
| `cargo fmt --manifest-path spikes/iocp/Cargo.toml -- --check` | PASS after evaluation and final validation |
| `cargo check --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc` | PASS initial port and final full spike/draft, including all test targets |
| `cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc -- -D warnings` | PASS after port fix, timers, TCP/pipes/stdio, helper, process fix, console, evaluation and draft fixes; intermediate failures below |
| `cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS final full spike/draft; initial comment-placement failures below |
| `cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc` | PASS after port, timers, TCP/pipes/stdio, process, console, draft and final review |
| `cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS macOS harness compilation; Windows code excluded |
| `cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets` | PASS after console and final review; Windows code excluded |
| `cargo tree --manifest-path spikes/iocp/Cargo.toml --target x86_64-pc-windows-msvc` | PASS: windows-sys -> windows-link only; no tokio or compio |
| `rg -n 'unwrap\(' spikes/iocp/src spikes/iocp/tests spikes/iocp/backend_draft` | PASS: no matches |
| Python source inventory / `.unwrap()` / owned-Markdown whitespace assertions | PASS: 23 Rust files, 3,368 lines at that check, 18 tests; no unwrap or trailing whitespace |
| `git diff --check` | PASS; initially new files were untracked, so separate whitespace/fmt checks also covered them |
| `git status --short` / `git status --short --branch` | PASS: initially only new lane files; after external checkpoint, four modified lane-owned files. No core/out-of-scope edits |
| `git log -4 --format='%h %s'` / `git diff --stat` | PASS: external checkpoint 2984635 and four subsequent modified files identified |

Source research used Microsoft Learn API pages for the Win32 calls (subtle semantics
are linked in code), the official compio crate archive and its pinned repository
source, and thin-cell 0.2.1 source. Initial docs URLs for CreateNamedPipeW and
UnregisterWaitEx failed; resolved to the current namedpipeapi/threadpoollegacyapiset
pages. Initial docs.rs/GitHub web retrieval failed; source was obtained from official
crate archives instead. No research dependency was added to Cargo.toml.

### Intermediate failed checks, fixed without weakening gates

- First Windows Clippy: `not_unsafe_ptr_arg_deref` on `owned(HANDLE)`. Made it unsafe
  with an explicit ownership contract and updated its callers. Subsequent PASS.
- TCP-only intermediate Clippy: `Pipe` variant and `Operation::data` unused.
  Completed planned pipe implementation; no lint suppression. Subsequent PASS.
- Initial process check through Clippy: E0308 callback expected
  `unsafe extern "system" fn(_, bool)`, found `fn(_, u8)`. Corrected to windows-sys's
  WAITORTIMERCALLBACK ABI. Subsequent PASS.
- Draft update Clippy: `collapsible_if` around timer-arm error handling. Collapsed
  the condition. Subsequent PASS.
- Strong unsafe-comment gate: first `src/process.rs:307`, then
  `src/bin/console_child.rs:11` and `tests/integration.rs:99` reported
  `unsafe block missing a safety comment`. Existing comments sat above assert
  macros; placed SAFETY comments immediately before the unsafe expressions.
  Subsequent PASS, including after the kernel/metadata storage separation.

### UNRUN runtime commands and gates

Every test below requires a Windows host. No Windows runner/link-and-execute setup
is available, so no binary was executed and no Windows precision, allocation,
completion, syscall, leak or process behavior has been reported as passing.

- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --target x86_64-pc-windows-msvc`.
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --all-targets -- --nocapture --test-threads=1` on Windows.
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test port` (1 test).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test timer -- --nocapture --test-threads=1` (3 tests).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test tcp` (1 test).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test pipe` (2 tests).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test stdio` (1 test).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test integration` (2 tests).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test process` (3 tests).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test console` (1 test).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test draft -- --nocapture --test-threads=1` (3 tests).
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --test handles` (1 test).
- UNRUN Windows-native `cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets -- -D warnings` and `cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets`; equivalent Windows cross-target checks PASS above.
- UNRUN windlass-contract, ETW, QueryProcessCycleTime comparisons, soak, full handle/
  thread leak suite, Windows minimum-version/VM precision matrix: require Windows
  and/or the core integration. Planned tests are not implemented coverage.

## Commit / integration blocker

FAIL:

```text
git add spikes/iocp LANE_REPORT.md && git commit -m 'Add standalone IOCP wait and wake probe'
fatal: Unable to create '/Users/amlug/projects/perry/windlass-lanes/windows/.git/index.lock': Operation not permitted
```

The session explicitly mounts `.git` read-only and disables escalation. No alternate
Git directory, permission bypass or outside-clone write was attempted. Subsequent
requested commit boundaries (timer, TCP/pipes/stdio, helper/process/console,
evaluation, backend draft, contract plan) are **UNRUN** for this same blocker.
The integrator or another external process subsequently created checkpoint
`2984635` (`checkpoint(windows): Codex lane work in progress (32 files)`). This
session did not author that commit. Final `LANE_REPORT.md`,
`spikes/iocp/backend_draft/{mod.rs,README.md}` and `spikes/iocp/tests/draft.rs`
remain modified after it and need a final commit. The metadata permission blocker
for this session remains unchanged.

No `trait-v0` appeared, so the authorized §6 fallback was used. `git fetch ../core
lane/core --tags` and merge are **UNRUN** (tag absent; `.git` would also prevent
fetch/merge). No files under `crates/` or other lanes were modified.

## Deviations and proposed specification changes

1. **APC / D1:** alertable GQCSEx can execute any queued user APC, including one
   installed by the host. The timer's own APC only increments an internal counter.
   Either qualify D1 for alertable waits or prefer NT packets with explicit support
   policy. APCs target the arming thread, so they cannot directly wake a different
   helper thread.
2. **NT packet support:** Microsoft documents the Nt* APIs but lists no minimum OS
   and supplies no SDK header/import library. Feature-detect exports. STATUS_PENDING
   on cancellation is not permission to reuse the packet; drain it first. Both
   timer mechanisms require actual Windows measurements before selecting a default.
3. **Transfer:** IOCP association lasts until handle close and is not removed by
   duplication. Generic detach/attach cannot simply reassociate; specify socket
   reconstruction/routing and a named-pipe transfer policy. No unsupported transfer
   is silently claimed here.
4. **Child stdio:** parent pipe ends should be overlapped; ordinary child's ends
   should be synchronous. Clarify §7.3's wording accordingly.
5. **Console close:** CTRL_CLOSE -> HUP is best-effort. Windows terminates after
   handler return/timeout, so deferred turn-based host cleanup is not guaranteed.
   No SIGTERM console equivalent. C/BREAK preserve normal pull-based dispatch.
6. **Draft boundary:** §6 has no internal trait. The compiled draft is not a full
   Loop: timer heap, liveness counters, pooled leases, multishot operations and
   process-wide routing belong to core. AcceptEx/ConnectEx/process/stdio/console
   mechanisms remain separate spike modules pending adaptation; exact steps are
   listed in `backend_draft/README.md`. No production backend is claimed.
7. **Queues / complexity:** helper and synchronous-reader probes use bounded mutex
   queues, not the specified core lock-free Poster. Draft uses linear slab lookup
   and liveness scans. Replace these at core integration; steady-state read/write
   allocation is source-designed and has an UNRUN counter gate, not a measured claim.
8. **Drop:** cancellation is not completion. Teardown may block until kernel buffers
   are released; draft aborts on an unrecoverably broken port while draining. Define
   production driver-failure/shutdown policy rather than returning with live I/O.
9. **Precision:** functional spike gate is no early expiry, p95 lateness <5ms and
   max <100ms over 100 samples at 250us. This does not demonstrate sub-ms behavior.
   Proposed dedicated-host target p50 <500us / p95 <1ms awaits measurement.

## Open questions for the integrator

- Commit the final four-file delta after external checkpoint 2984635; current
  session filesystem permissions prevent the requested commit workflow.
- Publish trait-v0; adapt types/storage/hooks according to backend_draft/README.md.
  Confirm ownership of secondary accept sockets and cancelled buffers.
- Decide APC caveat vs NT packet default, minimum Windows version/support policy,
  detach/attach strategy, pooled lease lifetime and fatal shutdown behavior.
- Core notification must remain logically parked while the GUI owns its wait;
  integrate that state with core's loom-checked handshake.
- Confirm stable compiler version: actual +stable is 1.97.1, which passes; 1.98 itself
  was not selected or installed by this lane.

## Next steps

1. Run all 18 tests on Windows with --nocapture --test-threads=1, retaining full output
   and precision distributions. Address runtime failures before production claims.
2. Integrate the draft and separate mechanisms once core's trait is tagged; replace
   draft IDs, completion queues and liveness with core equivalents. Add Windows
   contract variants from CONTRACT_TEST_PLAN.md to the core-owned suite.
3. Run Windows allocation/ETW/cycle/soak gates and full handle/thread leak coverage.
   Console input/resize/VT modes, UDP, transfer and service fan-out remain planned
   contract/integration work beyond the explicitly enumerated mechanism probes.
4. Commit the requested checkpoints when Git metadata permissions allow. No pushes,
   remotes, repositories, publication or toolchain/soak overrides were performed.
