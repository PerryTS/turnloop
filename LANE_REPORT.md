# wasm2 lane report

2026-09-14 handoff. Implemented WASI p2, experimental p3, web, target contracts
and required CI wiring. **The lane is not merge-ready:** unchanged WASI timer
precision and p3 UDP allocation gates fail; browser runtime tests are UNRUN in
the sandbox. No gate was weakened to turn these failures green.

Read completely: DESIGN.md (including no-spin rule 4a), CONTRIBUTING.md,
docs/INTEGRATION_REPORT.md and relevant wasm/core/CI lane reports. No AGENTS.md
applies. The integrator has periodically committed the working tree; this agent
performed no git mutation. The checked-in trait now says revision 2 (empty-wait
instrumentation); all revision-1 methods and ownership rules are preserved.

## Implemented

- **p2:** wasi:io/poll as the only wait per turn; monotonic deadlines; TCP/listen,
  UDP, pooled/provided buffers, writev, shutdown, generational cancellation and
  close ordering. Intrusive per-direction queues retain partial work. Persistent
  poll input/owner/index arrays plus scoped canonical return storage remove both
  poll allocations and read/UDP list allocations. Empty canonical lists handled.
- **p3 experimental:** persistent wait-set; fixed request/return storage; direct
  async TCP streams/futures and UDP subtasks; synchronous cancellation and cached
  EOF/shutdown results; cancelled-head successor progress. Context restoration
  across native async/wait-set boundaries fixes the release UDP stack trap.
  The remaining yield/allocator limitations are documented precisely below.
- **web:** HostCallback, only turn(Now), performance.now deadlines, asynchronous
  coalesced scheduling, fetch/AbortController and WebSocket host imports, complete
  generational IDs and 64-bit tokens, terminal oversize errors, unsupported
  operation errors and late-callback invalidation. Feature web-worker adds an
  actual bounded SAB/Atomics MPSC Poster with waitAsync, backpressure and no ticks.
- **contracts:** shared WASI scenarios including 60-expiry no-spin, 64-connection
  TCP, UDP, writev/shutdown, timers/posts, buffer/capacity pressure and cancellation;
  repeated EOF/shutdown, empty datagrams and cancelled-head regression. Four native/
  WASI allocation subjects, plus seven browser/Node contracts with actual HTTP,
  abort and WebSocket fixture counters. Two real Workers deliver 2,000 unique posts.
- **CI:** removed pending wasm comments; target metadata/feature selection, strict
  positive counts per binary and browser, independently attempted browsers, owned
  ephemeral fixtures/cleanup and traffic assertions. Pinned Wasmtime 46/nightlies/
  wasm-pack and a matching soaked wasm-bindgen CLI build. Required wasi/web jobs
  remain wired into ci-gate. Runtime steps still execute after lint/browser failure,
  while the job retains failure. Added adversarial fixture/traffic checks.
- **documentation:** docs/wasm.md covers setup, capabilities, host allocation
  boundary, Worker isolation, experimental ABI and all platform exclusions;
  CONTRIBUTING.md now describes the real wasm jobs.

## Verification outcome

| Area | Result |
| --- | --- |
| Formatting, native default/all-feature strict Clippy, stable workspace check, workspace tests, strict docs | PASS; existing protocol server ignores retained, not claimed as server passes |
| Full workspace p2/web strict cross-Clippy and stable cross-checks | PASS |
| p3 core/contract strict Clippy | PASS |
| p3 full workspace Clippy | FAIL: MongoDB -> BSON -> rand 0.9 -> getrandom 0.3.4 rejects p3 |
| p2 shared non-precision cases | PASS: 15-case coverage run plus new cancelled-head test (16 cases total) |
| p3 shared non-precision cases | PASS: 16 cases in release, plus debug coverage/cancellation regression |
| Full WASI precision | FAIL: unchanged <500us median lateness ceiling; p2 1.03475ms, p3 debug 2.346875ms and release 1.083958ms |
| p2 allocation gates | PASS all four: TCP/provided/pooled, accepts, timer batches, cancel backlog, 100 UDP datagrams and 20 actual deadline expiries; zero allocations |
| Native expanded allocation gates | PASS all four |
| p3 release allocation gates | Three PASS (TCP/timer/accept/backlog/deadline); UDP FAIL after 6,400 actual bytes: 100 allocations vs zero |
| p3 debug allocation harness | FAIL before tests: get-arguments canonical realloc uses zero shadow-stack context |
| Node 26.5.1 with pinned wasm-pack 0.15.0 | PASS seven contracts; fixture confirms 3 HTTP requests, 1 abort, 4 WebSockets, 6,657 echoed bytes; Worker and Rust zero-allocation subjects pass |
| Chrome and Firefox | UNRUN (sandbox), each launch attempted; zero browser tests executed |
| Seven-day soak | PASS all 208 locked registry versions; policy unchanged |
| no-tokio.sh | PASS every audited target, default/all features; no forbidden runtime added |
| CI adversarial tests | PASS 16, including real fixture cleanup and rejected absent traffic |
| Tool checksum installers | PASS Wasmtime, wasm-pack, actionlint, zizmor, ShellCheck |
| Workflow lint wrapper and direct zizmor | PASS; zizmor zero findings |
| Raw actionlint 1.7.12 | FAIL only pre-existing unsupported concurrency.queue key; existing strict compatibility wrapper retained |
| git diff --check / I/O unwrap audit | PASS; no new unwrap in wasm backend I/O paths, unsafe-aware Clippy passes |
| Linux/Windows runtime and native server integration | UNRUN (unavailable hosts / outside wasm backend scope); no claims of execution |

The initial fixed-port development fixture was stopped through its exec session.
Final CI runners own and reap their fixtures even on failure. A direct sandbox
kill attempt was denied; session interruption completed cleanup (exit 130).

## Deviations, open questions and next steps

**p3 remains opt-in experimental.** A nonblocking wait-set poll alone starves
host socket subtasks under continuous Now turns. One cooperative thread-yield plus
one poll fixes progress and passes no-spin, but host scheduling time is not
bounded by an established contract. Promotion needs a bounded host-progress
primitive or a proved yield budget; no fresh block_on or guest wait loop is used.

P3 UDP receive still owns a canonical list and allocates once per nonempty
receive. Correct reuse needs asynchronous return-storage ownership across turns,
multiple in-flight requests and cancellation, including error-string ownership.
The p2 synchronous scratch scope cannot safely be copied. Debug custom allocator
entry additionally needs valid canonical stack initialization before the harness
runs. These are unresolved implementation/toolchain integration requirements,
not allocation-gate exemptions. The UDP gate stays enabled and fails.

The earlier release UDP trap is **fixed** by saving/restoring the pinned compiler's
shadow stack (canon context slot 0) across the wait-set boundary. Both the UDP
regression and full release I/O coverage pass. A diagnostic newer p3-only nightly
(2026-09-13) did not independently fix it; pins remain 08-20 / 09-07. Runtime timer
precision remains a separate blocker even in release; deadlines are passed in
nanoseconds without a backend-added 1ms floor. Investigate runtime/host timing
outside the sandbox rather than busy-spin or relax the existing threshold.

P3 workspace checking also needs a soaked p3-capable getrandom dependency in the
protocol graph. No global insecure/custom RNG substitution or dependency-soak
exception was introduced. WASI nodelay remains advisory because the bindings do
not expose a setter; reuse-port explicitly returns Unsupported.

The web allocation gate covers Rust guest storage. Browser/JS networking, full
fetch response bodies, typed arrays and callbacks still allocate on the host;
that broader process-wide promise is not claimed. Explicit platform exclusions
(native threads, fd integration, transfer, native timer ceiling on web, raw socket
protocols) and their reasons are listed in docs/wasm.md, never counted as passes.

DESIGN.md is unchanged. Proposed clarifications for integration review: define
the web host allocation boundary and whole-message/oversize semantics; formalize
the p3 host-yield budget and canonical return allocator responsibilities; decide
how runtime timer precision and advisory WASI socket options are specified.

Integrator reruns (setup and PATH are in docs/wasm.md):

```sh
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3
export PATH="$PWD/.tools/bin:$PWD/.tools/wasm-bindgen-source/target/release:$PATH"
export WASM_PACK_CACHE="$PWD/.tools/wasm-pack-cache"
python3 scripts/ci/run-tests.py web --browser chrome
python3 scripts/ci/run-tests.py web --browser firefox
python3 scripts/ci/run-tests.py node
```

Chrome failed to start its driver (SIGKILL); Firefox/geckodriver reached session
creation then failed HTTP 500/SIGKILL. Both are UNRUN (sandbox), not test failures
or passes. Linux/Windows runtimes are likewise UNRUN because hosts are unavailable.
Required WASI timing and allocation failures remain real FAIL results.

## Exact verification command ledger

The following groups repeated identical commands; result sequences retain every
recorded run in order (for example FAIL → PASS records a fixed development
failure). Commands with test filters are coverage/diagnostic runs, not substitutes
for unfiltered gates. Older test names reflect the then-current source. PATH's
inherited suffix is represented by `$PATH` and this checkout by `$PWD`; all other
arguments are verbatim. The unabridged timestamped machine ledger is also in
`.tools/wasm2/commands.jsonl`. All unsafe blocks were checked with
`-D clippy::undocumented_unsafe_blocks`. Read-only source/tool inventory passed;
source reads and edit commands are not test runs.

| Command | Results in execution order |
| --- | --- |
| `cargo check -p turnloop --target wasm32-wasip2` | PASS |
| `bash scripts/ci/install-wasmtime.sh` | PASS |
| `cargo fmt --all` | PASS ×15 |
| `cargo clippy -p turnloop -p turnloop-contract --all-targets --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | FAIL ×2 |
| `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test -p turnloop-contract --target wasm32-wasip2 --test wasi -- --test-threads=1 --nocapture` | FAIL |
| `cargo clippy -p turnloop -p turnloop-contract --all-targets --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test -p turnloop-contract --target wasm32-wasip2 --test wasi -- --test-threads=1 --nocapture --skip timer_precision` | FAIL |
| `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test -p turnloop-contract --target wasm32-wasip2 --test wasi writev_shutdown -- --nocapture` | PASS |
| `python3 spikes/web/tests/bootstrap.py` | PASS |
| `cargo +nightly-2026-09-07 check -p turnloop --target wasm32-wasip3 --features wasi-p3-experimental` | FAIL |
| `cargo +nightly-2026-09-07 clippy -p turnloop -p turnloop-contract --locked --all-targets --target wasm32-wasip3 --features wasi-p3-experimental -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy -p turnloop -p turnloop-contract --locked --all-targets --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test -p turnloop-contract --locked --target wasm32-wasip3 --features wasi-p3-experimental --test wasi bounded_wait -- --nocapture` | PASS |
| `env PATH="$PWD/.tools/wasm-bindgen-source/target/release:$PATH" WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache wasm-pack test --mode no-install --node crates/turnloop-contract --locked --test node` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test -p turnloop-contract --locked --target wasm32-wasip3 --features wasi-p3-experimental --test wasi -- --nocapture --test-threads=1 --skip timer_precision` | FAIL ×2 → PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test -p turnloop-contract --locked --target wasm32-wasip3 --features wasi-p3-experimental --test allocations -- --nocapture --test-threads=1` | FAIL ×2 |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test -p turnloop-contract --release --locked --target wasm32-wasip3 --features wasi-p3-experimental --test allocations -- --nocapture --test-threads=1` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS ×2 |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL |
| `env PATH="$PWD/.tools/wasm-bindgen-source/target/release:$PATH" WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache wasm-pack test --mode no-install --node crates/turnloop-contract --locked --features web-worker --test node` | PASS |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | FAIL ×2 |
| `env PATH="$PWD/.tools/wasm-bindgen-source/target/release:$PATH" WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache python3 scripts/ci/run-tests.py node` | PASS |
| `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck wasm-pack` | PASS |
| `env PATH="$PWD/.tools/wasm-bindgen-source/target/release:$PATH" WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache python3 scripts/ci/run-tests.py web` | UNRUN (sandbox; launch exited 1) |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --release --test wasi udp -- --nocapture --test-threads=1` | FAIL ×2 → PASS ×2 |
| `python3 scripts/ci/lint-workflows.py` | FAIL |
| `bash scripts/ci/no-tokio.sh` | PASS |
| `python3 scripts/ci/soak.py` | PASS |
| `env PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` | PASS ×2 |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS ×2 |
| `.tools/bin/actionlint .github/workflows/ci.yml .github/workflows/release.yml` | FAIL |
| `.tools/bin/zizmor --offline --min-severity low .github/workflows` | PASS |
| `env PATH="$PWD/.tools/bin:$PWD/.tools/wasm-bindgen-source/target/release:$PATH" WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache python3 scripts/ci/run-tests.py web --browser firefox` | UNRUN (sandbox; launch exited 1) |
| `cargo test --locked --workspace -- --test-threads=1` | PASS |
| `cargo +nightly-2026-09-07 clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS ×2 |
| `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test --locked -p turnloop-contract --target wasm32-wasip2 --test wasi -- --nocapture --test-threads=1 --skip timer_precision` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --test wasi -- --nocapture --test-threads=1 --skip timer_precision` | PASS |
| `rustup toolchain install nightly-2026-09-13 --profile minimal --component clippy --target wasm32-wasip3` | PASS |
| `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test --locked -p turnloop-contract --target wasm32-wasip2 --test wasi cancelled_head_restarts_queued_read -- --nocapture --test-threads=1` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-13 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --release --test wasi udp -- --nocapture --test-threads=1` | FAIL |
| `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-wasip2` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --release --test allocations -- --nocapture --test-threads=1` | FAIL ×2 |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --release --test wasi -- --nocapture --test-threads=1 --skip timer_precision` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --test wasi cancelled_head_restarts_queued_read -- --nocapture --test-threads=1` | PASS |
| `env PATH="$PWD/.tools/bin:$PWD/.tools/wasm-bindgen-source/target/release:$PATH" WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache python3 scripts/ci/run-tests.py node` | PASS ×3 |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --release --test allocations steady_udp_and_deadline_poll_allocate_nothing -- --nocapture --test-threads=1` | FAIL |
| `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test --locked -p turnloop-contract --target wasm32-wasip2 --release --test allocations -- --nocapture --test-threads=1` | PASS |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1` | PASS |
| `cargo +nightly-2026-08-20 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `env 'RUSTDOCFLAGS=-D warnings' cargo doc --locked --workspace --all-features --no-deps` | PASS |
| `env CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 test --locked -p turnloop-contract --target wasm32-wasip3 --features wasi-p3-experimental --release --test wasi timer_precision -- --nocapture --test-threads=1` | FAIL |
| `git diff --check` | PASS |
