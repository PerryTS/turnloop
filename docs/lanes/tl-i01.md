# tl-i01 — queued work and OS waits

**Current work:** [tl-i01b](#tl-i01b--approved-nonblocking-discovery-amendment)
implements the spec-owner-approved outcome 2 (committed as `c7a2555` + `904e98f`).
The original tl-i01 audit below is historical; its "not fixed" status and
"DESIGN unchanged" statements describe the tree before the decision.

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

Status: **implemented; macOS + WASI 0.2/0.3 runtime verified locally; Linux,
Windows and web runtime await the integrator's CI.** The spec owner selected
refined outcome 2 on 2026-09-15; it supersedes the original tl-i01 blocker and
its zero-total-call acceptance. The historical tl-i01 evidence above is retained.
A Codex agent started this lane and stopped at its usage limit (checkpoint
`c7a2555`); Claude reviewed that checkpoint, fixed what it missed and finished
verification (`904e98f` and this report). Root `LANE_REPORT.md` is byte-identical
to `origin/main` (`a0fbb1b`).

### Adopted wording (DESIGN §10 rule 3, D7 steps 1/2/6)

§10 rule 3 now reads, verbatim from the decision:

1. A `turn` makes at most one OS wait.
2. When completions are already queued, the turn performs **no blocking wait**
   (positive or infinite timeout).
3. It may perform **at most one nonblocking native discovery poll** (zero
   timeout), and **only if native operations are pending** and native output
   reserve is available.
4. With queued work and **no** native operations pending, the turn makes **no OS
   call at all**: pure post/timer/terminal churn costs zero syscalls.
5. The no-spin rule (4a) is unchanged.

It defines *queued work* (completions already queued before the native step:
posts, blocking-pool and external-wait results, synchronous and terminal
completions including a timer's `Cancelled`/`Closed`) and *native operations*
(operations on native handles plus requests a backend accepts natively, i.e.
WASI 0.2 DNS), names the counters and lists the contract tests. A dated
rationale paragraph cites libuv's zero-timeout `uv__io_poll` while pending work
exists and the tl-i01 fairness probes. DESIGN has no change log; the rationale
paragraph is the record. D7 step 1 (the old `:187` sentence), step 2 and step 6
(`TurnInfo` fields) restate the rule. Restatements updated: `CONTRIBUTING.md`,
`docs/BACKEND_REVISION_2.md`, `docs/INTEGRATION_REPORT.md`,
`docs/upstream/wasi-p3-wait.md`, `backend/mod.rs` rustdoc, `TurnInfo`/`PollInfo`
docs. The checkpoint's libuv link carried unverifiable line anchors
(`core.c#L374-L449`); replaced by the libuv loop-API docs and the file link.

### Implemented (checkpoint `c7a2555` + `904e98f`)

- `PollInfo` split into `waits` / `discovery_polls` (plus unchanged
  `zero_event_waits`), mirrored in `TurnInfo::os_waits` / `discovery_polls`.
  Every backend classifies its actual call (table below).
- Shared regressions on native, WASI 0.2/0.3 and Windows:
  - `queued_post_idle_io`: post delivered with `os_waits == 0` and
    `discovery_polls == 1`, then byte-verified UDP read/write.
  - `queued_core_work`: posts and timer `Cancelled`/`Closed` with no native
    operation under Now/After/Forever give `(0, 0, 0)`.
  - `queued_terminals_idle_io`: queued `Cancelled`/`Closed` with capacity-one
    output, zero waits, one discovery poll each; a due timer beside idle UDP makes
    one discovery poll.
  - `sustained_posts_idle_io`: a post replenished before each of at least 64
    turns, a fresh read inside the unchanged two-second deadline, zero waits,
    zero discovery once no native operation remains.
  - The pre-existing repeating-timer fairness test is unchanged.
- Web callback equivalents: 64 posts beside an idle WebSocket read, then a real
  exchange and `Closed`, all `(0, 0)`, plus `queued_core_work`.
- Synthetic driver tests: 64 queued posts with cached backend work and zero
  backend calls; a natively accepted lookup keeps discovery through queued posts.
- Direct counter tests: kqueue (zero/finite/infinite × empty/woken), epoll
  assertions, IOCP direct versus Event-helper drain.
- Allocation gate `queued_posts_and_terminals_with_idle_udp_allocate_nothing`:
  1,000 warm-up + 1,000 measured rounds, 3,000 completions beside idle UDP,
  capacity-one output, zero allocations, and exactly 3,000 discovery polls and 0
  blocking waits. It runs natively and in the WASI allocation binaries.

### Counter semantics per backend

`PollInfo::waits` / `TurnInfo::os_waits`: blocking native waits (positive or
infinite timeout). `PollInfo::discovery_polls` / `TurnInfo::discovery_polls`:
zero-timeout native polls. Their sum is at most one per turn. `zero_event_waits`
keeps its revision-2 meaning across both kinds: native calls that returned no
native I/O or notifier event, including EINTR and private timeout expiry.
Classification is `PollInfo::native(timeout, empty)` on the effective timeout.

| Backend | `waits` | `discovery_polls` | Neither |
|---|---|---|---|
| epoll (`epoll_pwait2`) | timespec > 0 or null | timespec 0 | backend early return with cached work/events |
| epoll + timerfd fallback | armed timerfd, `epoll_wait(-1)` | `epoll_wait(0)`, preceded by a `timerfd_settime` disarm | same |
| kqueue | timespec > 0 or null | timespec 0 | same |
| IOCP direct | GQCSEx `INFINITE` behind the NT high-resolution packet timer | GQCSEx 0 | services/cached results already produced events |
| IOCP Event helper | never on the turning thread | never | queue drain; the helper's own blocking wait is outside the turn |
| WASI 0.2 | `wasi:io/poll` with a positive or absent clock pollable | zero-duration clock pollable | cached ready/cancelled/DNS results |
| WASI 0.3 (experimental) | blocking wait-set step | nonblocking step, including a deadline already completed during setup (cooperative yield stays part of discovery) | cached results |
| web | never | never | callback/Worker-ring draining |

Driver guard: the backend is called only with native output reserve free
(`buffered[NATIVE_EVENTS] == 0`) and, when work is queued, only while a native
operation is pending (`native_pending != 0`); queued turns force a zero
timeout. On native and WASI backends `has_work()` cannot force a queued call
without native operations, because a backend that drains stale cached readiness
falls through to its OS wait. On web the old `has_work()` arm is retained
(`cfg(turnloop_backend = "web")`): web poll never enters the OS but is what pumps
Worker/condition rings (`beginTurn`), and `worker_pending` is part of
`has_work()`. Without that arm, sustained local posts could starve a Worker ring
retained by a full poster; the checkpoint had removed it for every backend.

### What changed after the checkpoint (`904e98f`)

Review of `c7a2555` found four gaps; all fixed:

1. **Native lookups were not native operations.** `Driver::resolve` allocates a
   handle-less op, so a lookup the WASI 0.2 backend accepted never counted in
   `native_pending`. Queued posts then suppressed all discovery. Mutation run with
   the counting removed: `queued posts starved DNS: turns=689276, posts=689276,
   discovery_polls=0` (2 s deadline). Fix: an `Op::native` flag set for
   socket-handle ops and backend-accepted lookups, decremented in `retire`.
   Regressions: WASI 0.2 `queued_posts_preserve_native_dns_discovery` (64+ turns,
   post replenished before each, zero blocking waits, loopback `Resolved` within
   2 s, zero discovery once resolved) and the synthetic
   `natively_accepted_lookup_is_a_pending_native_operation` (mutation FAIL at
   turn 0: 0 polls vs 1).
2. **Web regression**, described above.
3. **Two revision-2 totals were missed**: `idle_keepalive` in
   `protocols/turnloop-http/tests/asynchronous.rs` (`waits > 0`) and the Windows
   busy-pipe allocation gate (`waits > 0`). Both summed `os_waits`, which used to
   include zero-timeout calls, so both now sum `os_waits + discovery_polls`: the
   same bound as before, not a loosened one. The four protocol `os_waits == 1`
   assertions for turns lasting at least 10 ms already mean one blocking wait and
   stay unchanged.
4. **DESIGN D7 step 1** (the old `:187` sentence) still said "zero if completions
   are already queued" without the discovery condition. The libuv link carried
   line anchors nobody could verify.

Also added: a due timer beside idle UDP makes one discovery poll and no blocking
wait (`queued_terminals_idle_io`).

**Counter mapping for existing assertions.** An old `os_waits == 0` assertion
that passed on main meant *no native call of any kind*, so it became
`(os_waits, discovery_polls) == (0, 0)`. An old `os_waits == 1` on a timed wait
became `(1, 0, …)`. Per-turn `os_waits <= 1` became the sum `<= 1`. Totals
`waits >= N` / `> 0` became sums. Now-only benchmarks count `discovery_polls`.
No numeric bound changed. Only the new I01 regressions, which failed on main by
design, use `os_waits == 0 && discovery_polls == 1`.

**Prototype withdrawn: due-timer expiry without native operations.** Treating an
already-due timer as queued work, so that it makes no OS call when no native
operation is pending, passed macOS contracts and the full WASI 0.2 suite. I
reverted it for two reasons. First, it is not in the approved wording: queued
work means completions already queued. Second, IOCP Event-helper mode re-arms
its deadline timer only after a poll drains the helper's forwarded `TIMER`
packet (`arm_event_deadline` returns early while `timer_pending`). Skipping that
poll could leave the next deadline unarmed while a GUI host waits on the Event
(`gui_event_tracks_timer_reset_and_partial_output`). This cannot be validated
without Windows. DESIGN now states explicitly that a due expiry is not queued
work and may spend its one call on a zero-timeout poll.

### Verification (tree `904e98f`, macOS arm64, `CARGO_BUILD_JOBS=4`)

Logs: `.tools/tl-i01b-claude/` (ignored). Plain `cargo` is nightly-2026-08-20;
WASI 0.3 uses nightly-2026-09-07. wasmtime 46.0.0 (the `scripts/ci/tools.json`
pin) was linked into `.tools/bin` from an existing local install. Unless marked
otherwise, the commands below ran on the committed tree. The checkpoint's
earlier Codex ledger (`.tools/tl-i01b/`) predates these fixes and is superseded.

| Command | Result |
|---|---|
| `cargo fmt --check` | **PASS** |
| `git diff --check a0fbb1b..HEAD`, `python3 scripts/ci/check-paths.py`, `python3 scripts/ci/feature_modes.py` | **PASS** (1452 files; 6 features / 18 native arms) |
| `cargo test --locked -p turnloop --lib clock_contract -- --test-threads=1` with the lookup counting removed (mutation) | **FAIL** as intended: lookup test, turn 0, 0 polls vs 1 |
| `cargo test --locked -p turnloop-contract --test wasi --all-features --target wasm32-wasip2 -- queued_posts_preserve_native_dns_discovery --exact` with the lookup counting removed (mutation) | **FAIL** as intended: 689,276 turns, 0 discovery, DNS starved |
| `python3 scripts/ci/run-tests.py native` (macOS: default, executor, all-features; workspace + every member) | **PASS**, 1467 passed / 0 failed / 92 ignored |
| ↳ includes `cargo test --locked --workspace -- --test-threads=1` | **PASS**, 259 tests |
| `cargo test --locked -p turnloop-{http,mongodb,mysql,postgres,smtp} --features turnloop --test asynchronous -- --test-threads=1` | **PASS** (12/3/3/5/3; one Node-fixture test ignored). This ran on the withdrawn due-timer prototype; the all-features native run above re-ran these suites on `904e98f` |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | **PASS**: core lib 9, contracts 27 debug + 27 release, allocations 10 |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | **PASS**: core lib 11, contracts 27 + 27, allocations 11 |
| `cargo clippy --locked --workspace --all-targets [--all-features] -- -D warnings -D clippy::undocumented_unsafe_blocks` (native) | **PASS** both |
| Same strict flags, `-p turnloop -p turnloop-contract -p turnloop-io --all-targets`: x86_64-unknown-linux-gnu (default, `turnloop/epoll-timerfd`, all-features), aarch64-unknown-linux-gnu all-features, x86_64-pc-windows-msvc (default, all-features), wasm32-wasip2 (default, all-features), wasm32-unknown-unknown (default, all-features), wasm32-wasip3 all-features | **PASS** (11 invocations) |
| wasm32-wasip3 default, same package set | **FAIL, inherited**: `crates/turnloop-io/tests/streams.rs` imports `backend::Platform` without `wasi-p3-experimental` (file untouched since `a0fbb1b`; CI clippies p3 with `--all-features`). `-p turnloop -p turnloop-contract` and `-p turnloop-io --lib` **PASS** |
| `cargo clippy --workspace --all-targets --all-features` for wasm32-wasip2 / wasm32-unknown-unknown / wasm32-wasip3 | **FAIL, environment**: `ring` build script needs the wasm C toolchain (`scripts/ci/install-wasm-toolchain.py`), not installed here |
| `cargo +stable check --locked --workspace --all-targets --all-features` | **PASS** |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --all-features --no-deps` | **PASS** |
| `bash scripts/ci/no-tokio.sh` | **PASS** (all graphs, default + all features) |
| `python3 scripts/ci/soak.py` | **PASS**: 251 locked versions; inherited rustls 0.23.45 exception only, expires 2026-09-21 |
| `cargo run --locked --release -p turnloop-bench -- --portable [--instruction-boundaries]` | **PASS** both (Now-only discovery assertions hold) |
| `python3 scripts/ci/run-tests.py native` on Linux x86_64/arm64 (all modes incl. timerfd/SIGCHLD fallbacks) | **UNRUN**, no host |
| `python3 scripts/ci/run-tests.py native` on Windows (incl. new IOCP counter test, Event-helper GUI tests, busy-pipe allocation gate) | **UNRUN**, no host |
| `python3 scripts/ci/run-tests.py web` / `node` (new callback and core-work cases, Worker rings) | **UNRUN**: `wasm-bindgen-test-runner` not installed |
| `python3 scripts/ci/run-tests.py protocol` / `protocol-wasi` with real servers | **UNRUN**, not affected by the change |

### Needs CI confirmation / open questions

- **Windows:** `direct_discovery_and_event_queue_draining_have_distinct_counts`
  (direct 0/1/empty, Event-helper drain 0/0/0), the four shared I01 contracts,
  the GUI Event tests' new `(0, 0)` assertions, and the busy-pipe gate's summed
  total. These are compiled by cross-clippy only.
- **Linux:** epoll and timerfd classification (`epoll.rs` unit test), and the
  shared contracts in all six native modes on both architectures. In timerfd
  fallback mode one discovery poll costs `timerfd_settime` plus `epoll_wait(0)`,
  two syscalls; a follow-up could skip the disarm when the timer is not armed.
- **Web/Node:** `queued_post_with_idle_callback_io_never_waits`,
  `queued_core_work_makes_no_native_calls`, and the retained web `has_work()`
  poll for Worker rings.
- **Spec owner:** should timer *expiry* count as "timer churn"? Today a due
  timer with no native operation makes one zero-timeout call. See the withdrawn
  prototype; extending the rule needs an IOCP Event-helper re-arm fix validated
  on Windows. Unverified side note: the same helper `TIMER`-packet drain is
  already skipped on main when a timer fires in a queued turn with no native
  operation. Worth one Windows check.
- **API:** `TurnInfo` gained a public field (`discovery_polls`, no
  `#[non_exhaustive]`), and `os_waits` no longer includes zero-timeout calls.
  Host code that summed waits must add the two counters.
