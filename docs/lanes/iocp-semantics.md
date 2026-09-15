# IOCP semantics — issue #11

Base `a0fbb1b`, branch `lane/iocp-semantics`, macOS arm64, 2026-09-15.
Implementation and local verification complete. Windows runtime
**UNRUN (no Windows host)**. No commits/pushes; `.git` is read-only.

## 1. CTRL_CLOSE_EVENT

**Reference:** libuv v1.52.1
[`uv__signal_control_handler`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/signal.c#L116-L145)
queues SIGHUP and sleeps indefinitely only when a watcher accepted it; otherwise
it returns FALSE. Windows' [documented handler order and close timeout](https://learn.microsoft.com/en-us/windows/console/handlerroutine)
make the close window system-controlled (normally five seconds).

**Decision:** match libuv. Without Hup subscriptions, older host handlers run.
With Hup subscribed, publish the signal, release all ticket accesses, then call
Sleep(INFINITE). The host must turn and exit within the OS budget. This sleep is
on Windows' control thread, with no polling or per-loop wait floor.

**Conflict/deviation:** the requested combination of running earlier unknown
host handlers and retaining this handler thread cannot be achieved with the
supported API: handlers run newest-first, and only FALSE continues the chain.
libuv itself prevents earlier handlers while holding subscribed Hup. This was
raised through an optional clarification; with no reply, the stated Node/libuv
compatibility target selected the decision above. We neither remove nor reorder
host registrations. A host needing its handler first can register it after
turnloop's subscription. Always returning FALSE would preserve older handlers
but surrender libuv's cleanup window; that is not the selected policy.

**Implementation:** `signals.rs` separates dispatch from the close hold; ACTIVE
has already decremented before sleeping, so dropping subscriptions/loops does not
join a permanently parked handler. Int/Break behavior is preserved.

**Tests (Windows runtime UNRUN):**

- `close_dispatch_releases_tickets_before_holding_the_control_thread`: a controlled
  hold proves Hup publication, one consumption, live hold during subscription Drop,
  then FALSE without a subscriber. The synthetic hold is explicitly not an OS close.
- `real_console_close_chains_without_hup_and_allows_subscribed_cleanup`: two fresh
  CREATE_NEW_CONSOLE subprocesses register the host handler first. WM_CLOSE must
  actually terminate the console: the unsubscribed case writes one host-handler
  marker; the subscribed case delivers Hup, drops its loop and exits 23 with an
  exact cleanup marker. Readiness, real HWND, watchdog, exit code and markers are
  required; these intentionally terminating subprocesses do not claim a libtest
  pass of their own.

## 2. windowsHide / CREATE_NO_WINDOW

**Reference:** [Node spawn options](https://nodejs.org/api/child_process.html#child_processspawncommand-args-options)
and libuv [`uv_spawn`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/process.c#L1018-L1068).
Node defaults windowsHide to false. libuv uses SW_HIDE and conditionally suppresses
the console when stdio is not inherited.

**Decision/implementation:** additive `ProcessSpec::windows_hide`, default false,
ignored on Unix. True sets STARTF_USESHOWWINDOW/SW_HIDE; false uses SW_SHOWDEFAULT.
CREATE_NO_WINDOW requires every stdio slot to be Pipe/Null. `Inherit` and
`Handle` preserve console attachment. turnloop's Handle is treated as inherited
stdio; libuv specifically tests UV_INHERIT_FD, while its distinct UV_INHERIT_STREAM
representation has no exact counterpart in ProcessSpec. Existing background,
stdio and allocation fixtures explicitly request hiding. No caller depended on
an assertion of unconditional console detachment with inherited stdio.

**Tests (Windows runtime UNRUN):**
`windows_hide_and_detached_match_console_inheritance` runs eight real children in
an isolated console. It covers default/hidden, Null/Pipe, Inherit, adopted Handle,
and detached combinations, checks actual console membership and startup show mode,
requires real Ctrl-C delivery to attached children and host, and asserts exact
stdout, one exit and one EOF. Existing argv/env/cwd and process tests are retained.

## 3. Lifetime jobs, normal leader exit, detached

**Reference:** libuv
[`uv__init_global_job_handle`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/process.c#L69-L122)
and [`uv_spawn` job assignment](https://github.com/libuv/libuv/blob/v1.52.1/src/win/process.c#L1082-L1103),
[Node detached](https://nodejs.org/api/child_process.html#optionsdetached), and
[Windows nested-job inheritance](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs).

**Decision/implementation:** one process-lifetime, non-inheritable job carries
KILL_ON_JOB_CLOSE, BREAKAWAY_OK, SILENT_BREAKAWAY_OK and DIE_ON_UNHANDLED_EXCEPTION.
Non-detached children join it before resume. Its static owner survives all loops
and individual leader exits; Windows closes it on process death. Silent breakaway
excludes ordinary grandchildren. ERROR_ACCESS_DENIED from lifetime assignment is
tolerated as in libuv; other failures preserve their OS error and clean up the
still-suspended child. Unlike libuv's Windows Store workaround, turnloop does not
assign the embedding host itself to this job; its existing job membership is left
to the host. This backend already requires modern Windows for its timer mechanism.

`new_process_group` and `detached` create a separate tree-control job without
KILL_ON_JOB_CLOSE. Explicit group kill still terminates the tree, but releasing
that job after normal leader exit leaves grandchildren alive. A requested tree
job deliberately retains descendants in its hierarchy, including at parent death;
that is turnloop's explicit tree-control extension. Close/Drop probe the owned
leader's signaled state before considering tree termination, covering callback lag.

Additive `ProcessSpec::detached`, default false, selects DETACHED_PROCESS and
CREATE_NEW_PROCESS_GROUP, skips the lifetime job, and retains explicit group kill.
No CREATE_BREAKAWAY_FROM_JOB is forced; restrictive external host jobs can still
limit independence, as documented by libuv. Unix implements the same option through
async-signal-safe setsid in the child hook; it remains group-kill capable.

**Ownership difference retained:** detached does not imply unref. Explicit close
and owning-loop Drop still terminate live owned children, consistent with the
existing revision-2 contract on both native platforms. Parent process death without
that explicit teardown permits detached survival. This is not a new release/disown
API, and no existing live-child ownership test was changed to permit leaks.

**Tests:**

- Windows **UNRUN** `parent_death_kills_only_non_detached_children`: four combinations
  of detached/default and normal OS exit/forced parent death; handshake pins the
  live child's process handle first, then verifies death or survival with bounded
  waits. RAII cleanup terminates surviving fixtures.
- Windows **UNRUN** `normal_leader_exit_keeps_grandchildren_alive_through_close_and_drop`:
  eight group/plain × serviced/unserviced exit × close/drop cases. A synchronous
  control pipe prevents the leader exiting until its grandchild is pinned; actual
  leader exit precedes close/Drop, and the grandchild must remain running afterward.
- Windows **UNRUN** `detached_children_skip_lifetime_job_but_keep_explicit_group_kill`:
  query real job membership of suspended children, assert live state, then execute
  real process/group termination and wait for exit. Existing child/grandchild group
  kill and 400 kill/close race cases remain intact.
- Native **PASS** `detached_process_starts_a_new_unix_session`: two real children
  report PID/PGID/SID; detached must equal its PID, ordinary must inherit the host
  session/group. It also proves default false, windows_hide ignored, exact EOF/exit,
  and referenced child liveness.

## 4. Independent synchronous read/write FIFOs

**Reference:** libuv [`tty.c`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/tty.c)
queues line reads independently of `uv__tty_write`. The synchronous pipe paths in
[`pipe.c`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/pipe.c#L1242-L1519)
likewise have a read worker and a separate non-overlapped write queue.

**Implementation:** reserve two workers at adoption, use the existing two intrusive
operation FIFOs, and select the matching worker for start, completion and cancel.
Each worker owns its own active/cancel/result state and duplicated handles. Both
workers join before release/detach/Drop. Reads cannot block writes or cancel them.
Regular files preserve order per direction; cross-direction shared-offset order
is intentionally unspecified. Resource setup allocates; operations do not create
threads, buffers, queues or worker state.

**Tests (Windows runtime UNRUN):**

- `windows_duplex_fifos_make_independent_progress_without_allocating`: a real sync
  duplex pipe opposite an overlapped peer queues 32 idle reads before 32 writes.
  Every write and byte must arrive before the peer is allowed to reply. Replies
  then complete reads in exact FIFO order. Capacity-one output, exact IDs/tokens,
  immutable/provided buffers, positive allocator calibration, one warmup plus
  eight measured rounds, **512 read/write completions and zero Rust allocations
  across all threads**. Existing file/IPC allocation gates are unchanged.
- `duplex_cancellation_is_per_direction_and_close_drop_join_both_workers`: 32 cases
  force an idle read and a 1-MiB blocked write. Cancelling either direction must
  leave the other usable; later active and queued requests in both directions
  must acknowledge before Closed or join during Drop. Counts, bytes and post-Drop
  buffer mutation assert the subjects and quiescence.

## 5. Persistent integration errors and deterministic Drop

**Reference:** libuv [`uv__poll`](https://github.com/libuv/libuv/blob/v1.52.1/src/win/core.c#L426-L532)
treats non-timeout GQCSEx failure as fatal. turnloop exposes fallible host-driven
turns; a one-shot error must not silently disable the driver.

**Implementation:** helper errors retain the copyable portable kind and original
OS code. Every subsequent turn and integration call checks that state, including
the core path with queued timers/posts, and re-signals the host event. No retry
polling or per-error allocation is introduced. The pump reserves an entire batch
before dequeueing, retaining every packet even if shutdown races the wait.

Drop stops/joins the helper, recovers its retained packets, cancels operations,
and uses blocking direct IOCP waits for acknowledgement. It no longer yields in
a loop on an empty/dead helper queue. If the underlying port or teardown is truly
unrecoverable, the existing fail-stop abort preserves kernel buffer ownership;
synthetic pump failure with a healthy port can quiesce normally.

**Tests (Windows runtime UNRUN):**

- `pump_failure_repeats_before_queued_work_and_drop_joins_pending_io`: actual
  pending socket read and queued core completions, failure injected at the pump's
  wait boundary, synchronization proves error publication, eight repeated exact
  errors and event wakes, then Drop must finish within a separate-thread watchdog
  and release the unchanged caller buffer. The fault hook is test-only.
- `shutdown_preserves_every_packet_under_full_queue_backpressure`: prepost 128
  distinct packets, wait for measured full capacity, join the blocked helper,
  drain seven at a time and require every identity exactly once.

## Verification and unchanged rules

All required local gates **PASS**: formatting; strict default/all-feature native
workspace Clippy; the exact requested Windows core/contract/io Clippy; stable
workspace all-target/all-feature check; warning-free core/contract rustdoc;
no-tokio; the seven-day soak (251 locked versions, inherited rustls exception
expiring 2026-09-21 unchanged); path/feature-mode checks and source audit.
Linux x86_64/arm64, WASI p2/p3 and web core/contract/io cross-Clippy also **PASS**.
WASI p3 uses the existing lane's installed nightly-2026-09-07; Cargo reports
inherited manifest/config warnings, while the strict Rust lints pass.

Native workspace tests **PASS: 252 passed, 13 ignored**. The native CI runner
passes all three modes with **252 default / 259 executor / 308 all-feature tests**,
then independently tests every required member with its own feature selection.
The independently selected contract crate passes **51 / 58 / 58 tests**.
[Every expanded native CI command and executed count](iocp-semantics-commands.md)
is recorded separately. The new Unix session test executes in these runs; no
Windows test is counted as locally executed. Existing allocation/no-spin gates
are included in those successful native runs.

Source/document/reference inspection **PASS**. No applicable AGENTS.md; initial
working tree clean. The required documents and relevant core/Windows lane reports
were read. No dependency, lockfile, CI gate, soak policy, allocation threshold or
DESIGN.md changes. All original Windows test bodies are preserved apart from
explicit windows_hide on existing ProcessSpec fixtures; a source audit verifies
that equality, protected paths and absence of new unwrap(). Every unsafe block
passes the strict undocumented-unsafe-block Clippy gate.

Cross-compilation is compilation only. Windows/Linux runtime and Windows resource,
console, queue, allocation and no-spin measurements remain **UNRUN** here. WASI/web
runtime is **UNRUN in this lane**; their process capability stays Unsupported.
SQL real-server bodies and browser launches are **UNRUN (unrelated scope / known
sandbox limits)**. No empty/cfg-excluded/ignored test is counted as an execution.

Detailed command ledger follows; raw logs and invocations remain under
`.tools/iocp-semantics/`. Intermediate compile failures are retained: unused fault
hooks before their regression was added, a never_loop fixture, cancel's bool
misread as Result (two attempts), and a missing Duration import in the new Unix
test. Each was corrected without weakening any gate.

## Deviations / proposed DESIGN clarifications / next steps

- Record the subscribed-Hup handler-order limitation and system close budget in
  §7.3; no supported implementation can satisfy both original close requirements.
- Clarify per-direction workers/FIFOs for synchronous Windows handles, portable
  detached and Windows hiding options, and separate parent-lifetime/tree jobs.
- Clarify retained integration errors and blocking cancellation drain on Drop.
  DESIGN.md itself is unchanged; API/backend docs record the implemented decisions.
- Integrator: commit the coherent working tree and run windows-2025's required
  default/executor/all-feature CI. Relay real-close, duplex, process/job, pump
  failure and allocation results. All ten new Windows tests are UNRUN locally.
- No further local implementation decision is pending. If host-handler precedence
  is required even with subscribed Hup, the documented libuv policy must be changed
  explicitly to chain-and-return, sacrificing its cleanup window.

## Windows runtime handoff — UNRUN

Run from the repository root on windows-2025. These commands are **UNRUN (no
Windows host)**, even though every test target passes Windows cross-Clippy here.
The native CI runner covers the full required default/executor/all-feature modes;
the targeted commands below make each new Windows subject easy to rerun.

| Exact command on Windows | Status / subjects |
| --- | --- |
| `python3 scripts/ci/run-tests.py native` | **UNRUN** — all required Windows CI modes and executed-test checks |
| `cargo test --locked -p turnloop --lib backend::iocp -- --nocapture --test-threads=1` | **UNRUN** — close hold, lifetime membership/group kill, full queue and retained packets, existing IOCP units |
| `cargo test --locked -p turnloop --lib iocp_failure_tests -- --nocapture --test-threads=1` | **UNRUN** — repeated pump error, real pending read, queued core work, host event and bounded Drop |
| `cargo test --locked -p turnloop-contract --test windows_console -- --nocapture --test-threads=1` | **UNRUN** — eight console spawn modes, real Ctrl-C and real console close, existing console tests |
| `cargo test --locked -p turnloop-contract --test windows_lifetimes -- --nocapture` | **UNRUN** — parent death, normal leader release, duplex cancellation/close/Drop, existing resource tests |
| `cargo test --locked -p turnloop-contract --test allocations -- --nocapture --test-threads=1` | **UNRUN** — all-thread duplex FIFO zero-allocation gate and existing allocation gates |

## Exact verification commands

Repeated commands retain every invocation label and outcome.

| Command | Results |
| --- | --- |
| `cargo fmt --all` | fmt-implementation: **PASS**; fmt-process-tests: **PASS**; fmt-duplex-tests: **PASS**; fmt-signals: **PASS**; fmt-complete: **PASS**; fmt-final-apply: **PASS** |
| `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | windows-implementation: **FAIL**; windows-process-tests: **FAIL**; windows-duplex-tests: **FAIL**; windows-signals: **FAIL**; windows-complete: **PASS**; windows-retained-batches: **PASS**; windows-final: **PASS** |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | native-clippy: **FAIL**; native-clippy-fixed: **PASS** |
| `bash scripts/ci/no-tokio.sh` | no-tokio: **PASS** |
| `python3 scripts/ci/soak.py` | soak: **PASS** |
| `cargo +stable check --locked --workspace --all-targets --all-features` | stable: **PASS** |
| `cargo clippy --locked --target x86_64-unknown-linux-gnu -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | linux-x86: **PASS** |
| `cargo clippy --locked --target wasm32-wasip2 -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | wasi-p2: **PASS** |
| `python3 scripts/ci/check-paths.py` | paths: **PASS**; paths-final: **PASS** |
| `python3 scripts/ci/feature_modes.py` | feature-modes: **PASS** |
| `cargo clippy --locked --target aarch64-unknown-linux-gnu -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | linux-arm: **PASS** |
| `cargo +nightly-2026-09-07 clippy --locked --target wasm32-wasip3 -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | wasi-p3: **PASS** |
| `cargo clippy --locked --target wasm32-unknown-unknown -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | web: **PASS** |
| `cargo test --locked --workspace -- --test-threads=1` | native-workspace: **PASS** |
| `python3 .tools/iocp-semantics/audit.py` | source-audit: **PASS**; source-audit-final: **PASS** |
| `python3 scripts/ci/run-tests.py native` | native-ci-modes: **PASS** |
| `cargo fmt --check` | fmt-final-check: **PASS** |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | native-default-clippy: **PASS** |
| `env RUSTDOCFLAGS=-Dwarnings cargo doc --locked -p turnloop -p turnloop-contract --all-features --no-deps` | rustdoc: **PASS** |
| `git diff --check` | diff-final-check: **PASS** |

Read-only source/reference inspection (`rg`, `cat`, `sed`, `git status/diff/show`,
`rustup target list --installed`, `rustup toolchain list`, upstream downloads and
SHA-256 inventory): PASS. Two guessed read paths/globs and the first report patch
were rejected; corrected reads/patches succeeded. No rejected mutation changed files.
