# IOCP follow-ups — issues #8, #9 and #10

Base `7f37e7e` (`0.1.0-alpha.2`), branch `lane/iocp-followups`.
Development host: macOS arm64. Implementation and local verification complete.
Windows runtime **UNRUN (no Windows host)**. Base Windows CI
being green is historical evidence, not execution of these changes. No commits
or pushes: `.git` is read-only and the integrator owns those actions.

## #8 — backlog and busy-client retries

**Root cause:** one spare instance ignored `ListenOpts::backlog`; `Open::Pipe`
made one `CreateFileW` call and surfaced ERROR_PIPE_BUSY immediately.

**Fix:** listen reserves `max(backlog, 1)` slots, with a separate pinned OVERLAPPED
slab and retained teardown events. Every slot arms ConnectNamedPipe immediately.
Accept consumes one connected instance and replenishes that fixed slot. No Rust
allocation occurs during re-arm. Listener keys never repeat within a loop; retired
listener packets are discarded without dereferencing their addresses. Teardown
cancels private connects and joins kernel access before freeing their storage.

Busy clients open the local NPFS root and submit overlapped `FSCTL_PIPE_WAIT`
availability requests through `NtFsControlFile`. This is the asynchronous equivalent of WaitNamedPipeW,
using the existing IOCP wait and CancelIoEx. Retries follow actual availability;
there is no helper thread, retry tick, minimum wait floor or host-thread blocking
open loop. A competing client can win after readiness, so another native wait is
armed, with at most one open attempt per execution pass.

The existing API had no connection deadline. The additive, backend-neutral
`pipe_connect_until(name, deadline, token)` reserves a core deadline entry and
cancels at the absolute deadline. Terminal TimedOut waits for native cancellation
acknowledgement; original cancellation failures and the deadline survive for retry.
Success collected before expiration wins. Explicit cancellation/close before
expiry retains Cancelled/Closed. Existing pipe_connect stays unbounded/cancellable;
turn deadlines bound the turn itself. No backend trait or Open/Operation change.
An optional API clarification was asked; implementation proceeded with this stated
assumption while the question remained unanswered.

**Tests:**

- Three batches of eight real clients open and write their unique identity before
  an accept is submitted/serviced. All 24 are accepted once, including multishot
  delivery with capacity-one output, unique bytes and exact replies on each peer.
- Busy connect: 500 us / 2 ms / 10 ms deadlines, TimedOut exactly once, at most two
  turns / one empty wait; an unbounded connect demonstrably parks, succeeds after
  backlog refill, and another pending wait cancels before Closed.
- 32 listener/busy-client drop cycles, reused listener names/handle slots, queued
  retired packets, native availability waits and exact warmed handle baseline.
- Allocation gates: 128 accept/re-arm/close operations and 16 busy-wait timeouts
  after connection setup, absolute zero Rust allocations and positive work counts.
- Shared native success/expired/explicit-cancel contract, executed on macOS and
  instantiated on Windows. A synthetic core test delays acknowledgement, forces
  an exact cancellation error, then retries: no timeout can free pending state.

Primary references: [WaitNamedPipeW](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-waitnamedpipew),
[NtFsControlFile completion contexts](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntfscontrolfile),
[pipe-wait request layout](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/f030a3b9-539c-4c7b-a893-86b795b9b711).
The native SDK layout is taken directly from windows-sys; no copied implementation
or new dependency is used. Runtime behavior of NPFS and cancellation awaits Windows CI.

## #9 — same-port accepted pipes

**Root cause:** every accepted pipe was marked routed, including handles associated
with the accepting loop's own port. Pending I/O registered a thread-pool wait.

**Fix:** transports retain known port ownership. Same-loop acceptance/reattachment
uses direct IOCP without reassociation. Unknown imported handles attempt association;
only a foreign-port rejection selects the bridge. Foreign known associations retain
the event route. Partial IPC transfers prepare a bridge event only when routed.

**Tests:** a test-only thread-local counter increments at the actual registration
call. 128 forced-pending bidirectional exchanges, before/after same-loop reattach,
require zero registrations; moving the server to another port must register exactly
one wait and return exact bytes. A separate unassociated-import test verifies direct
native completion and zero registrations. The existing IPC zero-allocation gate is
unchanged and remains required. Counters add no production fields or work.

## #10 — Windows revision-2 contracts

**Root cause:** Unix contracts tested live-child close/drop, process options and
file FIFO/drop, while corresponding Windows cases were missing. Existing Windows
exit-watch allocation gates covered processes but not real console control events.

**Added coverage:**

- Close a live child without a prior kill, with/without a Job Object: assert live
  identity before close, exactly Cancelled then Closed under capacity-one output,
  signaled duplicated process handle at Closed, and no duplicate completions.
- Drop loops owning eight live children, half in jobs. Sixteen measured cycles
  prove 128 actual terminations, no blocking wait needed after drop, and exact
  warmed process handle counts. Existing child lifecycle tests remain intact.
- native_child echoes 15 length-delimited argv/env/cwd fields: empty arguments,
  embedded quotes, backslashes before quotes, trailing backslashes, spaces, tabs,
  newline, Japanese and a non-BMP character. The test also checks case-insensitive
  environment replacement, empty values, env_clear removing PATH, canonical Unicode
  cwd, exact stdout, one EOF and a successful exit.
- Queue 64 regular-file writes; verify FIFO OpId/token/byte order and all 2,048 file
  bytes. Drop with 64 provided reads submitted and queued buffers still pending,
  mutate the released storage, then independently read the full file with a fresh
  worker. A separate gate measures 256 queued writes with zero allocations.
- Deterministic console trigger exists: run the allocation test in an isolated
  CREATE_NEW_CONSOLE child and call GenerateConsoleCtrlEvent for Ctrl-C and Break.
  After warm-up and positive allocator calibration, a process-wide counter covers
  the real OS handler thread as well as the loop: 200 deliveries, two Stopped and
  two Closed completions require zero Rust allocations. The parent checks exact
  execution marker, positive libtest count and successful exit under a watchdog.
  Windows/ntdll's own heap is outside the Rust allocator; #9's actual-registration
  counter separately addresses the hidden bridge cost.

## Verification

Final local checks **PASS**. Windows runtime **UNRUN (no host)**. Cross-compilation
checks all Windows tests and unsafe comments but does not execute NPFS, process,
worker, or console behavior. The final native CI runner positively counted 251
workspace tests with default features, 258 with executor, and 307 with all features;
its independent contract runs counted 50 / 57 / 57. No failed test or gate remains.

Source inspection PASS: complete required design/contribution/integration/revision
and Windows lane documentation, every IOCP file, and all requested contracts. Initial
Git status clean; no applicable AGENTS.md. Read-only `rg`, `cat`, `sed`, Git diff
and Rust target inventory PASS. `cargo fmt --all` after each edit group PASS.
Source audit confirms all pre-existing allocation scenarios and Windows lifetime
contracts are retained unchanged, no new `unwrap()`, and no dependency/policy/gate edits.

The seven-day soak PASS covers 251 locked registry versions. The existing rustls
security exception (RUSTSEC-2026-0285, expires 2026-09-21) is unchanged. WASI p3 uses
the repository's installed `nightly-2026-09-07`; the pinned default is
`nightly-2026-08-20`, and stable is 1.97.1. P3 emitted inherited manifest readme
warnings; strict Rust/Clippy warnings passed without suppression.

### Command ledger

Commands run from the clone root. Identical commands are consolidated with every
invocation label/result. Full output and machine-readable invocations remain in
local `.tools/iocp-followups/*.log` and `commands.jsonl`; the reproducible command
and outcome record is committed with this report by the integrator.

Intermediate failures were corrected without suppressions: `windows-initial`
found an unused transitional resource field; `windows-bridge` an unused test import;
`windows-review` a collapsible conditional. `source-audit` failed because the local
inspection script searched for a nonexistent marker; its corrected marker verified
the complete original allocation scenarios byte-for-byte. These are recorded below.

| Command | Invocations and result |
|---|---|
| `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | windows-initial: **FAIL**; windows-connect: **PASS**; windows-contracts: **PASS**; windows-bridge: **FAIL**; windows-allocations: **PASS**; windows-review: **FAIL**; windows-final: **PASS**; windows-complete: **PASS**; windows-reviewed: **PASS**; windows-native-fsctl: **PASS** |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | native-clippy: **PASS**; native-final-clippy: **PASS**; native-reviewed-clippy: **PASS** |
| `cargo test --locked -p turnloop-contract --test native_surface local_connect_deadlines_complete_and_cancel -- --exact --nocapture` | native-deadlines: **PASS** |
| `cargo test --locked --workspace -- --test-threads=1` | native-tests: **PASS** |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | native-default-clippy: **PASS**; native-final-default-clippy: **PASS** |
| `cargo +stable check --locked --workspace --all-targets --all-features` | stable: **PASS**; stable-reviewed: **PASS** |
| `cargo clippy --locked --target wasm32-wasip2 -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | wasi-p2: **PASS**; wasi-p2-reviewed: **PASS** |
| `cargo +nightly-2026-09-07 clippy --locked --target wasm32-wasip3 -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | wasi-p3: **PASS**; wasi-p3-reviewed: **PASS** |
| `cargo clippy --locked --target wasm32-unknown-unknown -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | web: **PASS**; web-reviewed: **PASS** |
| `cargo clippy --locked --target x86_64-unknown-linux-gnu -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | linux: **PASS**; linux-reviewed: **PASS** |
| `bash scripts/ci/no-tokio.sh` | no-tokio: **PASS** |
| `python3 scripts/ci/soak.py` | soak: **PASS** |
| `python3 scripts/ci/check-paths.py` | paths: **PASS**; paths-final: **PASS** |
| `python3 scripts/ci/feature_modes.py` | feature-modes: **PASS** |
| `cargo test --locked -p turnloop --lib connection_deadline_waits_for_acknowledgement_and_retains_cancellation_errors -- --nocapture` | deadline-error-test: **PASS** |
| `python3 scripts/ci/run-tests.py native` | native-ci-modes: **PASS**; native-final-modes: **PASS** |
| `env RUSTDOCFLAGS=-Dwarnings cargo doc --locked -p turnloop -p turnloop-contract --all-features --no-deps` | rustdoc: **PASS** |
| `cargo fmt --all --check` | fmt: **PASS**; fmt-final: **PASS** |
| `rustc +stable --version` | stable-version: **PASS** |
| `python3 .tools/iocp-followups/audit.py` | source-audit: **FAIL**; source-audit-fixed: **PASS** |
| `git diff --check` | diff-final: **PASS**; diff-report: **PASS** |

### Commands expanded by the native CI runner

Both complete runner invocations passed. The table includes every expanded Cargo
command; positive executed-test counts are from the final run. Counts include
applicable doctests, with zero-test platform binaries excluded by the CI counter.

| Exact command | Result |
|---|---|
| `cargo +nightly-2026-08-20 metadata --format-version 1 --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml --no-deps` | PASS (metadata) |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml --workspace -- --test-threads=1` | PASS executed 251 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop -- --test-threads=1` | PASS executed 14 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-http -- --test-threads=1` | PASS executed 26 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-tls -- --test-threads=1` | PASS executed 11 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-mongodb -- --test-threads=1` | PASS executed 21 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-mysql -- --test-threads=1` | PASS executed 12 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-postgres -- --test-threads=1` | PASS executed 18 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-redis -- --test-threads=1` | PASS executed 10 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-smtp -- --test-threads=1` | PASS executed 12 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-websocket -- --test-threads=1` | PASS executed 3 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-io -- --test-threads=1` | PASS executed 5 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-contract -- --test-threads=1` | PASS executed 50 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml --workspace --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` | PASS executed 258 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop --features turnloop/executor -- --test-threads=1` | PASS executed 15 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-http -- --test-threads=1` | PASS executed 26 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-tls -- --test-threads=1` | PASS executed 11 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-mongodb -- --test-threads=1` | PASS executed 21 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-mysql -- --test-threads=1` | PASS executed 12 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-postgres -- --test-threads=1` | PASS executed 18 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-redis -- --test-threads=1` | PASS executed 10 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-smtp -- --test-threads=1` | PASS executed 12 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-websocket -- --test-threads=1` | PASS executed 3 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-io --features turnloop/executor -- --test-threads=1` | PASS executed 5 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-contract --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` | PASS executed 57 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml --workspace --all-features -- --test-threads=1` | PASS executed 307 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop --all-features -- --test-threads=1` | PASS executed 15 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-http --all-features -- --test-threads=1` | PASS executed 38 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-tls --all-features -- --test-threads=1` | PASS executed 14 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-mongodb --all-features -- --test-threads=1` | PASS executed 25 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-mysql --all-features -- --test-threads=1` | PASS executed 16 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-postgres --all-features -- --test-threads=1` | PASS executed 24 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-redis --all-features -- --test-threads=1` | PASS executed 13 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-smtp --all-features -- --test-threads=1` | PASS executed 16 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-websocket --all-features -- --test-threads=1` | PASS executed 8 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-io --all-features -- --test-threads=1` | PASS executed 5 tests |
| `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-followups/Cargo.toml -p turnloop-contract --all-features -- --test-threads=1` | PASS executed 57 tests |

### Runtime still required on the target hosts

| Command / subject | Result and reason |
|---|---|
| `python3 scripts/ci/run-tests.py native` on windows-2025 | **UNRUN (no Windows host)**; required default/executor/all-features runs, including every new contract and the unchanged IPC/exit allocation gates. |
| `cargo test --locked -p turnloop --lib backend::iocp::pipes::tests::accepted_pipe_uses_no_bridge_until_moved_to_another_port -- --exact --nocapture` | **UNRUN (Windows)**; must report one executed test and verify zero same-port / one foreign-port bridge registration. |
| `cargo test --locked -p turnloop --lib backend::iocp::pipes::unassociated_import_uses_direct_iocp_without_a_bridge -- --exact --nocapture` | **UNRUN (Windows)**; must report one executed test. |
| `cargo test --locked -p turnloop-contract --test windows_lifetimes -- --test-threads=1 --nocapture` | **UNRUN (Windows)**; backlog, deadlines, close/drop, argv/env/cwd, FIFO and exact handle counts. |
| `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1 --nocapture` | **UNRUN (Windows)**; new backlog/busy deadline, FIFO and isolated real-console gates plus all existing gates. |
| `python3 scripts/ci/run-tests.py native` on Linux | **UNRUN (no Linux host)**; cross-Clippy PASS only. |
| WASI 0.2/0.3 and browser runtime suites | **UNRUN in this lane**; all three core cross-Clippy targets PASS. Local pipes remain explicitly unsupported there. |
| SQL fixture servers / browser sandbox launches | **UNRUN (unrelated to this lane)**; known PostgreSQL shmget, MySQL initialization and Chrome sandbox limits retained. |

## Deviations, open questions and next steps

- No dependency, manifest, lockfile, seven-day soak, policy, gate or existing test
  assertion weakened or changed. Only windows-sys APIs are used on Windows.
- Additive pipe_connect_until and its preallocated core heap are the sole API scope
  extension, needed because the existing API had no deadline. DESIGN.md remains
  unchanged; revision-2 docs record the API and ownership semantics. Pending busy
  connections reject detach so the availability handle cannot escape without its
  retained name/wait state; established endpoints retain detach support.
- Listener release/destruction synchronizes cancellation of its private connects,
  analogous to existing worker/bridge teardown. Ordinary availability and accept
  progress use the single IOCP poll; no background polling/timer was introduced.
- Integrator: commit the coherent tree, run windows-2025 default/executor/all-features
  CI, and relay the new tests' results. Windows, Linux and WASI/web runtime are UNRUN
  locally; cross-Clippy is compilation only. SQL/browser fixture runs are unrelated
  and UNRUN in this lane (known sandbox limits retained).
