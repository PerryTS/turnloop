# tl-i01 — queued work and OS waits

**Current work:** [tl-i01b](#tl-i01b--approved-nonblocking-discovery-amendment)
implements the spec-owner-approved outcome 2. The original audit below is historical.

Status: **specification decision required; I01 is not fixed**. Base a0fbb1b,
macOS arm64. The audit reproduced in a shared contract: one queued post delivered
with an idle pending UDP receive, **os_waits=1**. A temporary driver-only change
to skip backend polling whenever work is queued passed the new regression and
the existing no-spin test, but failed the unchanged timer/I/O/post fairness test
after two seconds. Per the lane instruction, production changes have stopped;
the original driver is restored. No rule or test is weakened.

## Implemented

- Shared queued-post/idle-UDP regression, instantiated on native Unix, Windows,
  WASI 0.2 and 0.3. Checks actual post delivery, zero waits, and later byte-verified
  I/O progress. It currently fails on macOS as intended by the audit reproduction.
- Queued Cancelled/Closed with idle UDP and capacity-one output; both terminal
  results must deliver before the zero-wait assertion. Original driver delivers
  both and records two OS waits.
- Sustained producer with a separate sending loop: exhaust receiver readiness,
  verify the sender wrote a byte, replenish a post before every receiver turn,
  require at least 64 turns and fresh read progress within the existing fairness
  test's two-second deadline. Capacity-one output remains full. Original-driver
  probe: 63 posts, one exact-byte receive, five OS waits in 64 turns.
- Callback-equivalent browser/Node test: 64 posts with an idle WebSocket read,
  zero waits, then a real callback-driven byte exchange and Closed delivery.
  Runtime UNRUN here; instantiated in both existing web test binaries.
- Allocation gate: 1,000 warm-up plus 1,000 measured post/cancel/close rounds,
  3,000 measured completions beside idle UDP, capacity-one output, zero allocation
  threshold. No production behavior was introduced. Existing allocation gates
  and all previous tests remain intact.

## Evidence / specification decision

The current Backend revision 2 exposes `poll(timeout)` and `has_work()`, but no
strictly no-wait collection operation. A zero duration still permits one OS wait.
Skipping poll also prevents cached I/O and queued native terminal delivery.
More fundamentally, after Unix cached readiness reaches EAGAIN, only the poller
event path marks that resource ready again. IOCP similarly obtains pending native
completion acknowledgements through its port wait (unless the host explicitly
selected the separate Event helper). WASI p2/p3 discover new readiness/completions
through their poll/wait-set paths. A cached-only collection method alone cannot
guarantee discovery through an indefinitely replenished post/timer backlog.

Decision needed: define how fresh native I/O discovery is permitted while core
completions remain continuously queued. A zero-time wait exemption would change
§10.3 and is **not authorized**. Treating completions copied into host output as
no longer queued would also require an explicit interpretation; it cannot be
silently used to claim the hot-path guarantee. A no-wait backend discovery design
needs a concrete mechanism on all six targets, bounded work and allocation gates;
adding a background poller by default changes the host-owned-loop architecture.

A second temporary guard, `(!queued || self.backend.has_work())`, allows
cached work through. It passed the post and terminal no-wait regressions, but
failed both fresh-I/O producer progress (64 posts, zero reads, zero waits in the
initial fixed-count diagnostic) and the unchanged repeating-timer fairness test.
Final sustained coverage retains the existing two-second fairness deadline while
requiring at least 64 turns; the initial 64-turn probe is evidence, not a new
universal scheduling bound. All temporary guards are restored.

### Concrete decision submitted (not adopted)

**Retain both current rules as release requirements; reject both driver-only
patches.** Revision 2's `poll(Duration::ZERO)` is not a no-wait primitive.
A production fix needs an agreed native-discovery mechanism before implementation
can resume. The specification owner must choose one of these explicit outcomes:

1. Preserve both rules and require bounded discovery while queues remain nonempty
   on **every backend**, with a separate no-wait collection API. Specify how IOCP
   acknowledgements and WASI host progress occur, the syscall/allocation bounds,
   and who owns scheduling. Cached-only draining is insufficient, as reproduced.
2. Amend §10.3 to permit one nonblocking native discovery call on a queued turn
   when native operations are pending and native output reserve is available.
   This is the existing driver's policy and retains the demonstrated hot-path
   cost; it does **not** satisfy I01's current acceptance.
3. Keep absolute §10.3 and explicitly make fresh-I/O discovery conditional on a
   queue-free turn supplied by the host (drain/throttle producer ingress).
   This changes the existing sustained-backlog fairness guarantee and cannot
   pass the unchanged fairness contract.

Only outcome 1 preserves both requirements. Outcomes 2 and 3 need explicit
specification approval and revised acceptance; neither is implemented or used to
weaken tests. No default helper thread, hidden zero-time poll, redefined wait
counter, or drain-before-poll reinterpretation has been introduced.

DESIGN.md remains unchanged. This is a conflict with the existing backend model,
not a proof that every possible backend redesign is impossible.

## Verification commands

The final command ledger below includes all invocations, including deliberate
FAIL probes. Plain cargo uses nightly-2026-08-20; WASI p3 uses the repository's
nightly-2026-09-07. Cross-target checks cover core/contracts and all their test
binaries, not full protocol C toolchains. No cross-compilation is counted as runtime
coverage. The seven-day gate checks 251 locked registry versions with the sole
inherited rustls 0.23.45 security exception; no policy/lockfile change.


| Command | Status / evidence |
|---|---|
| `cargo test --locked -p turnloop-contract --lib native::queued_post_with_idle_native_io_never_waits -- --exact --nocapture` | **FAIL**, one test executed; delivered=1, os_waits=1 |
| `cargo test --locked -p turnloop-contract --lib native:: -- --test-threads=1 --nocapture` with temporary `if self.buffered[NATIVE_EVENTS] == 0 && !queued` | **FAIL**, 22 passed / 1 failed; new zero-wait and existing no-spin passed; unchanged timer backlog fairness failed |

Raw logs: `.tools/tl-i01/`. The temporary driver mutation was restored from the
saved original file; no Git writes. Mandatory design/contribution/integration and
relevant core/Windows/WASM lane reports read fully. Initial tree clean; no applicable
AGENTS.md. Read-only rg/cat/sed/status/toolchain inspection completed; one guessed
web module path did not exist and was corrected to `backend/web.rs`.

## Final verification summary

| Check | Result |
|---|---|
| Formatting, native strict workspace Clippy (default/all features), stable workspace/all-target/all-feature check | **PASS** |
| Core/contract strict all-target Clippy: Linux x86_64 + arm64, six modes each | **PASS**, default, timerfd, SIGCHLD, combined fallbacks, executor, all features |
| Core/contract strict all-target Clippy: Windows, WASI 0.2/0.3, web, default/all features | **PASS**; p3 all features instantiates its experimental backend; inherited Cargo manifest/config warnings remain |
| `cargo test --workspace` | **FAIL**, 35 passed / 3 failed before fail-fast stopped later binaries; all three failures are new §10.3 regressions |
| Serial all-feature workspace with `--no-fail-fast` | **FAIL**, 308 passed / 3 failed / 20 ignored, plus MongoDB doctest compilation error E0463 (`can't find crate for bson`) |
| Existing shared fairness and idle-socket no-spin tests | **PASS** on restored driver in both workspace runs |
| Core/contract allocation binary | **PASS**, 11 default and 12 all-feature tests; includes new calibrated zero-allocation subject and all existing gates |
| Zero-tokio / seven-day soak | **PASS**, all eight target graphs plus union, default/all features; 251 locked versions, inherited rustls security exception only |
| Path case, feature modes, whitespace, source audit | **PASS**; no production/design/dependency changes; no existing test lines removed; no new unwrap or unsafe |

The new post regression records `os_waits=1` and verifies later reads=1/writes=1
before asserting the wait violation. Queued terminal delivery records two waits
for two completions. The final sustained producer runs deliver 63 posts and one
byte in 64 full-output turns with two waits (default) and three waits (all features),
then deliver the remaining post. Those variable counts are measurements, not
required counts; the unchanged acceptance is zero.

MongoDB doctest compilation failed while cross-compilation was also running;
no protocol source changed. The isolated `cargo test --locked -p turnloop-mongodb
--all-features --doc` retry **PASSed compilation** and found zero doctests. It is
not a runtime test pass and does not erase the original workspace-command failure.
The transient build-artifact cause is unconfirmed; no protocol change was made. The ignored cases and cfg-excluded platform binaries
are **UNRUN**, never included in positive pass counts. No failed test is ignored
or changed to pass. The default workspace's early stop is not a full-workspace pass.

The final tree's validation is complete. The inherited root lane report is
preserved in Git history and `.tools/tl-i01/inherited-report.md`. No commits,
pushes, dependency updates or DESIGN amendments were made.

## Runtime handoff and open questions

I01 remains **not fixed**. Resolve the native-discovery decision above before
resuming production work; the test-only tree intentionally exposes the three
§10.3 failures. All old fairness/no-spin/allocation assertions are retained.
The integrator must commit the report and tests; `.git` is read-only here.

| Command / environment | Status |
|---|---|
| `python3 scripts/ci/run-tests.py native` on Linux x86_64 and arm64 | **UNRUN (no hosts)**; all six required modes/fallbacks must execute |
| `python3 scripts/ci/run-tests.py native` on Windows | **UNRUN (no host)**; default/executor/all-feature contracts and allocation gates |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | **UNRUN here**, integrator CI requested |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | **UNRUN here**, integrator CI requested; experimental feature retained |
| `python3 scripts/ci/run-tests.py web` and `python3 scripts/ci/run-tests.py node` | **UNRUN here**, integrator CI requested; both browser and Node include the new callback case |
| Full-workspace cross-target Clippy including protocol C dependencies | **UNRUN**; cross-checks above cover core/contracts; native full-workspace Clippy PASS |
| Full real-server tests, SQL initialization, browser sandbox launch | **UNRUN**; unrelated to the stopped driver fix; supplied sandbox limitations retained |
| Linux instruction benchmark / long runtime soak / hosted CI | **UNRUN (no corresponding host/run)**; no benchmark or policy change |

## Complete command ledger

Every wrapped verification invocation is listed below; names distinguish temporary
probe states from the restored driver. The two initial unwrapped commands are
recorded above. Source inspection, report generation, saved-original restoration
and ledger aggregation used local Python/cp plus read-only rg/cat/sed/Git queries;
all completed. The source audit compares the driver, DESIGN, lockfile and resolver
configuration byte-for-byte to HEAD and rejects removal of any pre-existing Rust
line. Raw logs and the exact audit script remain under `.tools/tl-i01/`.

| Invocation | Result | Exact command |
|---|---|---|
| fmt-apply | **PASS** | `cargo fmt --all` |
| native-contracts | **FAIL** | `cargo test --locked -p turnloop-contract --lib native:: -- --test-threads=1 --nocapture` |
| cached-only-probe | **FAIL** | `cargo test --locked -p turnloop-contract --lib native:: -- --test-threads=1 --nocapture` |
| fmt-apply-final | **PASS** | `cargo fmt --all` |
| native-clippy | **PASS** | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-after-allocation | **PASS** | `cargo fmt --all` |
| cross-windows | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-ready | **PASS** | `cargo fmt --all` |
| native-clippy-final | **PASS** | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| no-tokio | **PASS** | `bash scripts/ci/no-tokio.sh` |
| native-clippy-all | **PASS** | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-unknown-linux-gnu-default | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| soak | **PASS** | `python3 scripts/ci/soak.py` |
| fmt-final-apply | **PASS** | `cargo fmt --all` |
| stable | **PASS** | `cargo +stable check --locked --workspace --all-targets --all-features` |
| x86_64-unknown-linux-gnu-timerfd | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-unknown-linux-gnu-sigchld | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-unknown-linux-gnu-fallbacks | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-unknown-linux-gnu-executor | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-unknown-linux-gnu --features executor -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-unknown-linux-gnu-all | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-default | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-timerfd | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target aarch64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-sigchld | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target aarch64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-fallbacks | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target aarch64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-executor | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target aarch64-unknown-linux-gnu --features executor -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-all | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target aarch64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-pc-windows-msvc-default | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-pc-windows-msvc-all | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target x86_64-pc-windows-msvc --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| paths | **PASS** | `python3 scripts/ci/check-paths.py` |
| modes | **PASS** | `python3 scripts/ci/feature_modes.py` |
| fmt | **PASS** | `cargo fmt --check` |
| diff | **PASS** | `git diff --check` |
| source-audit | **PASS** | `python3 .tools/tl-i01/audit.py` |
| workspace-tests | **FAIL** | `cargo test --workspace` |
| wasm32-wasip2-default | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasm32-wasip2-all | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target wasm32-wasip2 --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasm32-wasip3-default | **PASS** | `cargo +nightly-2026-09-07 clippy --locked -p turnloop -p turnloop-contract --all-targets --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasm32-wasip3-all | **PASS** | `cargo +nightly-2026-09-07 clippy --locked -p turnloop -p turnloop-contract --all-targets --target wasm32-wasip3 --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasm32-unknown-unknown-default | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasm32-unknown-unknown-all | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --target wasm32-unknown-unknown --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| workspace-serial-all | **FAIL** | `cargo test --locked --workspace --all-features --no-fail-fast -- --test-threads=1` |
| native-clippy-complete | **PASS** | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| allocations | **PASS** | `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1 --nocapture` |
| stable-complete | **PASS** | `cargo +stable check --locked --workspace --all-targets --all-features` |
| x86_64-unknown-linux-gnu-complete | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| aarch64-unknown-linux-gnu-complete | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| x86_64-pc-windows-msvc-complete | **PASS** | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| mongodb-doc-isolated | **PASS** | `cargo test --locked -p turnloop-mongodb --all-features --doc` |
| final-fmt | **PASS** | `cargo fmt --check` |
| final-source-audit | **PASS** | `python3 .tools/tl-i01/audit.py` |
| report-whitespace | **PASS** | `git diff --check` |


## tl-i01b — approved nonblocking discovery amendment

Status: implementation complete; final verification in progress. The spec owner
selected refined outcome 2 on 2026-09-15. This supersedes the original tl-i01
specification blocker and its intentionally failing zero-total-call acceptance.
The historical evidence and command ledger above are retained.

### Implemented

- Amended DESIGN D7/§10 rule 3 with the native-pending/output-reserve condition,
  zero syscalls for queued pure-core work and the libuv/fairness rationale.
  Updated revision-2, contribution, integration and p3 accounting restatements.
- `PollInfo::waits` / `TurnInfo::os_waits` count positive-timeout/infinite native
  waits; `discovery_polls` counts zero-time native discovery. Their sum keeps the
  original single-call bound. Epoll/timerfd, kqueue, direct IOCP and WASI p2/p3
  classify the actual call. Web and IOCP Event-helper queue draining report both
  zero. No helper waits are attributed to the turning thread.
- `zero_event_waits` retains its existing raw meaning across **both** categories,
  including interrupted calls and each backend's private-timeout accounting.
  Every old no-spin limit is unchanged; invocation bounds/totals use the sum.
- Tightened the driver guard: queued work with zero native-pending operations
  skips the backend even if `has_work()` reports cached work. Native discovery
  through queued post/timer backlogs and native reserve backpressure are retained.
- Shared post + idle UDP requires zero blocking waits and exactly one discovery
  poll, then exact-byte I/O. Queued Cancelled/Closed, pure-core work, and sustained
  producer contracts run on native/WASI/Windows; web has callback equivalents.
  The sustained producer still requires at least 64 turns and fresh I/O within
  two seconds. The pre-existing repeating-timer fairness test is unchanged.
- Native allocation gate retains 1,000 warm-up + 1,000 measured rounds, 3,000
  completions, capacity-one output and absolute zero allocations. It now also
  requires 3,000 measured discovery polls and no blocking waits.
- Added direct kqueue and IOCP/Event counter tests, plus a synthetic driver test
  that verifies 64 delivered posts and exactly zero backend calls with cached work.
  Old strict zero-call assertions require both counters zero; Now-only benchmarks
  now assert discovery counts without changing workload/baselines.
- Restored root LANE_REPORT.md byte-for-byte from inherited HEAD; lane reporting
  lives here. No Git writes, dependency updates, policy or baseline changes.

### Verification (in progress)

Logs and the exact machine-readable command ledger: `.tools/tl-i01b/`.
Initial native shared contracts PASS (26 tests) and allocation binary PASS
(11 tests). Native default workspace Clippy and Windows core/contract/bench
all-target/all-feature cross-Clippy PASS. Zero-tokio and seven-day soak PASS
(251 versions, inherited exact rustls exception only).

Intermediate FAIL: new test used PartialEq on Timeout, which does not implement
it; changed to matches! without altering behavior. A local multi-file edit twice
stopped at a missing marker; source inspection completed the remaining edits.
WASI p3 default initially found the new helper unused with its backend disabled;
matching its existing backend feature cfg fixed that diagnostic. The full default
workspace then reached a pre-existing turnloop-io allocation test that imports
Platform without enabling the experimental backend. Required feature-enabled
verification is recorded below; no test cfg was weakened.

Full Windows workspace cross-Clippy FAILed in ring because Windows SDK assert.h
is unavailable; core/contract/bench all-target checks PASS separately.

Initial Linux whole-workspace cross-checks failed because a local Zig wrapper
rewrote an output path as well as the target argument; corrected the wrapper to
translate only --target. No repository build configuration was changed.

### Deviations / open questions / next steps

No further DESIGN changes proposed; DESIGN has no separate change log, so its
dated rationale paragraph records the approved amendment. WASI p3 retains the documented experimental
cooperative-yield/scheduling limitations; counter separation does not claim to
resolve them. Windows/Linux runtime and WASI/web/Node runtime are UNRUN here;
the integrator must run the unchanged required CI matrix. No runtime result is
inferred from cross-compilation. Complete local matrix and append exact ledger.
