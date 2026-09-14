# core3 verification commands

Commands run on macOS arm64 using nightly-2026-08-20 unless specified.
Cross-compilation is not runtime execution. Raw output is in `.tools/core3/`.

- PASS: `cargo build --release -p turnloop-bench --target-dir target/core3-before`.
- PASS: five fresh processes of `target/core3-before/release/turnloop-bench`;
  every row asserted positive operations and `unit == instructions`.
  Raw before measurements: `.tools/core3/before.jsonl`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 1/50, exit 0, 52 test passes, log `.tools/core3/contract50-01.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 2/50, exit 0, 52 test passes, log `.tools/core3/contract50-02.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 3/50, exit 0, 52 test passes, log `.tools/core3/contract50-03.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 4/50, exit 0, 52 test passes, log `.tools/core3/contract50-04.log`.

- **FAIL** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 5/50, exit 101, 36 test passes, log `.tools/core3/contract50-05.log`.

- **PASS** `cargo test -p turnloop-contract --all-features --test native_surface kills_live_child_and_grandchild_as_a_group -- --test-threads=1`; group-reproduce 1/1, exit 0, 1 test passes, log `.tools/core3/group-reproduce-01.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 1/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-01.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 2/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-02.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 3/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-03.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 4/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-04.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 5/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-05.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 6/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-06.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 7/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-07.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 8/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-08.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 9/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-09.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 10/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-10.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 11/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-11.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 12/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-12.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 13/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-13.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 14/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-14.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 15/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-15.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 16/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-16.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 17/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-17.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 18/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-18.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 19/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-19.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 20/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-20.log`.

## Development measurements and regression probes

- PASS: `cargo test -p turnloop -p turnloop-contract --all-features -- --test-threads=1`
  after initial readiness queues (`initial-tests.log`) and after race regressions
  (`regressions.log`).
- PASS: `cargo build --release -p turnloop-bench --target-dir target/core3-after`
  and initial execution; `after-initial.jsonl` records the reduction.
- PASS: `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`
  after fixing SAFETY comment placement (`clippy-fixed.log`). Initial command FAIL
  on three comments above assert macros (`clippy-initial.log`); no lint suppressed.
- PASS: `cargo test -p turnloop-contract --test allocations file_readiness_survives -- --test-threads=1`
  after initializing reserved Darwin mutexes (`file-backpressure-fixed.log`).
  Earlier FAILs: unfinished warm-up wake interrupted the quiet timer measurement;
  then one lazy pthread-mutex allocation in Files::start. All zero-allocation and
  exact wait assertions were retained; a byte-producing worker and timer now finish
  warm-up first. `file-allocation-stack.log` attributes the allocation to
  std::sys::sync::mutex::pthread::Mutex::get for a previously unused file op slot.
- Expected FAIL (exit 101): `cargo test --manifest-path .tools/core3/before-source/Cargo.toml --target-dir target/core3-original-tests -p turnloop-contract --test native_surface pending_signal_cannot_outlive -- --test-threads=1`.
  The fixed regression applied to exact starting core 9e9d8aa terminates its
  subprocess with SIGUSR1 (30); `signal-negative-control-clean.log`.
  Two earlier snapshot probes passed but are NOT negative-control evidence: an
  integrator checkpoint moved HEAD, and restored old timestamps then reused a
  newer Cargo artifact. The final probe used the explicit starting SHA and a fresh
  target directory. Starting-code benchmark binaries likewise used fresh targets.
- PASS: `cargo build --manifest-path .tools/core3/before-source/Cargo.toml --release -p turnloop-bench --target-dir target/core3-original-boundaries`
  and equivalent build from baseline revision f747623 with target/core3-pre-core2.
  Both used the identical extended ri_instructions harness. Five processes per
  revision measured three workloads at capacities 16/1024/8192 and separate drops;
  `boundaries-{pre-core2,before,after}.jsonl` holds preliminary results.
- FAIL: initial contract repetition campaign stopped at run 5, after 4 PASS runs,
  with SIGCHLD (20) during the process-group contract. Never retried inside a gate.
  XNU source confirms kqueue notification precedes ordinary signal disposition
  processing; postsig_locked assumes a newly observed SIG_DFL is fatal even for
  a previously caught SIGCHLD. Kqueue now uses SIG_DFL for SIGCHLD throughout
  subscription (default-ignore, preserving zombies), SIG_IGN for other signals.
  The post-fix campaign restarts at zero under `contract50-final`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 21/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-21.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 22/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-22.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 23/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-23.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 24/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-24.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 25/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-25.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 26/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-26.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 27/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-27.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 28/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-28.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 29/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-29.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 30/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-30.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 31/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-31.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 32/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-32.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 33/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-33.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 34/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-34.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 35/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-35.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 36/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-36.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 37/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-37.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 38/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-38.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 39/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-39.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 40/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-40.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 41/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-41.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 42/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-42.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 43/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-43.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 44/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-44.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 45/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-45.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 46/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-46.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 47/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-47.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 48/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-48.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 49/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-49.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 50/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-50.log`.

- PASS: read-only assertions against starting revision 9e9d8aa: Cargo.toml,
  Cargo.lock, .cargo/config.toml, scripts/ci/policy.toml and the committed Linux
  instructions.json are unchanged; no added Rust line contains `.unwrap()`.
- PASS: final `cargo test -p turnloop-contract --all-features -- --test-threads=1`
  campaign completed **50/50 consecutive runs, zero failures, 52 tests per run**.
  That is 2,600 test passes, 12,800 four-loop signal rounds / 51,200 deliveries,
  and 12,800 children in the concurrent exit test, besides other process fixtures.
  The prior failed campaign is retained above and excluded from these counts.
- Full-workspace repetitions use `RUST_TEST_THREADS=1`, following CONTRIBUTING's
  signal/process/allocator isolation requirement. The requested Cargo command is
  otherwise unchanged; no retry/skip or timing/allocation threshold adjustment.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 1/10, exit 0, 237 test passes, log `.tools/core3/workspace10-01.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 2/10, exit 0, 237 test passes, log `.tools/core3/workspace10-02.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 3/10, exit 0, 237 test passes, log `.tools/core3/workspace10-03.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 4/10, exit 0, 237 test passes, log `.tools/core3/workspace10-04.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 5/10, exit 0, 237 test passes, log `.tools/core3/workspace10-05.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 6/10, exit 0, 237 test passes, log `.tools/core3/workspace10-06.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 7/10, exit 0, 237 test passes, log `.tools/core3/workspace10-07.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 8/10, exit 0, 237 test passes, log `.tools/core3/workspace10-08.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 9/10, exit 0, 237 test passes, log `.tools/core3/workspace10-09.log`.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace --all-features`; workspace10 10/10, exit 0, 237 test passes, log `.tools/core3/workspace10-10.log`.

- **PASS** `cargo fmt --check`; fmt 1/1, exit 0, 0 test passes, log `.tools/core3/fmt-01.log`.

- **PASS** `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; clippy-default 1/1, exit 0, 0 test passes, log `.tools/core3/clippy-default-01.log`.

- **PASS** `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; clippy-all 1/1, exit 0, 0 test passes, log `.tools/core3/clippy-all-01.log`.

- **FAIL** `cargo test --manifest-path .tools/core3/before-source/Cargo.toml --target-dir target/core3-negative-esrch -p turnloop esrch_before_wait_status -- --test-threads=1`; esrch-negative 1/1, exit 101, 0 test passes, log `.tools/core3/esrch-negative-01.log`.

- **PASS** `cargo +stable check --locked --workspace --all-targets --all-features`; stable 1/1, exit 0, 0 test passes, log `.tools/core3/stable-01.log`.

- **PASS** `cargo build --release -p turnloop-bench --target-dir target/core3-final`; bench-final-build 1/1, exit 0, 0 test passes, log `.tools/core3/bench-final-build-01.log`.

- **PASS** `cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; linux-default 1/1, exit 0, 0 test passes, log `.tools/core3/linux-default-01.log`.

- **PASS** `env MIRI_SYSROOT=.tools/core3/miri-sysroot cargo miri setup`; miri-setup 1/1, exit 0, 0 test passes, log `.tools/core3/miri-setup-01.log`.

- **PASS** `cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; linux-all 1/1, exit 0, 0 test passes, log `.tools/core3/linux-all-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; aarch64-unknown-linux-gnu 1/1, exit 0, 0 test passes, log `.tools/core3/aarch64-unknown-linux-gnu-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target x86_64-unknown-freebsd -- -D warnings -D clippy::undocumented_unsafe_blocks`; x86_64-unknown-freebsd 1/1, exit 0, 0 test passes, log `.tools/core3/x86_64-unknown-freebsd-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target aarch64-apple-ios -- -D warnings -D clippy::undocumented_unsafe_blocks`; aarch64-apple-ios 1/1, exit 0, 0 test passes, log `.tools/core3/aarch64-apple-ios-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks`; x86_64-pc-windows-msvc 1/1, exit 0, 0 test passes, log `.tools/core3/x86_64-pc-windows-msvc-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks`; wasm32-wasip2 1/1, exit 0, 0 test passes, log `.tools/core3/wasm32-wasip2-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks`; wasm32-unknown-unknown 1/1, exit 0, 0 test passes, log `.tools/core3/wasm32-unknown-unknown-01.log`.

- **PASS** `cargo clippy -p turnloop -p turnloop-contract --lib --all-features --target aarch64-linux-android -- -D warnings -D clippy::undocumented_unsafe_blocks`; android 1/1, exit 0, 0 test passes, log `.tools/core3/android-01.log`.

- **PASS** `env MIRI_SYSROOT=.tools/core3/miri-sysroot python3 scripts/ci/run-tests.py miri`; miri 1/1, exit 0, 2 test passes, log `.tools/core3/miri-01.log`.

- **PASS** `cargo +nightly-2026-09-07 clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks`; wasip3 1/1, exit 0, 0 test passes, log `.tools/core3/wasip3-01.log`.

- **PASS** `bash scripts/ci/no-tokio.sh`; no-tokio 1/1, exit 0, 0 test passes, log `.tools/core3/no-tokio-01.log`.

- **PASS** `python3 scripts/ci/soak.py`; soak 1/1, exit 0, 0 test passes, log `.tools/core3/soak-01.log`.

- **PASS** `python3 scripts/ci/run-tests.py loom`; loom 1/1, exit 0, 6 test passes, log `.tools/core3/loom-01.log`.

- **PASS** `env 'RUSTFLAGS=--cfg loom' cargo clippy -p turnloop --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; loom-clippy 1/1, exit 0, 0 test passes, log `.tools/core3/loom-clippy-01.log`.

- **PASS** `python3 scripts/ci/check-paths.py`; paths 1/1, exit 0, 0 test passes, log `.tools/core3/paths-01.log`.

- **PASS** `git diff --check`; diff 1/1, exit 0, 0 test passes, log `.tools/core3/diff-01.log`.

- Expected FAIL: `esrch-negative-01.log` runs the identical injected registration
  regression against starting core 9e9d8aa, with only the registration closure seam
  added for injection. It fails with `Error { kind: Other, os: Some(3) }` after
  proving the hook ran once while WNOHANG returned zero. The fixed version passes
  in all ten full-workspace runs, alongside the existing real exited-child test.
- PASS: final workspace reliability **10/10 consecutive runs**, 237 test passes per
  run, zero failures. Combined final campaigns: **4,970 test passes**, 61,440 churn
  signal deliveries and 15,360 concurrent-contract child exits. Ignored external
  server tests are not counted as executed.

- **PASS** `env RUST_TEST_THREADS=1 cargo test --workspace`; workspace-default 1/1, exit 0, 222 test passes, log `.tools/core3/workspace-default-01.log`.

- **PASS** `env RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --all-features --no-deps`; rustdoc 1/1, exit 0, 0 test passes, log `.tools/core3/rustdoc-01.log`.

- **PASS** `python3 .tools/core3/measure.py`: five rotated fresh-process rounds of all three revision binaries, steady and `--instruction-boundaries`; 390 positive ri_instructions measurements, identical extended harness, cgu=1. Raw committed artifact: `benchmarks/core3-macos-arm64.jsonl`; metadata includes source and binary SHA-256s.

- **PASS** `cargo fmt --check`; final-fmt 1/1, exit 0, 0 test passes, log `.tools/core3/final-fmt-01.log`.

- **PASS** `python3 scripts/ci/check-paths.py`; final-paths 1/1, exit 0, 0 test passes, log `.tools/core3/final-paths-01.log`.

- **PASS** `git diff --check`; final-diff 1/1, exit 0, 0 test passes, log `.tools/core3/final-diff-01.log`.
