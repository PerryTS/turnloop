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

## sem-fix1

Base `6e5baf5`, PR #16; macOS arm64, 2026-09-15. Candidate fix and tests
implemented. **Windows runtime and exact kernel root cause remain UNRUN locally.**
Integrator reports windows-2025 run **34925939690** failed only
`windows_duplex_fifos_make_independent_progress_without_allocating`, in all three
modes. That report is pre-fix evidence, not a post-fix pass.

### Finding and selected fix

The workers duplicated one native file object, as the supplied hypothesis states.
However, each job also called **GetConsoleMode before ReadFile/WriteFile**, despite
`Detached::from_handle` already classifying the quiescent handle. This unnecessary
control-I/O call is a separate potential blocking point. The failing allocation
workload repeatedly starts writes beside an idle read; the reported passing
cancellation test alone does not identify which native call is blocked.

`Shared::console` now retains the classification supplied at adoption. Workers
select console-record reads or ordinary byte I/O directly. Console identity stays
fixed when tty flags change. This removes the redundant syscall for every adopted
synchronous kind, without changing the file object, buffer ownership, two FIFOs,
per-direction cancellation, completion publication, close, detach or joining.
No new worker, retry, cancellation, queue, allocation or wait is added per operation.
The original failing test and all its counts/thresholds are **byte-for-byte intact**.

This is a source-supported candidate, **not a claim that the macOS host verified
Windows kernel behavior**. The new raw diagnostic must confirm a pending ReadFile,
a blocked mode query and a direct WriteFile completing before the peer replies.
It releases the read and joins the threads before checking the progress result,
so a failure can distinguish the two hypotheses. If direct WriteFile also stalls,
this candidate is insufficient and the integrator must relay that result for a
kernel-I/O scheduling fix. No test was changed to accept stalled duplex writes.

### Research / alternatives

- [Microsoft's synchronous file-object contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwcreatefile)
  documents shared-object serialization. This supports investigating the supplied
  diagnosis; it does not establish which call stalled in this CI trace.
- [libuv v1.52.1 pipe.c](https://github.com/libuv/libuv/blob/v1.52.1/src/win/pipe.c):
  adoption queries file mode; the non-overlapped read worker uses a zero-byte
  ReadFile and writes run in a separate FIFO worker calling WriteFile directly.
  Neither worker reclassifies the pipe as a console. `uv__pipe_read_eof` stops
  reading and emits EOF; interrupt/stop uses CancelSynchronousIo and a yield loop
  to close the before-kernel-entry race. That loop is not copied into ordinary
  turnloop duplex progress.
- [libuv v1.52.1 tty.c](https://github.com/libuv/libuv/blob/v1.52.1/src/win/tty.c):
  `uv__tty_read_stop` wakes raw input with an injected record; line-input stop uses
  its read-console cancellation helper. These are specialized console paths,
  not a general mechanism for restarting arbitrary byte reads before writes.
- [CPython's original Windows-console implementation investigation](https://bugs.python.org/msg372536)
  identifies GetConsoleMode as NtDeviceIoControlFile and ordinary ReadFile/WriteFile
  as different NT calls. This motivates the diagnostic; it is not this lane's
  own Windows stack trace.
- [ReOpenFile](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-reopenfile)
  has original-object and sharing constraints; changing regular-file overlap also
  changes file-position management. No reopen preserving every adopted endpoint's
  identity and offset was established, so option (a) is not used speculatively.
- [CancelSynchronousIo](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelsynchronousio)
  may race a successful read and is not an acknowledgement. Option (b) would need
  an acknowledged restart protocol and protection against replaying consumed bytes;
  it is not justified until the direct-I/O diagnostic demonstrates it is needed.
  Option (c) is not used to relax the existing duplex pipe contract.

### Handle-kind audit and coverage

| Adopted synchronous kind | Behavior / coverage |
| --- | --- |
| Duplex named pipe | Same endpoint and independent byte FIFOs; original 32 idle reads before 32 writes, FIFO replies, 512 measured read/write completions and all-thread zero allocation preserved. New raw diagnostic separates data/control calls. Existing 32 cancellation/close/drop cases unchanged. |
| Anonymous pipe | Native ends are one-way; read and write endpoints each use cached non-console classification. New gate checks actual pipe types, exact bytes and 256 measured completions. Existing 128 cancellation/close/drop and fresh-byte cases unchanged. |
| Console input / screen output | Distinct native objects; cached console identity survives tty mode changes. Existing isolated mode/resize/UTF-8 test additionally writes Q while input is idle, requires its exact completion and reads Q from the real screen buffer in both close/drop cases. |
| Character devices | NUL positively exercises FILE_TYPE_CHAR, byte writes and EOF with 256 measured completions. Other synchronous character handles retain ReadFile/WriteFile and the device driver's native serialization/cancellation semantics; separate workers are not a guarantee about arbitrary drivers. Serial-port/third-party-device runtime is UNRUN (hardware unavailable). |
| Regular files | Shared file position retained; reads at EOF complete instead of waiting for append. New gate checks FILE_TYPE_DISK, 256 measured EOF/write completions and all 129 persisted bytes. Existing FIFO, drop and allocation workloads unchanged. Other overlapped files remain Unsupported. |

New allocation workload runs one warmup and 128 measured rounds per kind, checks
exact identities, positive allocator calibration and **zero allocations on all
Rust threads**: 768 measured completions. No pre-existing gate is reduced.
`idle_synchronous_pipe_reader_does_not_spin` adds 60 expiries at 500 us / 2 ms /
10 ms, with the existing ≤2 turns / ≤1 empty wait bounds, actual OS waits,
no early expiry and exactly one final read-cancellation acknowledgement.
These are Windows tests: compilation is not execution.

### Verification

Final local gates **PASS**: fmt, strict native workspace Clippy in default/all
features, strict Windows core/contract/io and whole-workspace all-target/all-feature
Clippy, stable workspace all-target/all-feature check, core/contract rustdoc,
no-tokio, seven-day soak (251 versions; inherited rustls exception unchanged),
path/feature gates, source-preservation audit and whitespace check.

Native CI passed all three modes: **252 default / 259 executor / 308 all-feature**
workspace tests, plus every independent member check. The independent contract
counts are **51 / 58 / 58**. Default/executor ignore 13 service tests and all
features ignore 20; these are UNRUN, not passes. Existing native allocation and
no-spin gates executed. All new/strengthened Windows subjects remain **UNRUN**.

Corrected intermediate failures are retained: Win32 constants imported from the
wrong module; an incomplete diagnostic edit; unsafe comments outside assertion
macros; and the initial direct Zig target spelling. No assertion or lint was
suppressed to obtain a pass. See the exact ledger below and `.tools/sem-fix1/` logs.

### Deviations / proposed DESIGN clarification

No dependency, lockfile, seven-day policy, security exception, CI requirement,
allocation threshold or existing no-spin assertion changed. No new I/O unwrap or
production unsafe block. DESIGN.md is unchanged. Proposed §7.3 clarification:
classify adopted synchronous handles while quiescent and retain that identity;
per-direction queues do not override arbitrary device-driver serialization.
Also qualify the blanket statement that synchronous files cannot be reopened:
ReOpenFile can reopen files under its access/sharing constraints, but preserving
all imported pipe endpoints and shared file-position behavior is not established.
The backend README records the type-specific behavior and remaining runtime gap.

### Open questions / next steps

1. Integrator: commit the coherent tree and run the Windows commands below;
   `.git` is read-only here. Confirm the raw diagnostic and original failure before
   treating the candidate as a verified fix. An asynchronous request for these
   results was sent through this conversation while local checks continued.
2. **UNRUN (Windows):**
   `cargo test --locked -p turnloop-contract --test windows_lifetimes -- --test-threads=1 --nocapture`;
   includes the raw diagnostic, synchronous no-spin and existing lifecycle cases.
3. **UNRUN (Windows):**
   `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1 --nocapture`;
   original duplex gate plus the new three-kind gate and all original workloads.
4. **UNRUN (Windows):**
   `cargo test --locked -p turnloop-contract --test windows_console -- --test-threads=1 --nocapture`.
5. **UNRUN (Windows):** `python3 scripts/ci/run-tests.py native` for the full required
   default/executor/all-feature matrix. Linux runtime has no host. WASI/web runtime
   and SQL servers are UNRUN in this Windows-only lane; no capability/gate changed.

### sem-fix1 verification commands

Every invocation is retained, including corrected intermediate failures.

| Invocation | Result | Exact command |
| --- | --- | --- |
| fmt-first | **PASS** | `cargo fmt --all` |
| windows-first | **FAIL** | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-tests | **PASS** | `cargo fmt --all` |
| windows-tests | **FAIL** | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-complete | **PASS** | `cargo fmt --all` |
| windows-complete | **FAIL** | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| windows-fixed | **PASS** | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| no-tokio | **PASS** | `bash scripts/ci/no-tokio.sh` |
| soak | **PASS** | `python3 scripts/ci/soak.py` |
| features | **PASS** | `python3 scripts/ci/feature_modes.py` |
| paths | **PASS** | `python3 scripts/ci/check-paths.py` |
| native-clippy | **PASS** | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| native-all-clippy | **PASS** | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| stable | **PASS** | `cargo +stable check --locked --workspace --all-targets --all-features` |
| windows-workspace | **FAIL** | `env 'CC_x86_64_pc_windows_msvc=zig cc -target x86_64-windows-gnu' AR_x86_64_pc_windows_msvc=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/iocp-semantics/.tools/sem-fix1/zig-global ZIG_LOCAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/iocp-semantics/.tools/sem-fix1/zig-local cargo clippy --locked --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-final | **PASS** | `cargo fmt --all` |
| fmt-check | **PASS** | `cargo fmt --check` |
| windows-workspace-wrapper | **PASS** | `env CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/iocp-semantics/.tools/sem-fix1/clang-windows AR_x86_64_pc_windows_msvc=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/iocp-semantics/.tools/sem-fix1/zig-global ZIG_LOCAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/iocp-semantics/.tools/sem-fix1/zig-local cargo clippy --locked --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| rustdoc | **PASS** | `env RUSTDOCFLAGS=-Dwarnings cargo doc --locked -p turnloop -p turnloop-contract --all-features --no-deps` |
| audit | **PASS** | `python3 .tools/sem-fix1/audit.py` |
| whitespace | **PASS** | `git diff --check` |
| native-ci | **PASS** | `python3 scripts/ci/run-tests.py native` |
| fmt-final-check | **PASS** | `cargo fmt --check` |
| report-whitespace | **PASS** | `git diff --check` |

All native CI subprocesses (counts exclude ignored and cfg-excluded tests):

| Exact command | Result |
| --- | --- |
| `cargo +nightly-2026-08-20 metadata --format-version 1 --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --no-deps` | **PASS metadata** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --workspace -- --test-threads=1` | **PASS executed 252 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop -- --test-threads=1` | **PASS executed 14 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-http -- --test-threads=1` | **PASS executed 26 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-tls -- --test-threads=1` | **PASS executed 11 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mongodb -- --test-threads=1` | **PASS executed 21 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mysql -- --test-threads=1` | **PASS executed 12 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-postgres -- --test-threads=1` | **PASS executed 18 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-redis -- --test-threads=1` | **PASS executed 10 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-smtp -- --test-threads=1` | **PASS executed 12 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-websocket -- --test-threads=1` | **PASS executed 3 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-io -- --test-threads=1` | **PASS executed 5 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-contract -- --test-threads=1` | **PASS executed 51 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --workspace --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` | **PASS executed 259 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop --features turnloop/executor -- --test-threads=1` | **PASS executed 15 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-http -- --test-threads=1` | **PASS executed 26 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-tls -- --test-threads=1` | **PASS executed 11 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mongodb -- --test-threads=1` | **PASS executed 21 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mysql -- --test-threads=1` | **PASS executed 12 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-postgres -- --test-threads=1` | **PASS executed 18 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-redis -- --test-threads=1` | **PASS executed 10 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-smtp -- --test-threads=1` | **PASS executed 12 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-websocket -- --test-threads=1` | **PASS executed 3 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-io --features turnloop/executor -- --test-threads=1` | **PASS executed 5 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-contract --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` | **PASS executed 58 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --workspace --all-features -- --test-threads=1` | **PASS executed 308 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop --all-features -- --test-threads=1` | **PASS executed 15 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-http --all-features -- --test-threads=1` | **PASS executed 38 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-tls --all-features -- --test-threads=1` | **PASS executed 14 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mongodb --all-features -- --test-threads=1` | **PASS executed 25 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mysql --all-features -- --test-threads=1` | **PASS executed 16 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-postgres --all-features -- --test-threads=1` | **PASS executed 24 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-redis --all-features -- --test-threads=1` | **PASS executed 13 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-smtp --all-features -- --test-threads=1` | **PASS executed 16 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-websocket --all-features -- --test-threads=1` | **PASS executed 8 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-io --all-features -- --test-threads=1` | **PASS executed 5 tests** |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-contract --all-features -- --test-threads=1` | **PASS executed 58 tests** |

Read-only source/API inspection (`rg`, `cat`, `sed`, `git status/diff/show`,
upstream API opens/searches and the CPython message download), initial clean-tree
check, preserved-function comparison and dependency/no-new-unwrap audit: **PASS**.
The final scripted audit independently checks the preserved functions. No Windows
code, ignored service test or empty native harness is counted as runtime coverage.

The first full-workspace Windows attempt failed because cc-rs supplied an LLVM
target spelling that Zig rejects. The successful attempt uses this local executable
C compiler wrapper (the exact environment and command are recorded above):

```python
#!/usr/bin/env python3
import os
import sys
args = [arg for arg in sys.argv[1:] if arg != '--target=x86_64-pc-windows-msvc']
os.execv('/opt/homebrew/bin/zig', ['zig', 'cc', '-target', 'x86_64-windows-gnu', *args])
```

Rust still checks the MSVC target; installed Zig supplies Windows C headers and
COFF dependency objects for cross-Clippy. This is not MSVC linking or execution.
The wrapper and its caches are checkout-local; no source/dependency/gate was
changed to obtain that pass.

## sem-fix2

Base `a6a345d` (sem-fix1), PR #16, 2026-09-15. Windows runtime evidence comes from
GitHub windows-2025 runs; raw kernel/session probes ran on throwaway `diag/*`
branches (`diag/sem-kernel`, `diag/sem-fix2`, deleted afterwards) and are quoted
below. Commits: `c10097a`, `e96c475`, `ba53146`, `3204fd1`, `c0029a9` and this
report commit.

### CI reports every failing test

`cargo test` stopped at the first failing test binary, so `allocations` hid
`windows_console` and `windows_lifetimes`. `scripts/ci/run-tests.py native` now
passes `--no-fail-fast` to the workspace and per-member commands; cargo's non-zero
exit still raises and the positive passed-count check is unchanged
(`scripts/ci/test_native_selection.py` asserts the flag on every native command).
Run **34930832074** (a6a345d + this change) then showed the complete failing set in
all three modes: the duplex allocation gate, both new console tests,
`duplex_cancellation_is_per_direction_and_close_drop_join_both_workers` and the
sem-fix1 diagnostic. Every other Windows test, including the never-before-run
lifetime/job tests, passed.

### Root cause 1: one synchronous file object serializes all I/O

sem-fix1's hypothesis did not hold: in run 34930832074 the diagnostic failed on
"GetConsoleMode did not reproduce the serialization point" (the query on a pipe
returned without waiting). Raw probe, run **34930861414** (repeated in 34932207539),
two duplicates of one synchronous duplex named-pipe client:

```
reader IO pending flag = 1
WriteFile beside idle read: still blocked after 1.5s
writer IO pending flag while waiting = 0
CancelSynchronousIo(writer waiting behind read) -> 0 (Element not found. (os error 1168))
CancelSynchronousIo(idle reader) -> 1 (ok)
reader after cancel: returned ok=0 err=995 n=0
writer after reader cancel: returned ok=1 err=0 n=1
reissued read receives every server byte: returned ok=1 err=0 n=2 head=[77, 78]
```

The write waits for the file object's I/O lock with no request of its own (so it
cannot be cancelled); cancelling the idle read releases it and consumes no data.
libuv's `uv_pipe_getsockname` documents the same serialization for its
non-overlapped pipes. Alternatives, measured (run 34932207539):

- ReOpenFile with FILE_FLAG_OVERLAPPED fails with ERROR_PIPE_BUSY (231) for a
  named-pipe client end, server end, and both anonymous ends. No second object.
- `FSCTL_PIPE_ASSIGN_EVENT` (readiness without a pending read) returns
  STATUS_NOT_SUPPORTED (0xC00000BB).
- Preempting a write is unsafe: a 1-MiB write blocked on quota, after the peer
  consumed 1000 bytes, returned `ok=0 err=995 n=0` on cancellation and the other
  1047576 bytes were discarded (`server available after write cancellation = 0`).
- A read waiting behind a quota-blocked write is not released by data arriving
  (`ReadFile beside blocked write with 1 byte available: still blocked`) nor by
  CancelSynchronousIo (ERROR_NOT_FOUND).

**Fix (`sync_io.rs`).** The two direction workers share one state. For synchronous
pipe handles with both read and write access, the write worker sets `writing`,
cancels an in-kernel read with CancelSynchronousIo and waits until it returns; the
read worker treats an aborted preempted read as not consumed and reissues the same
request after the write, so read FIFO order and exactly-once completion hold. A
read that starts while a pipe write owns the object parks on the condition variable
instead of entering ReadFile, so its cancellation needs no kernel call. Consoles,
disks, other character devices and one-way pipe ends (every anonymous pipe; the
other direction fails its access check first) are not preempted. No allocation,
thread or queue is added per operation.

**Lost cancellation.** A CancelSynchronousIo issued just after a named-pipe read
enters the kernel can report success yet leave the read pending (run 34932207539:
`CancelSynchronousIo returned TRUE but ReadFile did not return; pending flag=1`). Run **34932652249**,
300 immediate cancellations: `histogram=[0, 3, 297]` successful calls per read, all
300 aborted, no bytes lost, worst 25.5 ms. Both the cancellation helper (which
previously waited forever after one success) and the preemption now re-issue after
at most 10 ms while the worker is still inside I/O; before kernel entry they yield,
sleeping 1 ms only after 64 misses (only a foreign holder of the object's I/O lock
can cause that).

**Documented per-kind limitation.** A pipe write waiting for the peer to drain still
owns the object, so reads on that endpoint complete only after it finishes or is
cancelled (evidence above; independence is impossible without an independent
object, which the kernel does not provide). Idle reads never delay writes.

Tests:
- `windows_duplex_fifos_make_independent_progress_without_allocating`: unchanged,
  passes in all modes (512 completions, zero allocations on all threads).
- The sem-fix1 diagnostic is replaced by
  `synchronous_pipe_write_waits_behind_idle_read_until_the_read_is_cancelled`,
  asserting the confirmed kernel behaviour above (blocked write, no writer request,
  ERROR_NOT_FOUND for the writer, aborted read with 0 bytes, write released, every
  later byte received).
- `duplex_cancellation_is_per_direction_and_close_drop_join_both_workers`: the
  cancel-read case previously required a new read to complete beside a still
  blocked 1-MiB write, which the kernel forbids. It now first proves the write is
  inside WriteFile (peer receives bytes), cancels the read, then drains every
  surviving write byte (all 0x33, full count) and requires the queued read to
  complete with the peer's byte after the write. Close still cancels a
  kernel-blocked write, a parked read and two queued requests (4 cancellations in
  both cases; the surviving write now completes instead of being cancelled).
  An executor-mode run of the first restructure (34933660415) failed "read overtook
  the write" because starting a request does not schedule its worker thread; a
  probe showed a blocked synchronous 1-MiB WriteFile always reports the full count
  (run 34934771562), so the test pins the in-kernel state instead.
- New `duplex_writes_preempt_reads_at_every_entry_point_without_losing_bytes`:
  384 request/response rounds with the read queued in the same turn, one turn
  earlier, or settled 2 ms, checking every byte and each completion once.

### Root cause 2: CREATE_NEW_CONSOLE is a pseudoconsole on windows-2025

Run **34932207539**, session probe: steps run in session 2, `WinSta0`, visible. An
isolated CREATE_NEW_CONSOLE child's console window has class `PseudoConsoleWindow`,
owned by `Microsoft.WindowsTerminal_1.23.20211.0_x64__8wekyb3d8bbwe\OpenConsole.exe`
(default-terminal handoff). `PostMessageW(WM_CLOSE)` and `WM_SYSCOMMAND/SC_CLOSE`
succeed, the hidden window is destroyed (`IsWindow` 0) and no control event reaches
the process (killed after 8 s). `ClosePseudoConsole` on a pseudoconsole child
delivered `control-event:2` (CTRL_CLOSE_EVENT) and exit 0xC000013A.

`real_console_close_chains_without_hup_and_allows_subscribed_cleanup` now hosts both
fixtures (`chain`, `hup`) in a pseudoconsole and closes it: a real OS
CTRL_CLOSE_EVENT on hosted and self-hosted runners alike; these cases always run.
The original WM_CLOSE cases are kept for a classic `ConsoleWindowClass` window; on
any other window class each is skipped with a message written directly to stderr
(visible in CI output), e.g. `SKIP ... NewConsoleWindow/chain: console window class
"PseudoConsoleWindow" does not turn WM_CLOSE into CTRL_CLOSE_EVENT` (run
34934771562). Markers moved to a named pipe because stdout is the console under a
pseudoconsole. **Capability-gated:** the two WM_CLOSE cases, on hosted runners.
They remain runnable on a self-hosted runner with classic conhost; not yet observed.

### Root cause 3 (backend bug): removing the console handler deadlocked close cleanup

With a real close, the `hup` fixture was killed with 0xC000013A instead of exiting
23 (run 34933660415). Probe, run **34934409445**: while one thread is inside a
console control handler, `SetConsoleCtrlHandler` from another thread blocks —
`add another handler while the handler runs: BLOCKED for 1.5 s ... returned 1 after
release`, same for removing the running handler. The subscribed Hup handler never
returns (libuv's hold), and the last subscription's Drop removed it, so dropping
the loop — the cleanup the hold exists for — deadlocked. `signals.rs` now installs
the handler once and never removes it, as libuv's `uv__signals_init` does; without
a matching subscription it returns FALSE, exactly as if absent. The pseudoconsole
`hup` case then exits 23 with its marker (run 34934771562 and the PR run below).

### Root cause 4: inherited CTRL+C-ignore, and CREATE_NO_WINDOW semantics

Run 34932207539: the cargo test process and its CREATE_NEW_CONSOLE child have
process-parameter ConsoleFlags `0x1` (CTRL+C ignored, inherited from the runner).
In the child, `ctrl-c without enabling: handler calls=0`, Ctrl-Break 1, after
`SetConsoleCtrlHandler(NULL, FALSE)` flags 0x0 and Ctrl-C 1. libuv/Node also honor
the inherited flag, so `windows_hide_and_detached_match_console_inheritance` now
establishes the precondition in its isolated host, as the fan-out test already did.
That exposed the next iteration (run 34934409445):
`hide=true inherited=0 ... output="console:true,show:0"`. CREATE_NO_WINDOW (applied
exactly as libuv does) gives the child its own windowless console; the probe
treated having any console as sharing the host's. `console-probe` now receives the
host PID and reports membership in the host's console plus whether it has its own
console; the test additionally asserts that every non-detached child has a console
and detached ones do not. All eight combinations pass (run 34934771562).

### Verification

Local (macOS arm64): `cargo fmt --check` PASS; `cargo clippy --locked --target
x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets
--all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` PASS; `cargo
clippy --locked --workspace --all-targets -- -D warnings` PASS; `cargo test --locked
-p turnloop -p turnloop-contract -- --test-threads=1` PASS; `python3 -m unittest
test_native_selection` PASS.

windows-2025 PR run **34934941536** (commit `c0029a9`): **success**, every job
green (self-hosted-windows skipped: `vars.SELF_HOSTED_WINDOWS` unset). Workspace
counts: default **289**, executor **296**, all-features **345** tests executed, plus
every member. In all three modes: the duplex allocation gate, the kernel
serialization test, both duplex tests, idle no-spin, both console tests and all
lifetime/job tests pass. Twelve executor-mode repetitions of both duplex tests
passed on the diag branch (34934771562). The final report commit only changes this
document and the skip logging; see the PR checks for its run.

### Open items

- WM_CLOSE on a classic conhost window is untested here (capability-gated on hosted
  runners; runnable on the self-hosted Windows runner).
- Two early probe variants (`diag/sem-kernel`, runs 34932652249 and 34933725475)
  hung while racing immediate cancellations on an anonymous pipe, without printing
  where; a traced rerun (34934409445) aborted all 20 reads promptly and did not
  reproduce it. Anonymous pipes are never
  preempted; user cancellation of their reads is covered by the existing
  128-round `cancelled_synchronous_reads_release_buffers_and_threads`, which passes.
- If a foreign thread holds a synchronous pipe's I/O lock, preemption or
  cancellation retries every 1 ms until that I/O ends (documented; no turnloop
  thread can cause it).
- DESIGN §7.3 clarification proposed: per-direction FIFOs on synchronous pipes
  give idle-read preemption, not concurrent kernel I/O; the console control handler
  is process-lifetime once installed.
