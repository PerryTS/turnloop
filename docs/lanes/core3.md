# core4 lane report

Implementation and local verification are complete. **Linux runtime remains
UNRUN**; exact x86_64/arm64 integrator commands are below. Starting revision:
`b78d533`. Read DESIGN.md, CONTRIBUTING.md, docs/INTEGRATION_REPORT.md and the
core/CI lane reports completely; no applicable AGENTS.md. The prior root report
is preserved in Git history and `.tools/core4/inherited-core3-report.md`.
Integrator checkpoints appeared during work; this agent made no commits because
`.git` is read-only.

## Implemented

- **Timerfd accounting:** in this checkout, line 516 checks `zero_event_waits`;
  `os_waits` is line 515. Both epoll branches already report one OS wait. A private
  timerfd expiry returns a TIMER readiness event, so the old `n == 0` expression
  incorrectly reported zero empty waits. The fix excludes only private timeout
  events from the native-work count. Notifier and I/O events still count, even
  when they produce no user completion. The existing nanosecond one-shot plus
  single `epoll_wait(-1)` path is preserved; no polling, extra wait or timer floor
  was added. This is the defect demonstrated by source inspection; runtime proof
  on Linux must come from the integrator.
- **Regressions:** retained the original file-backpressure test verbatim. A new
  allocation gate checks 60 quiet expiries (20 each at 500 us, 2 ms, 10 ms), exact
  handle/op/token and Timer completion, `now() >= at`, one OS/empty wait, zero
  waits for queued Closed results, and zero allocations. Epoll unit tests assert
  timeout/notifier/I/O accounting, rearming after an abandoned timeout expires,
  Now/Forever handling and actual forced-timerfd selection.
- **SIGCHLD:** retained the fallback. A mode-specific epoll unit test proves
  forced SIGCHLD refuses pidfd registration. Every Linux mode runs all existing
  process/signal contracts, including 256 children, fan-out/churn, reaping,
  process-group cleanup, stdio, no-spin and allocation tests.
- **BTree isolation:** removed `turnloop/timer-btree` and moved its implementation
  and sorted-reference conformance test into the private benchmark crate. Only
  `turnloop-bench/timer-btree` remains, with no feature forwarding. Production
  always uses the preallocated 4-ary heap. Both benchmark variants execute nine
  workloads with positive operation counts and deadline/order checks.
- **Required modes:** Linux x86_64 and arm64 each get default, timerfd, SIGCHLD,
  combined fallbacks, executor and all-features arms. macOS/Windows each get
  default, executor and all-features: **18 native arms**, all unconditional,
  fail-fast disabled and required through `ci-gate`. Each runs the workspace,
  independently requires positive core/protocol/contract counts, and runs
  applicable Node/curl interop. Existing Windows pending-contract metadata and
  the WASI/web jobs are unchanged.
- **Feature coverage gate:** `scripts/ci/feature_modes.py` reads the manifest and
  explicit JSON flow rows in ci.yml (valid YAML). It rejects unlisted public
  features, including implicit optional-dependency features; all-features cannot
  grant implicit coverage. Missing/duplicate/optional/unwired mode arms, removed
  Linux combinations, unknown features and missing ci-gate dependencies fail.
  The runner consumes the same rows, with optional `--mode`. Workspace selections
  stay complete; independent member commands project features onto the member
  and its direct dependencies, as Cargo requires. Sans-IO members without a
  core dependency have no backend feature to select.

## Verification

[Every verification command and exit status](docs/core4-commands.md), including
intermediate failures, is recorded. Raw output: `.tools/core4/logs/`. PASS means
actual command success; cross-Clippy is compilation only.

| Command / group | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`, default and `--all-features` | PASS native |
| Same full-workspace Clippy with `--target x86_64-unknown-linux-gnu`, default and all features | PASS; Linux runtime UNRUN |
| Core/contract/bench all-target Linux x86_64 Clippy separately with `--features turnloop/epoll-timerfd` and `--features turnloop/process-sigchld` | PASS |
| Core/contract/bench all-target/all-feature Clippy for Linux arm64, Windows MSVC, WASI 0.2 and browser wasm | PASS |
| Same WASI 0.3 Clippy with `cargo +nightly-2026-09-07` | PASS; existing Cargo manifest/config warnings remain, Rust warning-denial passes |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS, stable 1.97.1 |
| `python3 scripts/ci/run-tests.py native` | PASS final: all three macOS modes, **1,174 test passes** including independent repetitions |
| Workspace tests inside that runner | PASS: default **223**, executor **231**, all features **239** |
| Independent contract runs inside that runner | PASS: default **46**, executor **53**, all features **53**; each preserves allocation/no-spin/process/signal gates |
| `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode MODE` for default/executor/all-features | PASS **17 tests per mode**; real Node/curl HTTP, TLS and WebSocket legs, cleanup completed |
| `cargo run --release --locked -p turnloop-bench -- --portable --timers`, then with `--features timer-btree` | PASS; nine workloads per variant, 100,000 asserted operations per workload; portable ns smoke only, no instruction-performance claim |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS **84 tests**, including missing-mode/new-feature/zero-test negative controls |
| `python3 scripts/ci/feature_modes.py` | PASS, **4 public features / 18 required arms** |
| Checksum-pinned actionlint/zizmor/ShellCheck installation and `python3 scripts/ci/lint-workflows.py` with `.tools/bin` on PATH | PASS; existing strictly validated actionlint queue compatibility wrapper preserved |
| `bash scripts/ci/no-tokio.sh` | PASS, eight targets plus union, default/all features |
| `python3 scripts/ci/soak.py` | PASS, 241 locked versions, only the existing exact rustls security exception |
| `python3 scripts/ci/check-paths.py`; `git diff --check` | PASS |
| `python3 .tools/core4/audit.py` | PASS: actual all-feature core graph has only default/executor/timerfd/SIGCHLD; all three native modes have independent positive counts and execute both allocation regressions and process/signal subjects; protected policy/baseline files unchanged |

The final native run executes both allocation regressions six times (workspace
and independent contract in each mode). The new test asserts **360 expiries**
with zero allocations across those executions. Ignored real-server tests are
UNRUN, never included in positive passed-test counts.

Linux/Windows runtime, hosted GitHub CI/ci-gate execution and Linux instruction
measurements are **UNRUN (no host)**. WASI/browser production backend runtime is
**UNRUN in this lane**; no cross-check substitutes for it. SQL real-server bodies
remain **UNRUN (sandbox)** and were not retried here. The pre-existing intentional
curl-unavailable test logs one UNRUN curl leg while proving its Node 100-stream
leg ran; ordinary curl HTTP/1 and HTTP/2 legs pass.

## Intermediate failures

The first macOS wrapper run passed default mode and the executor workspace, then
Cargo rejected `turnloop-contract/executor` when testing only `-p turnloop`.
The runner now applies the correct member/dependency feature scope; all 84 Python
regressions and the complete native wrapper pass after that fix. No test threshold
or positive-count gate was relaxed. The ledger retains the failed invocation.

Initial read-only inspection had an overbroad directory scan, a zsh loop variable
that shadowed PATH in that subprocess, and a few guessed nonexistent source/log
paths. Subsequent reads used discovered paths; these are inspection errors, not
verification passes. Formatting was applied once with `cargo fmt --all` and
subsequent formatting checks passed.

## Deviations / proposed DESIGN clarifications

- Define empty-wait instrumentation to exclude the backend's private timeout
  mechanism. A timerfd-only expiry is equivalent to a timed wait returning zero;
  notifier readiness remains native work. Backend and TurnInfo docs now state
  that normalization. The Backend trait shape and revision remain unchanged.
- D6 should say production uses the preallocated indexed 4-ary heap, with the
  allocating BTree variant isolated in the benchmark crate. DESIGN.md is unchanged.

Read-only reference verification: the [timerfd manual](https://man7.org/linux/man-pages/man2/timerfd_create.2.html)
documents nanosecond one-shot arming, expiry readiness and counters reset by
reconfiguration; the [epoll wait manual](https://man7.org/linux/man-pages/man2/epoll_wait.2.html)
documents event-count returns. These support the accounting diagnosis, not a
Linux runtime claim. Cargo.lock, dependency/soak settings, the rustls exception,
no-tokio policy and instruction baseline are unchanged.

## Open questions / next steps

No implementation decisions remain open. The integrator should checkpoint the
final source/report files, run the commands below on Linux x86_64 and arm64, and
return full output for any failure. In particular, confirm that the supplied
failure refers to `zero_event_waits` (line 516 in b78d533). If a Linux rerun instead
fails `os_waits`, that is distinct evidence requiring further investigation.
Run the required GitHub matrix and unchanged instruction gate before merging;
Linux completion and performance are not claimed from macOS checks.

## Linux integrator commands (UNRUN here; execute on x86_64 and arm64)

Run serially on each real Linux host, pinned nightly from rust-toolchain.toml.
Do not drop allocation, native_surface process/signal, or no-spin tests. These
commands execute the entire contract package in each requested backend mode:

```bash
cargo test --locked -p turnloop-contract -- --test-threads=1
cargo test --locked -p turnloop-contract --features turnloop/epoll-timerfd -- --test-threads=1
cargo test --locked -p turnloop-contract --features turnloop/process-sigchld -- --test-threads=1
cargo test --locked -p turnloop-contract --features turnloop/epoll-timerfd,turnloop/process-sigchld -- --test-threads=1
cargo test --locked -p turnloop-contract --features turnloop/executor,turnloop-contract/executor -- --test-threads=1
cargo test --locked --workspace --all-features --no-fail-fast -- --test-threads=1
```

The exact CI entry points also run all other workspace tests and enforce positive
counts independently for every core/protocol/contract package. They cover the
new epoll unit tests as well as the contract regressions:

```bash
python3 scripts/ci/feature_modes.py
python3 scripts/ci/run-tests.py native --mode default
python3 scripts/ci/run-tests.py native --mode epoll-timerfd
python3 scripts/ci/run-tests.py native --mode process-sigchld
python3 scripts/ci/run-tests.py native --mode fallbacks
python3 scripts/ci/run-tests.py native --mode executor
python3 scripts/ci/run-tests.py native --mode all-features
```

Equivalently, `python3 scripts/ci/run-tests.py native` runs all six Linux modes.
For the per-mode native HTTP/TLS/WebSocket interop legs:

```bash
for mode in default epoll-timerfd process-sigchld fallbacks executor all-features; do
  python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode "$mode" || exit
done
```

Return command exit codes, positive counts, and full failure diagnostics. In
particular, the old file-backpressure test must retain one OS wait, one zero-event
wait, `now() >= at`, and zero allocations. Linux runtime remains UNRUN until these
results arrive; cross-Clippy is never substituted for that evidence.
