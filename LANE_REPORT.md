# wasm-merge lane report

2026-09-14, macOS arm64, branch `lane/wasm2`. Textual merge resolution and local
verification are complete. The previous session was killed by the host; this run
resumed its files and logs, reviewed the implementation and ran the required
checks again, serially with `CARGO_BUILD_JOBS=4`. Git metadata is read-only:
**the integrator must stage the resolved files and commit the merge**. Unmerged
index entries remain expected; there are no working-file conflict markers.

Read DESIGN.md, CONTRIBUTING.md, docs/INTEGRATION_REPORT.md, the wasm/core/CI lane
reports, Backend revision 2 and the current wasm documentation. No applicable
AGENTS.md was found. No subagents, Git writes, pushes, publishing or changes to
other clones were performed.

## Implemented and retained

- Resolved all ten conflicted files by combining the parents. The Backend trait
  body matches main's revision 2 exactly apart from whitespace. Native services,
  executor, types and exports coexist with WASI p2/p3 and web providers. All
  allocation functions from both parents survive; no assertion or gate was removed.
- WASI p2/p3 implement `wasi:cli` stdio with owned stream endpoints, provided/pooled
  reads, queued writes/writev, shutdown, EOF caching on p3, cancellation and close.
  P2 drops subscriptions before parent streams; p3 uses CLI result-future vtables
  and acknowledgement. Closing driver endpoints preserves the host standard streams.
- AF_UNIX, handle passing, processes, signals and TTY mode/size operations return
  `ErrorKind::Unsupported` on WASI/web. Web stdio is also Unsupported. Failed
  setup is repeatedly tested for leaked handles/operation credits.
- External waits work on the single WASI/web agent, using 16,384 reserved slots
  and each loop's completion queue. Condition inequality/notify/store, deadlines,
  cancellation, stop, capacity and drop have tests. Deadlines participate in
  `next_deadline` and native/host waits. Expiry delivery during a turn does not
  leave a stale notifier flag that forces the next deadline into a Now poll.
- `web-worker` adds bounded Atomics-backed condition-update rings, owner routing,
  full/contended/closed rejection and full-width values, alongside the existing
  Worker Poster. No OS thread is started on WASI. The executor runs on WASI
  sockets and web WebSockets with sleep/timeout/cancellation and allocation gates.
- Retained p3 UDP canonical storage, its concurrent/cancellation/drop allocation
  tests, real WASI entropy for getrandom 0.3/0.4 and BSON, the release allocation
  requirement, bare-clock measurements, the <=2 ms WASI median precision bound,
  every no-spin assertion and pinned browser/driver tooling. P3 stays experimental.
- CI retains **all 14 prerequisite jobs**, with `ci-gate.needs` exactly their
  union. Main's HTTP/TLS/WebSocket/zstd, security exception, portable Windows,
  deterministic MySQL, Redis 8.4, PostgreSQL 16.13 observer, protocol summary,
  path-case, wasi-sdk and curl HTTP/2 fixes remain. WASI runners exercise core,
  debug/release semantics and independent release allocation binaries with actual
  stdin input; protocol-WASI remains separately required.
- Reconciled Cargo.lock from main's reviewed lock using the pinned resolver with
  the seven-day policy: its log explicitly reports resolution “as of 7 days ago”.
  Exact wasm binding pins were retained, and rustls stays **0.23.45** with main's
  sole RUSTSEC-2026-0285 exception. No additional exception or soak override.
- Fixed two test-only portability/determinism issues without reducing coverage:
  the newer p3 Clippy requires enumerating an upstream decoder diagnostic table;
  the HTTP crash fixture now finishes writing both real diagnostic markers before
  its invalid first line can trigger teardown. Its runner still performs the reap;
  all log-tail, stderr/stdout, state-cleanup and return-code assertions remain.

## Final verification results

Cross-checks compile code; they do not establish runtime behavior on another OS.
The twelve ignored real-server tests in the native workspace result are UNRUN,
not included in the pass count. The separate HTTP fixture run executes its two
normally ignored HTTP cases. Detailed commands and earlier failures follow below.

| Gate | Final result and positive evidence |
| --- | --- |
| Conflict-marker scan | PASS, no matches, including a separate hidden-file scan of workflows/config |
| Parent union / trait / policy audit | PASS: all 14 jobs, complete fan-in, all 19 parent allocation helper/test names, exact main trait body, rustls 0.23.45, active seven-day config |
| `cargo fmt --all --check` | PASS |
| Workspace/all-target strict Clippy, default and all features | PASS: native, x86_64 Linux, WASI p2, web wasm, p3 on nightly-2026-09-07 |
| Workspace/all-target/all-feature release Clippy | PASS: WASI p2 and p3 |
| Windows strict cross-Clippy | PASS: core, contract and bench, all targets/all features; runtime UNRUN |
| Stable 1.97.1 workspace/all-target/all-feature check | PASS: native, WASI p2 and web wasm |
| Rustdoc with warnings denied | PASS |
| Native workspace all features | PASS, **236 tests**, 12 ignored external-service cases |
| Native default/all-feature CI runner | PASS, **771 passes including independent member/contract repetitions**; earlier known child race retained below |
| WASI p2 production runner, Wasmtime 46.0.0 | PASS: **6 core + 22 debug + 22 release + 9 release allocation tests** |
| WASI p3 production runner, Wasmtime 46.0.0 | PASS: **8 core + 23 debug + 23 release + 10 release allocation tests** |
| WASI protocol runners, p2 and p3 | PASS, **23 each**: 16 HTTP codec, 3 HTTP allocation, 3 portable TLS, 1 decoder allocation |
| Node 26.5.1 web runner | PASS, **11 tests**; 3 fetches, 1 observed abort, 6 WebSockets, 7,937 echoed fixture bytes; Worker Poster and condition delivery executed |
| Python automation | PASS, **80 tests**, with warnings treated as errors |
| no-tokio | PASS, normal/build/dev graphs for 8 policy targets plus all-target union, default/all features |
| Dependency soak | PASS, **251 registry versions**, every checksum/age, exactly the existing rustls exception |
| cargo-deny | PASS: advisories, bans, licenses, sources; transitive version-duplicate warnings remain |
| check-paths / diff whitespace | PASS: 1,337 tracked files, 181 Rust/Cargo references; no whitespace errors |
| Workflow lint wrapper / zizmor / ShellCheck | PASS; existing strict queue compatibility validation retained |
| Raw actionlint 1.7.12 | **FAIL**, only the two inherited `concurrency.queue` parser errors; no new filter or suppression |
| Browser archive/driver pin verification | PASS: Chrome/ChromeDriver 153.0.8010.36, Firefox 155.0.1, geckodriver 0.37.1, all committed SHA-256 pins |
| HTTP/TLS/WebSocket native interop | PASS, **17 tests**, all required fixture suites attempted and summarized |
| h2spec | PASS, **147 distinct strict tests**, zero failed/skipped |
| Loom | PASS, **5 production models** |
| Miri | PASS, **2 marked pure-core tests**; sysroot built within .tools |
| Portable timer benchmark | PASS, **9 workloads**, 100,000 asserted operations each; reports nanoseconds, not Linux instructions |
| Private fixture cleanup audit | PASS, no HTTP/SQL/Redis/MongoDB/test-server state remains |

### Precision, no-spin and allocation subjects

Final release measurements on bare Wasmtime 46.0.0, twenty 250-us deadlines each:

| Path | Median lateness |
| --- | ---: |
| Bare WASI p2 | 1,056,375 ns |
| Driver WASI p2 | 1,043,125 ns |
| Bare WASI p3 | 1,029,541 ns |
| Driver WASI p3 | 1,047,125 ns |

The bare waits reproduce the host's approximately millisecond lateness. Both
driver medians satisfy the unchanged <=2 ms bound. Full raw samples remain in
`.tools/wasm-merge/restart-{baseline,wasi}-p{2,3}.log`; docs/wasm.md preserves the
original raw measurement series and the reproducer.

Ordinary timers and external waits each exercise **60 actual expiries** beside an
idle registered socket: twenty at 0.5/2/10 ms, no early expiry, <=2 turns and <=1
zero-event OS wait per expiry. WASI runs these in debug and release; Node runs
both host-scheduled variants with an idle WebSocket and zero OS waits.

The zero-allocation gates retain TCP provided/pooled transfers, accepts, timer
batches, backlog cancellation, 100 UDP datagrams/6,400 bytes, concurrent IPv6 UDP
with 320 verified receives and 320 cancellations across two loops, capacity-one
output and pending-drop reuse. New gates measure 800 external-wait completions,
100 stdio writes, caller-buffer stdio reads through repeated EOF and 1,001
executor I/O/sleep rounds. P3 additionally measures scalar entropy and canonical
storage; its semantic test fills **754 bytes through both getrandom generations**
and creates two distinct BSON ObjectIds. Web measures Rust allocation counts,
including executor turns and waits, and delivers **64 Worker condition completions**
to two loops. JavaScript/runtime allocations are explicitly outside that counter.

## Failures investigated and preserved

- Initial p2 Clippy failed with Apple clang's missing wasm target; sourcing the
  checksum-pinned wasi-sdk environment fixed it. All final wasm checks used that
  environment. Initial web closure u64 inference and an executor test holding a
  RefCell borrow across await were corrected; final checks pass without allows.
- Initial p3 Clippy rejected an inherited decoder diagnostic range-index loop;
  iterator/enumerate preserves every printed entry. P3 Cargo also prints inherited
  manifest warnings and its unused old unstable-config-key warning; the locked
  dependencies are still independently audited by the pinned soak tool.
- Initial new external-wait no-spin tests failed on p3 in debug and release.
  Same-agent expiry had used the notifying completion path during its own turn,
  leaving a stale wake. The dedicated during-turn publication path fixes that
  cause. The original numerical bounds remain; full runners pass afterward.
- The previous native CI runner failed `concurrent_256_children_exit_once`
  (`native_surface.rs:119`, spawn returned `Error { kind: Other, os: Some(3) }`). This is the user-reported
  core2 race owned by core3. Fresh all-feature workspace and complete native runner
  pass, which does not establish that the intermittent race is fixed. The native
  process/signal engines and executor file match the integrated main index;
  no process/signal code was changed here. SIGUSR1 fan-out failure was not observed
  in this lane; the approximately 33x Linux idle regression was not measured.
- A new runner unit fixture initially used a synthetic path outside its mock
  workspace; corrected the fixture root, preserving validation. On restart, the
  existing HTTP crash test raced log emission: HTTP correctly rejected an invalid
  first line and killed the fake before its final diagnostics. Synchronizing
  handoff with the actual last marker fixes the fixture; an added pre-handoff
  assertion proves it has not already been reaped. All original assertions remain.
  One intermediate edit missed Python's `in` keyword; the syntax error was fixed
  before the focused regression and all 80 automation tests passed.
- Raw actionlint still rejects only `concurrency.queue`. Main's existing wrapper
  separately validates exact queue policy and filters only that known parser
  diagnostic. No workflow policy or gate was weakened to get a pass.

## UNRUN and limitations

| Command / capability | Status and reason |
| --- | --- |
| `python3 scripts/ci/run-tests.py web` (both pinned browsers) | **UNRUN on this Mac**, per user instruction: macOS SIGKILLs chromedriver. Required Linux CI browser runs retained; digest verification is not browser execution |
| Native tests on Linux / Windows, Docker service job | **UNRUN**, no corresponding hosts or Docker. Windows production IOCP still belongs to its lane |
| `python3 scripts/ci/instructions.py` | **UNRUN**, needs Ubuntu/Valgrind. Core3 owns known approximately 33x idle regression; committed baseline unchanged |
| `scripts/test-servers.py run cargo test --workspace -- --include-ignored` | **UNRUN (sandbox)** for full database service execution: supplied PostgreSQL shmget and MySQL initializer failures; integrator reruns outside sandbox |
| Redis/MongoDB/SMTP external-server bodies | **UNRUN in this merge verification**; ordinary native protocol tests execute. No concurrent database process groups were launched during the memory-constrained restart |
| Full Windows workspace test-target C compilation | **UNRUN** without Windows SDK headers; requested Windows core/contract/bench cross-check passed |
| Actual GitHub workflows, release/publication, prolonged cross-platform soak | **UNRUN**; no such runtime/publishing claims |

P3 is explicitly experimental. Its current host-yield/cancellation bounds and raw
component context ABI portability remain unproven; see docs/upstream/wasi-p3-wait.md.
Debug semantic/no-spin tests run, but p3 custom-allocator tests require release
because the pinned compiler can trap in pre-main argument lowering. The allocation
gate and threshold are preserved, not waived. Browser host allocation accounting
remains distinct from measured Rust guest allocation freedom.

## Deviations, open questions and next steps

No DESIGN.md edit or new dependency/security exception. WASI terminal bindings
provide detection, not portable size/mode queries, so those operations explicitly
return Unsupported as requested. Existing proposals about browser allocation scope
and strict p3 scheduler/context guarantees remain in docs/wasm.md and the upstream
handoff document; they are not silently declared solved.

There are no blocking implementation questions for the integrator. Next steps:

1. Stage the resolved files and commit the merge; only Git's unmerged index state
   remains. The source tree has no conflict markers.
2. Run Linux/Windows CI and both pinned browsers, plus real SQL/full protocol
   fixtures outside the sandbox. Review core3's process/signal and idle-cost fixes;
   preserve the current assertions and instruction baseline policy.
3. Keep p3 experimental until its documented upstream bounds are established.
   Remove the rustls exception at **2026-09-21T15:11:17Z**, when its exact soak ends.

## Verification command ledger

All commands ran from this clone. Plain `cargo` uses nightly-2026-08-20; p3 names
nightly-2026-09-07 explicitly. WASM commands sourced `.tools/wasm-env.sh` from the
pinned installer; `.tools/bin` and `.tools/wasm-bindgen-source/target/release` were
on PATH for the relevant tools, and `WASM_PACK_CACHE` was inside `.tools`.
`CARGO_BUILD_JOBS=4` applies to the restart; Go tool builds used GOMAXPROCS=4 and
GOFLAGS=-p=4. “Latest” reports the last execution of that exact command; history
counts preserve failures, including pre-restart attempts. Logs and full invocation
timestamps are in `.tools/wasm-merge/commands.jsonl` and the named `.log` files.
The wrappers print every nested Cargo/Wasmtime command and assert positive test
counts; the runtime rows above identify which subjects actually executed.

Additional non-ledger audits: `git status`, parent-stage/current diff review,
exact trait/body and CI-job/function-union checks, rustls/config assertions,
fixture-state absence and source hygiene: PASS. The initial
`cargo check --workspace --all-targets --all-features` regenerated the lock under
the soak and passed (initial-check.log); `python3 scripts/ci/install-wasm-toolchain.py`
passed its pinned SDK checksum and three compiler/archive probes (wasi-sdk.log).
The user-specified marker command returned no matches (rg exit 1 is expected):

```sh
rg -n '^(<<<<<<<|=======|>>>>>>>)( |$)' --glob '!target/**' --glob '!.tools/**' .
```

| Verification command | Latest | History |
| --- | --- | --- |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x2, FAIL x1 |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1, FAIL x2 |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1, FAIL x1 |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS | PASS x3 |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | PASS | PASS x3, FAIL x1 |
| `python3 scripts/ci/run-tests.py node` | PASS | PASS x3 |
| `cargo +nightly-2026-09-07 test --locked -p turnloop-contract --test wasi --all-features --release --target wasm32-wasip3 --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' external_wait_deadlines_do_not_spin -- --nocapture --test-threads=1` | PASS | PASS x1 |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS | PASS x3, FAIL x2 |
| `python3 scripts/ci/check-paths.py` | PASS | PASS x3 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x2 |
| `python3 scripts/ci/soak.py` | PASS | PASS x2 |
| `python3 scripts/ci/lint-workflows.py` | PASS | PASS x2 |
| `.tools/bin/actionlint -color` | FAIL | FAIL x2 |
| `.tools/bin/zizmor --offline --min-severity low .github/workflows` | PASS | PASS x2 |
| `bash scripts/ci/no-tokio.sh` | PASS | PASS x2 |
| `cargo deny --locked check advisories bans licenses sources` | PASS | PASS x2 |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --release --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x2 |
| `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo fmt --all --check` | PASS | PASS x2 |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS | PASS x2 |
| `cargo test --locked --workspace -- --test-threads=1` | PASS | PASS x1 |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS | PASS x2 |
| `python3 scripts/ci/install-browsers.py --verify-only` | PASS | PASS x2 |
| `bash scripts/ci/install-wasmtime.sh` | PASS | PASS x1 |
| `python3 -c <parent-job-union and complete ci-gate.needs assertion; exact source in ci-union ledger entry>` | PASS | PASS x1 |
| `python3 scripts/ci/run-tests.py native` | PASS | PASS x1, FAIL x1 |
| `cargo +nightly-2026-09-07 clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x2 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x2 |
| `env 'RUSTDOCFLAGS=-D warnings' cargo doc --locked --workspace --all-features --no-deps` | PASS | PASS x2 |
| `cargo run --locked -p turnloop-contract --release --all-features --target wasm32-wasip2 --example wasi_timer_baseline --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"'` | PASS | PASS x2 |
| `cargo +nightly-2026-09-07 run --locked -p turnloop-contract --release --all-features --target wasm32-wasip3 --example wasi_timer_baseline --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"'` | PASS | PASS x2 |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | PASS | PASS x2 |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | PASS | PASS x2 |
| `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo clippy --locked --workspace --all-targets --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo clippy --locked --workspace --all-targets --target wasm32-wasip2 --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo clippy --locked --workspace --all-targets --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo clippy --locked --workspace --all-targets --target wasm32-unknown-unknown --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --target wasm32-wasip3 --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `git diff --check` | PASS | PASS x2 |
| `python3 -W error -m unittest discover -s scripts/ci -p test_servers.py -k http_failed_start -v` | PASS | PASS x1, FAIL x1 |
| `cargo clippy --locked --workspace --all-targets --all-features --release --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | PASS x1 |
| `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-wasip2` | PASS | PASS x1 |
| `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown` | PASS | PASS x1 |
| `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop` | PASS | PASS x1 |
| `python3 scripts/ci/run-tests.py loom` | PASS | PASS x1 |
| `env MIRI_SYSROOT=<clone>/.tools/miri-sysroot cargo miri setup` | PASS | PASS x1 |
| `env MIRI_SYSROOT=<clone>/.tools/miri-sysroot python3 scripts/ci/run-tests.py miri` | PASS | PASS x1 |
| `python3 scripts/ci/h2spec.py` | PASS | PASS x1 |
| `cargo run --release -p turnloop-bench --locked -- --portable --timers` | PASS | PASS x1 |
