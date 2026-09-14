# core2 lane report

Updated 2026-09-14. Native scope implemented; macOS runtime verification passes.
Release is blocked by the pre-existing protocol test dependency's newly published
rustls advisory, described below. Linux/Windows runtime tests remain UNRUN.

Read completely: `DESIGN.md`, `CONTRIBUTING.md`, `docs/INTEGRATION_REPORT.md`, and
`docs/lanes/{core,windows,wasm}.md`. No applicable `AGENTS.md`. Git remains read-only;
the integrator has periodically checkpointed the tree. No commits/tags or pushes
were attempted. Wave-1 history remains in the integration and lane reports.

## Implemented

- AF_UNIX stream listen/connect/accept with `PipeName`; `open_stdio` classifies
  pipes, regular files and TTYs; framed SCM_RIGHTS sending/receiving retains
  independent socket ownership and supports detach/attach across loops/processes.
- Processes with argv, environment, cwd, inherit/null/pipe/existing-handle stdio,
  uid/gid and isolated process groups. kqueue NOTE_EXIT plus SIGCHLD cooperation;
  Linux pidfd with forced/testable SIGCHLD fallback. Only owned children are reaped.
  `prepare_close` initiates termination without blocking; cancellation and Closed
  follow reaping through ordinary turns. Loop destruction also kills/reaps live
  owned children. Group termination includes the fixture's live grandchild.
- One lazy process-wide signal dispatcher; per-loop fan-out, coalescing, stop and
  original-disposition restoration. Child registrations share SIGCHLD ownership.
- TTY Normal/Raw/Io, dimensions, SIGWINCH resize subscription and full saved-mode
  restoration on close/drop, including transferred transport ownership.
- One generic external-wait helper, fixed 16,384 registrations, host-owned atomic
  condition support, notifications, inequality, exact deadlines and cancellation.
- Regular-file stdio on the existing bounded blocking pool, fixed reusable jobs,
  per-file FIFO and worker quiescence before provided buffers can be reused.
- Optional `executor`: `!Send LocalExecutor`, tokens to wakers, fixed operation
  staging, TCP/UDP/local-pipe/stdio futures-io adapters, sleep/timeout, local spawn,
  JoinHandle cancellation, and drop-cancels-I/O. Buffered writes report acceptance;
  flush/close confirms native completion. Pending calls may replace caller buffers.
- Public rustdoc throughout `crates/turnloop`, enforced with `deny(missing_docs)`,
  two executable loop/executor examples, updated crate README, and the full
  [Backend revision 2 handoff](docs/BACKEND_REVISION_2.md).

The starting main already called the Backend revision 2 for empty-wait counters.
This lane extends that revision with new Open/Operation/Outcome variants, optional
native service methods, notifier injection and nonblocking `prepare_close`.
The handoff documents every addition, ownership and Windows/WASM integration.
Existing production backends and portable core/executor compile. Standalone IOCP
and WASM spikes were not altered or represented as production implementations.

Only one dependency was added: optional `futures-io = 0.3.31`. No additional futures
executor/utility dependency; the remaining machinery uses std. The seven-day soak
and all banned runtime/dependency gates remain unchanged.

## Runtime coverage

The final native runner executes **110 default-feature workspace tests** and
**118 all-feature workspace tests**; its independent contract passes execute
**42 default / 49 all-feature tests**. Ignored protocol-server tests are not counted.

Core/contract all-feature coverage includes 5 core unit tests, 22 existing shared
contracts, 14 native surface tests, 6 executor tests, 6 allocation gates, the
existing descriptor lifetime test, and 2 executable rustdoc examples.

New subjects asserted: local echo bytes and both accept/connect tokens; child-loop
stdio bytes on all three streams and regular-file/null stdio; socket transfer to
and back from a child; exact exits and reaping including deterministic
exit-before-registration (`waitid(WNOWAIT)`), 256 concurrent children, direct kill,
close/drop and a live grandchild group; four-loop/four-thread signal fan-out;
SIGCHLD/user-subscription cooperation; full openpty mode restoration and resize;
1,024 notified waits across four loops plus inequality/deadline/cancellation;
64-connection executor echo; timer/timeout timing; UDP/stdio traffic; pending read
buffer replacement/shrinking; pending write buffer replacement; task cancellation
before first poll; drop of pending I/O and unconsumed accepted sockets.

No-spin contracts preserve the original limits: idle future timers at 0.5/2/10 ms,
≤2 turns and ≤1 zero-event wait per expiry. The new process/signal case proves
60 real timer expiries with both services registered; existing socket/timer,
queued-completion and notifier syscall gates remain intact.

Allocation counters measure the loop thread after warm-up, with exact byte/event
counts: existing read/write/accept/timer/backlog gates, 200 IPC/socket transfers
and cancelled waits, 200 measured file read/write cycles, 1,000 measured executor
I/O/sleep cycles, and 16 child exits plus 200 signals and 200 notified waits.
All six gates report zero allocations. Construction, owned caller payloads,
process spawn/signal subscription setup and task spawning allocate; no claim of
zero setup allocation or a global allocator measurement of helper threads is made.

## Verification commands

[docs/core2-commands.md](docs/core2-commands.md) records every scripted verification
invocation with PASS/FAIL and its raw log identifier. Logs are in `.tools/core2/`.
Intermediate failures are retained, including compiler/type errors and runtime
bugs subsequently fixed. Formatting during editing, source reads/searches and
read-only Git inspection also succeeded; the unwrap search had no matches.

| Command / scope | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| Same Clippy with `--all-features` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS, stable 1.97.1 |
| `cargo test --workspace -- --test-threads=1` | PASS; serialization follows CONTRIBUTING for process/signal/allocator isolation |
| `python3 scripts/ci/run-tests.py native` | PASS, both workspace configurations and independent contract passes |
| `cargo test -p turnloop -p turnloop-contract --all-features -- --test-threads=1` | PASS, including both doctests |
| `cargo test -p turnloop-contract --all-features --test allocations -- --test-threads=1` | PASS, all six gates |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --all-features --no-deps` | PASS |
| `cargo rustc -p turnloop --lib --all-features -- -Dmissing_docs` | PASS; lint subsequently made a crate-level requirement |
| Linux x86_64 workspace/all-target Clippy, default and all features | PASS; includes pidfd and forced SIGCHLD/timerfd branches |
| Windows MSVC core/contract/bench all-target/all-feature Clippy | PASS |
| WASI 0.2 and web workspace/all-target/all-feature Clippy | PASS |
| WASI 0.3 core/contract/bench all-target/all-feature Clippy, nightly-2026-09-07 | PASS; Cargo emits existing manifest/config warnings; compiler warnings denied |
| iOS arm64 core/contract all-target/all-feature Clippy | PASS |
| FreeBSD x86_64 core/contract all-target/all-feature Clippy | PASS after installing its target and fixing test PID width |
| Android arm64 core/contract library/all-feature Clippy | PASS |
| Android arm64 all-target Clippy | FAIL: pinned Clippy diagnoses `missing_const_for_thread_local` in the existing allocation-test TLS macro despite both initializers already being `const`; no lint suppressed |
| `bash scripts/ci/no-tokio.sh` | PASS, all 8 configured target graphs with default/all features |
| `python3 scripts/ci/soak.py` | PASS, 194 locked registry versions; resolver policy active |
| `python3 scripts/ci/run-tests.py loom` | PASS, 5 production notifier/queue/pool models |
| `python3 scripts/ci/run-tests.py miri` | Initial FAIL: default sysroot cache outside sandbox writable paths. PASS after setup and rerun with `MIRI_SYSROOT=$PWD/.tools/miri-sysroot`; 2 real pure-Rust tests |
| `python3 scripts/ci/install-tools.py cargo-deny actionlint zizmor shellcheck` | PASS, all four official artifact hashes verified |
| `cargo +nightly-2026-08-20 deny --locked check` with `.tools/bin` on PATH | FAIL: rustls advisory below; bans/licenses/sources PASS |
| `python3 scripts/ci/lint-workflows.py` with `.tools/bin` on PATH | PASS |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS, 15 automation tests |
| `git diff --check` and final `cargo fmt --check` | PASS |

Full target spellings, flags and interim results are preserved in the ledger.
Cross-compilation is never counted as runtime execution. No gate/test was weakened.

### Advisory gate blocker

`cargo-deny` fetched **RUSTSEC-2026-0285 / GHSA-2mjx-qc3c-rqvc**, published
2026-09-14. The existing protocol dev dependency `rustls 0.23.44` is affected;
`0.23.45` fixes it. The crates.io index records publication at
**2026-09-14 15:11:17 UTC**, so it is soak-eligible only at
**2026-09-21 15:11:17 UTC**. Existing `0.23.44` was published
2026-09-07 09:17:42 UTC. No advisory ignore, dependency-soak override, fabricated
version or unrelated protocol dependency migration was introduced.

Evidence: [upstream advisory](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc)
and the [official registry index](https://index.crates.io/ru/st/rustls).
The native core and executor do not depend on rustls.

### UNRUN

- Linux and Windows native runtime suites, allocation/no-spin/syscall gates and
  platform process races: no Linux/Windows host or Docker. Run the same generic
  contracts, plus platform-specific fixtures, when those hosts/providers are ready.
- FreeBSD, iOS and Android runtime suites: no corresponding runtime host/device.
- WASI/web shared native-service/executor runtime tests: production providers and
  shared platform test instantiations are still absent in this starting workspace.
  The external-wait thread service explicitly rejects WASM. No zero-test success
  is presented as platform runtime coverage; existing strict CI jobs remain on.
- PostgreSQL/MySQL server tests: UNRUN (known sandbox shmget/initializer limits).
  Other ignored protocol server suites were not rerun for this core lane and remain
  UNRUN here. Normal in-process protocol suites ran in workspace verification.
- Browser launch/integration, Linux instruction baseline/Callgrind and platform
  syscall traces: UNRUN in this environment; no substitute measurements supplied.
- Git commit/tag operations: UNRUN, metadata read-only; integrator owns checkpoints.

## Decisions, limitations and proposed DESIGN clarifications

No changes to authoritative `DESIGN.md`. Detailed proposals are in the revision 2
handoff: local control-stream framing and duplicate ownership; nonblocking
kill/reap-on-close with a process teardown hook; subscribed-signal disposition
ownership; TTY Raw/Io and restoration across transport movement; generic external
waits and buffered futures-io writes. These are concrete documented API choices.

Hosts own Unix socket-path cleanup and coordinate shared descriptor flags/TTY
state. They must not reap turnloop children or overwrite active subscribed signal
handlers. Group kill requires an owned unreaped leader, preventing PID/PGID reuse.
Loop destruction may wait for child termination and running file-job quiescence;
ordinary turns and close use completion acknowledgement. External waits have a
fixed process-wide 16,384-slot bound. UDP's AsyncRead view omits sender addresses.
Adapters buffer writes, so flush/close before drop is required to retain output.

Windows needs to implement `prepare_close` with its process/Job Object mechanism,
retain native operation storage until acknowledgement, supply the existing spike's
stdio/console mechanisms and instantiate the shared contracts. Signal scenarios
accept a platform-supported signal and real delivery callback; they do not hardcode
Unix-only signals for IOCP. Native-only capabilities on WASM remain explicit errors.

## Open questions and next steps

1. Integrator: review/checkpoint this coherent tree and Backend revision 2 handoff;
   coordinate the `prepare_close` addition with the Windows production port.
2. Run Linux default and all-feature suites (pidfd and forced SIGCHLD/timerfd),
   Windows IOCP shared contracts, and platform runtime/no-spin/allocation gates.
3. Once rustls 0.23.45 is soak-eligible, update the shared protocol dev pin/lock,
   rerun protocol TLS/server tests and `cargo-deny`. Until then, release remains
   blocked by the advisory gate; both security and soak policies stay enabled.
4. Resolve the pinned Android test-TLS Clippy diagnostic upstream or in a toolchain
   update; the production Android library cross-check already passes.
5. Review the documented API/specification clarifications. Relocated Miri, final
   Clippy, allocation, tree and format checks are complete; no remaining native
   feature implementation is deferred.
