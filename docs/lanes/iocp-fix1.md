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
