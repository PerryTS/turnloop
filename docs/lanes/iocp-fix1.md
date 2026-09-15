# IOCP fix1 lane report

Base: PR #5 `windows/iocp-backend`, `28a49c8`. Development host: macOS arm64.
Status: requested fixes and eleven new Windows regressions/allocation tests
implemented; local verification complete. **All new Windows runtime tests are UNRUN (no Windows host).** The
integrator's green run 34899966713 concerns the base, not this working tree.
No commits or pushes are made here; `.git` is read-only.

## Findings, fixes and tests

1. **Batch argument injection — confirmed.** `program()` returned explicit/PATH
   candidates without checking the extension before MSVCRT quoting/CreateProcessW.
   Resolved `.bat`/`.cmd` extensions now reject with InvalidInput, ignoring ASCII
   case, before stdio or process setup. `batch_programs_are_rejected_before_spawn`
   tests eight explicit/PATH cases, exact error, empty loop/no child completions,
   and absent marker. Explicit cmd.exe execution of each same fixture is the
   positive control and must create the exact marker bytes. PATH cases use bare
   filenames including the extension; no PATHEXT search behavior was added.
2. **Exit/kill/close race — confirmed.** Termination errors previously escaped
   whenever the callback had not published readiness. ERROR_ACCESS_DENIED plus a
   signaled owned process handle now records readiness and returns NotFound from
   kill; close accepts that result. Other errors retain their original identity.
   Windows lifetime tests require 200 process-kill + 200 job-kill/close cycles,
   exact Cancelled/Closed order under capacity-one output, no duplicates, and a
   signaled duplicated process handle at Closed. A natural-exit test waits on a
   duplicated raw handle before any turn. A backend unit test unregisters the
   callback while the child is suspended, then resumes/waits: both kill and close
   must take the callback-lag path with exit code 0, independent of scheduling.
   That unit fixture lists its own executable without running nested tests; the
   integration fixture separately exercises native_child exit code 23.
3. **Discarded completion batch — confirmed.** Entry processing now evaluates
   every entry, retains the first error, and returns it after the batch. poll also
   applies dequeued entries before propagating timer cancellation errors. The
   synthetic batch test places an invalid packet between two valid writes and
   checks both exact OpIds/byte counts/terminal completions under capacity-one
   output. The timer fault test injects a cancellation failure during poll and
   verifies dequeued I/O remains deliverable. Synthetic fixtures discard their
   unsubmitted metadata on unwind, so a failed assertion cannot hang teardown.
4. **STATUS_PENDING on timed waits — confirmed against timer.rs and Microsoft's
   documented contract.** cancel leaves the packet active on 0x103. Both wait and
   Event paths now retain pending state and defer rearm/cancel until the matching
   generation is dequeued. Stale generations do not clear it. The deterministic
   test substitutes NT association/cancellation functions, counts actual cancel
   calls, and uses real port packets to force wake → pending → stale generation →
   matching TIMER → successful next-generation reuse. It checks one wait and the
   private-timer empty-wait accounting. Existing deadline/allocation/no-spin gates
   remain unchanged; the mock test is not proof of NT runtime timing.
5. **Cancelled child watch stalled — confirmed.** Standalone cancellation is
   ready for every service kind and returns terminal Cancelled before status is
   queried. Close-induced cancellation retains the native exit watch until child
   termination, preserving the existing Cancelled-before-Closed/reap contract.
   The lifetime regression cancels via detach (Process exposes no exit OpId),
   requires exactly one terminal Cancelled in one turn with zero OS waits, and
   checks the raw process handle is still unsignaled. It then kills/waits/closes
   and checks no duplicate exit or cancellation. A Windows counting-allocator
   gate covers eight live-watch cancellations, eight exits and sixteen closes,
   exact identities and zero allocations after setup.
6. **Racy exact handle baseline — confirmed.** Every test in windows_lifetimes.rs
   holds one static mutex until its resources drop. The exact handle-count
   comparison remains intact; default parallel libtest execution can no longer
   interleave these tests' handle creation with that baseline.
7. **Quick wins — all confirmed.** Loopback SIO_TCP_INITIAL_RTO is best-effort;
   a unit test forces WSAENOTSOCK and checks the failing call ran. Signal tickets
   use Box::into_raw/from_raw, with reclamation after removing the published slot
   and joining dispatchers; a moved-subscription regression asserts 200 deliveries
   and no delivery after drop. Each of 128 synchronous-read cancel/close/drop
   rounds now writes new bytes after quiescence, reads them with fresh ownership,
   and checks both the payload and the original untouched buffer. CONTRIBUTING
   and revision-2 docs describe the production IOCP provider and required positive
   contract counts. DESIGN §7.3 now states §15 q3's existing NT packet decision.

## Verification ledger

Initial source inspection PASS: complete DESIGN, CONTRIBUTING, integration report,
relevant Windows/core lane reports, revision-2 contract and every IOCP source file;
no applicable AGENTS.md; initial working tree clean at the requested base.
Read-only inventory used rg/cat/sed, git status/log, rustup toolchain list and
source diff review. Two guessed source filenames were absent; subsequent reads
used the actual types.rs/driver.rs/native.rs files.

- PASS `cargo fmt --all` after each edit group.
- PASS initial `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`.
- FAIL intermediate same Windows command: vec_init_then_push in the new moved-ticket
  test. Fixed by constructing the initial vector directly and asserting capacity
  growth; no lint suppression. Subsequent identical command PASS.

The native runner passed all three modes: **236 / 243 / 276 workspace tests**,
with **49 / 56 / 56** independently executed contract tests. Total **1,297 passes**
includes package repetitions. The unchanged native allocation and no-spin gates
ran in every applicable mode. Ignored server and cfg-excluded Windows tests are
UNRUN, never included in those counts. [Every nested native test command and its
positive count](../iocp-fix1-commands.md) is recorded separately.

Final Windows all-target/all-feature Clippy includes all eleven new tests. Native
Clippy passes with default and all features; WASI 0.2, experimental 0.3 and web core
all-target/all-feature Clippy pass. Stable is 1.97.1. The p3 compiler reports only
inherited Cargo manifest/config warnings; strict Rust Clippy still passes.
No-tokio passes eight targets plus their union in default/all-feature modes. Soak
passes all 251 locked versions with only the pre-existing exact rustls exception.

Final static audit PASS: all eight tests in windows_lifetimes.rs acquire the mutex;
no new unwrap(); protected dependency/CI/soak files unchanged; new report whitespace
checked. One multi-file edit was rejected for a context mismatch without changing
the tree, then reapplied against the actual lines. Full local command logs and the
machine-readable ledger are under `.tools/iocp-fix1/`.

### Exact verification commands

Commands below are the recorded invocations; repeated Windows rows retain the
intermediate failure and the final passes. Formatting was also applied with
`cargo fmt --all` after each edit group (PASS).

| Invocation | Result | Command |
| --- | --- | --- |
| native-clippy | PASS | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| windows-clippy | FAIL | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| windows-clippy-fixed | PASS | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| no-tokio | PASS | `bash scripts/ci/no-tokio.sh` |
| stable | PASS | `cargo +stable check --locked --workspace --all-targets --all-features` |
| soak | PASS | `python3 scripts/ci/soak.py` |
| wasi-p2 | PASS | `cargo clippy --locked --target wasm32-wasip2 -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasi-p3 | PASS | `cargo +nightly-2026-09-07 clippy --locked --target wasm32-wasip3 -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| paths | PASS | `python3 scripts/ci/check-paths.py` |
| feature-modes | PASS | `python3 scripts/ci/feature_modes.py` |
| fmt | PASS | `cargo fmt --all --check` |
| diff-whitespace | PASS | `git diff --check` |
| native-tests | PASS | `python3 scripts/ci/run-tests.py native` |
| windows-final | PASS | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| native-default-final | PASS | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| web | PASS | `cargo clippy --locked --target wasm32-unknown-unknown -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-final | PASS | `cargo fmt --all --check` |
| stable-version | PASS | `rustc +stable --version` |

## Deviations, design clarifications and open questions

- No dependencies, manifests, lockfile, seven-day soak settings, CI requirements,
  zero-tokio policy, allocation thresholds or no-spin assertions were changed.
- DESIGN §7.3 only reconciles the already-decided timer mechanism. Recorded 275.2 us
  is **lateness**, not total elapsed time: spikes/iocp/tests/timer.rs subtracts the
  requested delay before reporting, matching WINDOWS_RESULTS.md and §15 q3.
- Standalone Windows exit-watch cancellation intentionally differs from Unix's
  retained-until-reap observation policy. A separate closing flag preserves the
  existing asynchronous close behavior while a watch is pending. If the watch has
  already completed cancellation, the revision-2 API has no separate teardown
  operation to delay Closed: existing Child::Drop synchronizes termination on
  release. Tests explicitly wait on the owned process before closing in this case.
  Completion-independent asynchronous process teardown needs a contract decision;
  it is not silently claimed as covered by these fixes.
- Native references checked: [TerminateProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess)
  documents asynchronous termination and ERROR_ACCESS_DENIED after exit;
  [NtCancelWaitCompletionPacket](https://learn.microsoft.com/en-us/windows/win32/devnotes/ntcancelwaitcompletionpacket)
  documents STATUS_PENDING prohibiting immediate packet reuse.

## UNRUN and next steps

- Integrator: commit/push the coherent tree and relay windows-2025 default,
  executor and all-features CI. New Windows unit/contract/allocation/handle tests
  remain UNRUN until those results arrive. Cross-Clippy is compilation only.
- Run `cargo test --locked -p turnloop --lib backend::iocp -- --nocapture` and
  `cargo test --locked -p turnloop-contract --test windows_lifetimes -- --nocapture`
  on Windows. Run the latter with default parallelism to exercise mutex isolation.
- Run `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1`
  and `python3 scripts/ci/run-tests.py native` on Windows.
- Reproduce the integrator's intermittent curl hang on Windows (UNRUN here):
  `1..200 | % { cargo test -p turnloop-http --features turnloop --test asynchronous curl_against; if ($LASTEXITCODE) { break } }`.
  No claim that the batch fix explains or resolves that historical hang.
- Separate follow-up lane: named-pipe backlog/ERROR_PIPE_BUSY retry; avoiding the
  bridge for accepted pipes on the same port; remaining Windows revision-2 tests
  (live-child loop drop, argv/env/cwd, worker-file FIFO under drop, exit/signal
  allocation coverage beyond this lane's child gate); CTRL_CLOSE_EVENT,
  CREATE_NO_WINDOW and KILL_ON_JOB_CLOSE semantics. Also preserve the review's
  synchronous-duplex FIFO and integration-helper error-reporting follow-ups.
- Linux/Windows runtime and Windows protocol cross-builds are UNRUN locally (no
  hosts/Windows SDK for ring). SQL and browser runtime are UNRUN in this lane;
  known sandbox failures are not counted as test passes. No unrelated service or
  browser code was changed.

## iocp-fix2

Base: integrator commit `87a44cd`, PR #5. Host: macOS arm64, 2026-09-15.
Windows runtime for this follow-up is **UNRUN locally (no Windows host)**.

### Root cause

The integrator's windows-2025 run **34910929791** failed
`kill_then_close_children_completes_once` with OS error 5 in all three modes.
The prior check recognized only a fully signaled process. A successful termination
request can precede handle signaling, so immediate close issued another request
and leaked ERROR_ACCESS_DENIED. The prior unit test covered callback lag after
complete exit, leaving this earlier window untested. Its panic poisoned the
shared test mutex, causing the two reported knock-on failures.

Microsoft documents asynchronous [TerminateProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess)
and the same termination mechanism for [TerminateJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-terminatejobobject).
[libuv's Windows kill path](https://github.com/libuv/libuv/blob/v1.x/src/win/process.c#L1299-L1324)
checks the exit code on access denied, retaining a zero-timeout handle check for
an actual exit code equal to STILL_ACTIVE (259). This supports the supplied
diagnosis; this lane cannot reproduce the kernel timing on macOS.

### Fix and call-site audit

- `Child::terminating` records an accepted process/job kill, or access denied
  with evidence that exit has begun. A second kill returns NotFound. Close skips
  repeated termination, while exit readiness still requires the real callback or
  a signaled owned handle. No early status, cancellation or Closed is synthesized.
- Access denied checks `GetExitCodeProcess != STILL_ACTIVE` and retains the
  signaled-handle fallback. Other termination errors preserve both kind and OS
  code; a failed exit-code query does not replace the termination error.
- `Child::Drop` routes termination through the same logic, avoids repeats, waits
  for actual exit and joins the callback. `job_assigned` distinguishes a usable
  tree from failed suspended-spawn setup: an unassigned job cannot substitute for
  terminating the child directly. A failed job kill still permits process cleanup.
- `Platform::kill` and `Services::kill` already delegate to Child. Services close
  sets its closing flag only after `Child::close` succeeds, and collection retains
  the exit watch until real exit. These paths require no additional mutation.
- The standalone spike's Drop deliberately ignores both termination return values
  and always waits on the parent; repeated-call access denied cannot escape or
  skip its wait. Its group-kill probe issues one request after asserting both live
  subjects, then waits for parent and grandchild. These three spike call sites
  remain unchanged. The unit-test forwarding calls count the real APIs and use
  the same Child result handling as production.
- All eight lifetime-test mutex acquisitions recover with
  `unwrap_or_else(std::sync::PoisonError::into_inner)`. Original test failures
  still fail. A source comparison confirms the **200 process + 200 job** loop,
  its completion helper and thresholds are unchanged (only the guard changed).

### Added coverage

Three Windows unit tests and one Windows allocation test were added:

- `terminating_process_without_callback_does_not_repeat_kill_or_complete_early`:
  four process/job × successful-request/access-denied cases. Per-child test-only
  overrides model termination results and an exit code while a real suspended
  child, with its wait unregistered, stays unsignaled. Counters require exactly
  one termination call and the appropriate exit query; two repeat kills and two
  closes must leave readiness/status pending. Cleanup restores real APIs even
  during assertion unwinding.
- `termination_errors_keep_their_identity_and_allow_retry`: four process/job ×
  active-status/failed-query cases preserve exact errors, retry actual wrapper
  calls, and prove other errors bypass the status query.
- `drop_joins_termination_without_repeating_it_and_handles_unassigned_jobs`:
  five real suspended-child cases cover process/job Drop, prior successful
  process/job kill and an unassigned job. Exact API counts and a signaled duplicate
  after Drop prove termination and joining ran.
- `windows_kill_repeat_and_immediate_close_allocate_nothing_after_setup`: eighteen
  children cover plain process kill, process-only kill within a job and group
  kill. After setup, all kills/repeat NotFound/close operations and 18 Cancelled +
  18 Closed completions require **zero allocations**. Capacity-one output, exact
  identities, positive counts, no duplicates and signaled handles at Closed are
  asserted. The existing allocation test is unchanged.

Deterministic kernel timing is unavailable: suspending the child's user thread
does not suspend kernel termination. The injected test establishes the unsignaled
state without a timing race; it is explicitly not proof of kernel timing. The
unchanged 400-cycle real-Windows contract remains the runtime regression.

### Verification commands

All available local checks passed. [Every command and all 36 nested native
invocations](../iocp-fix2-commands.md) are recorded separately; raw logs and the
machine-readable ledger are under `.tools/iocp-fix2/`. No verification failure
occurred in this follow-up. Cross-compilation is not Windows runtime proof.

| Result | Command |
| --- | --- |
| PASS | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| PASS | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| PASS | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| PASS | `cargo +stable check --locked --workspace --all-targets --all-features` |
| PASS | `cargo fmt --all --check` (also applied `cargo fmt --all`) |
| PASS | `python3 scripts/ci/run-tests.py native` |
| PASS | `bash scripts/ci/no-tokio.sh` |
| PASS | `python3 scripts/ci/soak.py` |
| PASS | `python3 scripts/ci/check-paths.py` |
| PASS | `python3 scripts/ci/feature_modes.py` |
| PASS | `python3 .tools/iocp-fix2/audit.py` |
| PASS | `git diff --check` |
| UNRUN | `cargo test --locked -p turnloop --lib backend::iocp::process -- --nocapture` (Windows) |
| UNRUN | `cargo test --locked -p turnloop-contract --test windows_lifetimes -- --nocapture` (Windows, default parallelism) |
| UNRUN | `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1` (Windows) |
| UNRUN | `python3 scripts/ci/run-tests.py native` (Windows) |

Native workspace counts are **236 / 243 / 276** (default/executor/all features),
with **49 / 56 / 56** independently executed contract tests. The total **1,297
passes** includes package repetitions. Existing allocation/no-spin gates ran in
all applicable modes. Ignored services and cfg-excluded Windows tests are UNRUN.
No-tokio passes all eight targets plus their union, default/all features. The
soak gate passes all **251 locked versions**, with only the inherited exact rustls
exception. Linux runtime and WASI/web runtime are UNRUN in this Windows-only lane;
SQL bodies remain UNRUN (sandbox).

### Deviations, open items and next steps

No DESIGN change is proposed. No dependencies, lockfile, soak settings, CI gates,
allocation thresholds, no-spin code or assertions changed. The inherited rustls
security exception is unchanged. Test-only overrides add no production fields,
indirection or allocations.

Integrator: commit the coherent tree and run Windows default/executor/all-features
CI, including `windows_lifetimes` with normal libtest parallelism. The supplied
87a44cd passing tests remain historical Windows evidence, not a follow-up pass.
This lane makes no commits or pushes because `.git` is read-only.

Known unrelated open item: `protocol-wasi (wasm32-wasip3)` HTTP
`expect_continue_timeout_and_early_response` hit a usize assertion. Outside this
lane's scope; no WASI implementation or test was changed.
