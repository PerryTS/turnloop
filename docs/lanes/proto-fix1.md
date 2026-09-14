# proto-fix1 lane report

Implementation and required native verification are complete. Base:
`fcea55ee9f876dbc8ffaa3c9341068e3aa3a75d3`; macOS arm64. The integrator created
checkpoint `1f3c516` during this work; this agent made no Git writes or commits.
Read DESIGN.md, CONTRIBUTING.md, docs/INTEGRATION_REPORT.md and the relevant
MongoDB, SQL, KV, HTTP, core, CI and WASM reports completely. No applicable AGENTS.md.

## Source of the intermittent two allocations

**Candidate (a): libtest's receiving thread contaminated a process-global counter.**
The [failing CI job](https://github.com/PerryTS/turnloop/actions/runs/34886441405/job/104118674676)
uses `cargo test --workspace --all-features -- --test-threads=1`. That still runs
the harness receiver concurrently with the test thread.

The pinned Rust source (commit `f7d782a3be46d6bb4b9792fe69a61db389ba1769`) calls
`run_test`, then `rx.recv()` in `library/test/src/lib.rs:443–445`. On its first
blocking receive, `std/sync/mpmc/context.rs:72` constructs `Arc<Context::Inner>`;
`std/sync/mpmc/waker.rs:49` grows its `selectors` vector. If the harness thread
is descheduled while the test finishes its two warm-ups, both allocations land
inside MongoDB's process-wide measurement window. This also explains why the
failure occurs in the first uncompressed/direct mode, rather than accumulating
per command or requiring all features.

A controlled reproduction runs the original MongoDB workload on a worker and
enters the receiving thread's first `recv` only after the measurement begins.
Temporary allocator backtraces identified:

| Allocating thread | Site | macOS allocation |
| --- | --- | ---: |
| Receiver, `measuring=false` | `Arc<Context::Inner>::new` | 48 bytes |
| Receiver, `measuring=false` | `Waker::register_with_packet`, selector vector growth | 96 bytes |
| Receiver, `measuring=false` | `SyncWaker::register`, pthread mutex initialization | 64 bytes |

Linux selects the inline futex mutex in `std/sys/sync/mutex/mod.rs`, so it has
only the two shared allocation sites. **Linux attribution is source-based; Linux
runtime is UNRUN.** The local controlled schedule catches three allocations on
macOS and zero on the MongoDB thread. An unforced instrumented libtest run passed
300/300 fresh macOS processes, so a spontaneous local reproduction is not claimed.
Trace instrumentation excludes its own allocations and was removed from the tree.
The reproduction source and full stacks remain in
`.tools/proto-fix1/channel-repro-final.rs` and `channel-cargo-repro*.log`.

`cargo tree --locked -e features -p turnloop-mongodb` and its `--all-features`
variant are byte-identical. A package-ID-based comparison of full workspace
metadata confirms all **111 reachable MongoDB nodes** have identical features
and dependency edges. Only core/contract/bench/HTTP/zstd-decoder workspace members
change features; MongoDB cannot reach those changed features. There is no
all-features-only pool, BSON/rand initialization or warmed buffer growth involved
in this native failure. The workload uses retained buffers and borrowed raw BSON;
setup/authentication/topology/entropy run outside its measured path.

## Implemented and audited

All **nine allocator implementations** in `protocols/*/tests/` and
`crates/turnloop-contract/tests/` now use const-initialized thread-local Cells.
No process-global allocation counter remains, including on WASI p2/p3 and web.
Allocator callbacks use `try_with` and never allocate for instrumentation.

| Gate | Result of audit / change |
| --- | --- |
| MongoDB | Replaced global atomics with a synchronous scoped TLS counter; counts alloc, alloc_zeroed and realloc; disables counting during unwind |
| PostgreSQL / MySQL | Existing TLS counters already correct; audited, unchanged |
| Redis / SMTP | Existing TLS counters already correct; audited, unchanged |
| HTTP / zstd-decoder | Existing native TLS extended to every target; removed wasm static variants; retained standalone harnesses and positive calibration |
| Core allocation contracts | Existing native TLS extended to WASI; removed the static single-agent shim |
| Browser / Node allocation contracts | Replaced atomics with TLS; added positive real-allocation calibration, which executes in both test binaries |

MongoDB retains **exactly two warm-up commands**, then 1,000 commands and 2,000
borrowed rows per mode, across direct/coordinator × plain/zlib. The original
`rows == 2004`, token/value assertions and **allocation count == 0** are unchanged.
The two warm-ups cover both alternating compression buffers and the operation
template; no extra warm-up or allocation allowance was introduced. The measured
closure directly executes every command and asserts that its current thread's
counter is enabled on every measured iteration.

New regressions prove all three allocation entry points count exactly one,
including initialized zeroed bytes; overlapping measurements count exactly two
worker allocations and zero owner allocations; panic cleanup permits correct
subsequent measurements. The overlap uses pre-warmed barriers, so the worker's
allocations provably occur while the owner's window is open. The unwind regression
is compiled only for `panic="unwind"`; abort-only targets cannot resume a panic.
The positive allocator calibration and complete protocol workload remain enabled
on those targets. WASI 0.2/0.3 and browser do not provide native `std::thread::spawn`,
so the two-thread regression is native-only.

**Every core allocation workload body, including both UDP tests, is byte-for-byte
identical to fcea55e.** Only its counter preamble changed; core5's UDP cancellation
investigation is untouched. Product code, Cargo.lock, dependencies, soak policy,
no-tokio policy, DESIGN.md, backends, no-spin bounds and instruction budgets are
unchanged. No new I/O unwrap was added; strict unsafe-comment lint passes.

## Verification results

Plain Cargo uses nightly-2026-08-20; p3 uses nightly-2026-09-07. Stable is 1.97.1.
WASI uses checksum-verified Wasmtime 46.0.0 and wasi-sdk 34.0 under `.tools/`.
All runtime counts below assert actual execution. Cargo totals include doctests;
the 12 ignored external-service cases per workspace run are **UNRUN**, not passes.

| Command / group | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`, default and all features | PASS |
| Whole-workspace/all-target/all-feature strict Clippy for Linux x86_64, WASI p2, web wasm and WASI p3 | PASS; Linux runtime UNRUN |
| Whole-workspace Windows all-target and library Clippy | FAIL: ring needs Windows SDK headers (`assert.h` missing); no fake headers or gate exclusions |
| Windows strict library Clippy for MongoDB/MySQL/Postgres/Redis/SMTP/core-contract | PASS |
| Windows strict Clippy of the exact new allocator helper and its tests, in a dependency-free temporary Cargo harness | PASS; no Windows runtime claim |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test --locked --workspace -- --test-threads=1` | PASS: **229 passed**, 12 ignored |
| `cargo test --locked --workspace --all-features -- --test-threads=1`, three complete consecutive runs | PASS: **245 / 245 / 245 passed**, 12 ignored each |
| MongoDB allocation binary, default, 300 fresh processes | PASS **300/300**, 1,200 test passes; all four modes per process |
| MongoDB allocation binary, all features, 300 fresh processes | PASS **300/300**, 1,200 test passes; all four modes per process |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS: **6 core + 22 debug + 22 release + 9 release allocation** tests |
| Same runner for `wasm32-wasip3` | PASS: **8 core + 23 debug + 23 release + 10 release allocation** tests |
| `python3 scripts/ci/run-tests.py protocol-wasi --target TARGET`, p2 and p3 | PASS: **23 each** (16 HTTP codecs, 3 HTTP allocation, 3 TLS, 1 decoder allocation) |
| `python3 scripts/ci/run-tests.py node` | PASS: **12 tests**, including new calibration; 3 fetches, 1 observed abort, 6 WebSockets and 7,937 echoed bytes |
| `python3 scripts/ci/run-tests.py web` | Command FAIL at driver discovery; browser test bodies **UNRUN (no local chromedriver/geckodriver)**; no browser launch or sandbox-launch failure claimed |
| Additional MongoDB allocation runtime on WASI p2 and p3 | FAIL: pre-existing wasm compression allocation, detailed below; positive allocator calibration PASS on both |
| Original fcea55e MongoDB allocation runtime on WASI p2 and p3 | Same FAIL: exactly 1,000 allocations in 1,000 compressed commands; independent baseline reproduction |
| `bash scripts/ci/no-tokio.sh` | PASS: eight targets plus union, default/all features, normal/build/dev graphs |
| `python3 scripts/ci/soak.py` | PASS: 251 registry versions; only the existing rustls security exception; seven-day resolver remains active |
| Path-case gate, feature-mode gate, whitespace and final source/feature/counter audit | PASS |

The required MongoDB repetitions used Cargo's JSON executable discovery, after
separate default and `--all-features` builds, then invoked that complete test binary
in a fresh process 300 times per mode with `--test-threads=1`. Each process must
exit zero, print the named workload's success and report all four tests passed.
No retries or failed rounds were discarded. Logs and per-round commands/statuses
are in `.tools/proto-fix1/repetitions.jsonl` and `{default,all-features}-NNN.log`.
Default and all-feature artifacts are identical because their feature graphs are
identical. Each mode executes **1,200,000 measured commands / 2,400,000 measured
rows**; combined **2,400,000 commands / 4,800,000 rows**, excluding warm-ups.
Reproducer:

```sh
python3 .tools/proto-fix1/run.py mongodb-final-build cargo test --locked -p turnloop-mongodb --test allocations --no-run --message-format=json
python3 .tools/proto-fix1/repeat.py mongodb-final-build.log default 300 --test-threads=1
python3 .tools/proto-fix1/run.py mongodb-all-final-build cargo test --locked -p turnloop-mongodb --test allocations --all-features --no-run --message-format=json
python3 .tools/proto-fix1/repeat.py mongodb-all-final-build.log all-features 300 --test-threads=1
```

## Separate pre-existing WASM compression defect

The extra MongoDB WASI run now supplies direct runtime evidence for a previously
unrun protocol gate. On **both p2 and p3**, calibration and the uncompressed modes
succeed, then zlib/direct fails: **1,000 allocations / 1,000 commands**. The compressed
coordinator mode is UNRUN there because the first failing assertion aborts WASI.
An untouched `git archive fcea55e` under `.tools/proto-fix1/baseline` reproduces the
same 1,000-allocation failure with the original global counter on both targets.
This is not introduced by TLS counting and is not the Linux two-allocation flake.

Upstream `miniz_oxide 0.9.1` in `src/deflate/core.rs:496` resets with
`self.lz = LZOxide::new()`. Its wasm-only constructor at lines 1595–1606 allocates
`codes` using `vec![0; LZ_CODE_BUF_SIZE].into_boxed_slice()`; native uses an inline
array. `flate2`'s retained compressor delegates each reset to that implementation.
Additional warm-up cannot eliminate an allocation repeated by every reset.

**Open release blocker:** provide a reviewed compressor fix that retains this
boxed buffer, and cover compressed MongoDB/MySQL command paths on WASI/web.
No dependency fork, version change, skipped workload or relaxed threshold was
introduced in this allocator-isolation lane. The failing MongoDB assertions remain
strict and runnable. Linux/Windows native runtime and full real-server suites also
remain integrator checks, not inferred passes.

## Intermediate failures and deviations

- The two early standalone rustc reproduction builds failed because the pinned
  Cargo layout separates rlib metadata. The Cargo example reproduction succeeded
  in exposing contamination, intentionally failing the old zero assertion. Its
  temporary example was removed after saving the source/stacks under `.tools/`.
- Initial Clippy found a safety comment separated from an unsafe block by a
  formatted assert. Bound the documented slice before asserting; final strict
  Clippy passes. No lint allowance or unsafe obligation was removed.
- The initial WASI runs attempted the new unwind regression on abort-only targets;
  those runs aborted as expected for that panic mode. Restricting this new test to
  actual unwind builds fixed its scope; the complete original workload stayed active
  and then exposed the independent compression defect above.
- The initial GitHub log retrieval failed because gh's default cache directory is
  not writable. Repeating with `XDG_CACHE_HOME` inside `.tools/proto-fix1` succeeded.
- Native allocation isolation already existed in SQL/KV/core/HTTP/decoder. Those
  native mechanisms were preserved instead of adding a shared runtime dependency.
  WASI p3 const TLS was tested successfully in the existing release allocation and
  standalone protocol harnesses, so **no single-agent/global-counter exception**
  is required. Existing p3 release/standalone harness constraints remain.
- No DESIGN.md change is proposed; there are no deviations from its zero-allocation,
  zero-runtime or no-spin rules. The inherited wasm compression failure is reported
  as FAIL, not waived or described as allocation-free.

## Open questions and next steps

No implementation question remains for the native flake fix. Integrator:

1. Commit the remaining source/report changes; rerun the unchanged Linux and Windows
   native matrix. No Linux/Windows machine or Docker is available here.
2. Address the independently reproduced miniz_oxide wasm compressor reset allocation;
   retain its zero threshold and add mandatory compressed protocol runtime coverage.
3. Execute browser tests with installed drivers and full SQL/server fixtures outside
   the sandbox. SQL tests are **UNRUN (sandbox)**; no failed initializer was retried
   or counted as a pass. Twelve ignored service tests remain UNRUN in these runs.
4. Core5 may continue its UDP flake investigation: this lane changed only that file's
   counter mechanism; every test/helper body is verified identical to the base.

## Complete command ledger

All recorded verification invocations follow, including failures and intermediate
runs. Full stdout/stderr, timestamps and exit statuses are in
`.tools/proto-fix1/commands.jsonl` and `<label>.log`. `run.py` preserves child exit
codes. WASM C-toolchain commands source `.tools/wasm-env.sh`; Node/web prepend
`.tools/wasm-bindgen-source/target/release` to PATH and use a local WASM_PACK_CACHE.
WASI p3 uses its explicit toolchain. Repetition counts are recorded separately as
above rather than listing 600 identical executable invocations.

Additional successful read-only verification before ledger setup: pinned Rust/std
source inspection; `git status` / `git rev-parse HEAD`; original MongoDB
`cargo test --locked -p turnloop-mongodb --test allocations --no-run --message-format=json`;
package default/all-feature `cargo tree -e features`; full default/all-feature
`cargo metadata --locked --format-version 1`; CI job JSON and log inspection via gh.
The two feature trees were compared with `diff -u` (PASS: no differences).
The first workspace-feature comparison incorrectly zipped node arrays with different
lengths; it was discarded and replaced with the package-ID/dependency-reachability
comparison asserted by the final audit. `cargo fmt --all` was applied after edits.

| Label | Result | Exact command |
| --- | --- | --- |
| probe-build | PASS (exit 0) | `cargo test --locked -p turnloop-mongodb --test allocations --no-run --message-format=json` |
| channel-build | FAIL (exit 1) | `python3 .tools/proto-fix1/build-repro.py` |
| channel-build-fixed | FAIL (exit 1) | `python3 .tools/proto-fix1/build-repro.py` |
| channel-cargo-repro | FAIL (exit 101) | `cargo run --locked -p turnloop-mongodb --example allocation_probe` |
| channel-cargo-repro-warmed-main | FAIL (exit 101) | `cargo run --locked -p turnloop-mongodb --example allocation_probe` |
| probe-repetitions | PASS (exit 0) | `python3 .tools/proto-fix1/repeat.py probe-build.log probe 300 --test-threads=1` |
| mongodb-fixed | PASS (exit 0) | `cargo test --locked -p turnloop-mongodb --test allocations -- --test-threads=1` |
| native-clippy-all | FAIL (exit 101) | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| wasmtime-install | PASS (exit 0) | `bash scripts/ci/install-wasmtime.sh` |
| mongodb-default-build | PASS (exit 0) | `cargo test --locked -p turnloop-mongodb --test allocations --no-run --message-format=json` |
| mongodb-all-build | PASS (exit 0) | `cargo test --locked -p turnloop-mongodb --test allocations --all-features --no-run --message-format=json` |
| wasm-toolchain | PASS (exit 0) | `python3 scripts/ci/install-wasm-toolchain.py` |
| no-tokio | PASS (exit 0) | `bash scripts/ci/no-tokio.sh` |
| native-clippy-all-final | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| mongodb-final-build | PASS (exit 0) | `cargo test --locked -p turnloop-mongodb --test allocations --no-run --message-format=json` |
| soak | PASS (exit 0) | `python3 scripts/ci/soak.py` |
| paths | PASS (exit 0) | `python3 scripts/ci/check-paths.py` |
| feature-modes | PASS (exit 0) | `python3 scripts/ci/feature_modes.py` |
| fmt | PASS (exit 0) | `cargo fmt --all --check` |
| diff-check | PASS (exit 0) | `git diff --check` |
| clippy-linux | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| mongodb-all-final-build | PASS (exit 0) | `cargo test --locked -p turnloop-mongodb --test allocations --all-features --no-run --message-format=json` |
| clippy-wasi-p2 | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| mongo-default-300 | PASS (exit 0) | `python3 .tools/proto-fix1/repeat.py mongodb-final-build.log default 300 --test-threads=1` |
| clippy-web | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| web-tools | PASS (exit 0) | `python3 scripts/ci/install-web-tools.py` |
| mongodb-wasi-p2 | FAIL (exit 134) | `cargo test --locked -p turnloop-mongodb --test allocations --release --target wasm32-wasip2 --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1` |
| clippy-wasi-p3 | PASS (exit 0) | `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| clippy-windows | FAIL (exit 101) | `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| mongodb-wasi-p3 | FAIL (exit 134) | `cargo +nightly-2026-09-07 test --locked -p turnloop-mongodb --test allocations --release --target wasm32-wasip3 --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1` |
| clippy-windows-libs | FAIL (exit 101) | `cargo clippy --locked --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| native-clippy-default | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| node | PASS (exit 0) | `python3 scripts/ci/run-tests.py node` |
| web | FAIL (exit 1) | `python3 scripts/ci/run-tests.py web` |
| stable | PASS (exit 0) | `cargo +stable check --locked --workspace --all-targets --all-features` |
| protocol-wasi-p2 | PASS (exit 0) | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` |
| mongodb-wasi-p2-final | FAIL (exit 134) | `cargo test --locked -p turnloop-mongodb --test allocations --release --target wasm32-wasip2 --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1` |
| protocol-wasi-p3 | PASS (exit 0) | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` |
| mongodb-wasi-p3-final | FAIL (exit 134) | `cargo +nightly-2026-09-07 test --locked -p turnloop-mongodb --test allocations --release --target wasm32-wasip3 --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1` |
| core-wasi-p2 | PASS (exit 0) | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` |
| mongo-all-300 | PASS (exit 0) | `python3 .tools/proto-fix1/repeat.py mongodb-all-final-build.log all-features 300 --test-threads=1` |
| core-wasi-p3 | PASS (exit 0) | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` |
| workspace-compile | PASS (exit 0) | `cargo test --locked --workspace --all-features --no-run` |
| mongodb-wasi-p2-diagnostic | FAIL (exit 134) | `cargo test --locked -p turnloop-mongodb --test allocations --release --target wasm32-wasip2 --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1 --nocapture` |
| workspace-default | PASS (exit 0) | `cargo test --locked --workspace -- --test-threads=1` |
| workspace-all-1 | PASS (exit 0) | `cargo test --locked --workspace --all-features -- --test-threads=1` |
| workspace-all-2 | PASS (exit 0) | `cargo test --locked --workspace --all-features -- --test-threads=1` |
| workspace-all-3 | PASS (exit 0) | `cargo test --locked --workspace --all-features -- --test-threads=1` |
| original-mongodb-wasi-p2 | FAIL (exit 134) | `cargo test --locked --manifest-path .tools/proto-fix1/baseline/Cargo.toml --target-dir target -p turnloop-mongodb --test allocations --release --target wasm32-wasip2 --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1 --nocapture` |
| windows-counter | PASS (exit 0) | `cargo clippy --manifest-path .tools/proto-fix1/counter-check/Cargo.toml --all-targets --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| windows-protocol-libs | PASS (exit 0) | `cargo clippy --locked -p turnloop-mongodb -p turnloop-mysql -p turnloop-postgres -p turnloop-redis -p turnloop-smtp -p turnloop-contract --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| clippy-final-linux | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| native-clippy-final-default | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| clippy-final-p2 | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| native-clippy-final-all | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| clippy-final-web | PASS (exit 0) | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| stable-final | PASS (exit 0) | `cargo +stable check --locked --workspace --all-targets --all-features` |
| clippy-final-p3 | PASS (exit 0) | `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| fmt-final | PASS (exit 0) | `cargo fmt --all --check` |
| paths-final | PASS (exit 0) | `python3 scripts/ci/check-paths.py` |
| whitespace-final | PASS (exit 0) | `git diff --check` |
| original-mongodb-wasi-p3 | FAIL (exit 134) | `cargo +nightly-2026-09-07 test --locked --manifest-path .tools/proto-fix1/baseline/Cargo.toml --target-dir target -p turnloop-mongodb --test allocations --release --target wasm32-wasip3 --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' -- --test-threads=1 --nocapture` |
| final-audit | PASS (exit 0) | `python3 .tools/proto-fix1/audit.py` |
