# core5 verification command ledger

PASS means exit zero; compilation never counts as target runtime execution.
The initial failing default-UDP option test is the deliberate pre-fix regression.
Subsequent failures (strict safety-comment placement, C cross-toolchain setup,
Android pinned-Clippy macro diagnostic, WASI 0.3 release timer precision) are
retained alongside later passes; see the root report for their dispositions.
Commands beginning with `.tools/core5` are local verification helpers only. The
Linux repetition command is reproduced completely in LANE_REPORT.md. No helper
changes a gate, dependency or source file under test.

- **FAIL** (exit 101): `cargo test --locked -p turnloop --lib backend::unix::udp_tests::default_udp_bind_does_not_enable_address_sharing -- --exact --test-threads=1 --nocapture`; log `.tools/core5/logs/20260914T193020877200.log`.
- **PASS** (exit 0): `cargo test --locked -p turnloop --lib backend::unix::udp_tests -- --test-threads=1 --nocapture`; log `.tools/core5/logs/20260914T193311684203.log`.
- **FAIL** (exit 101): `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193405018121.log`.
- **PASS** (exit 0): `python3 .tools/core5/repeat.py`; log `.tools/core5/logs/20260914T193403877627.log`.
- **PASS** (exit 0): `bash scripts/ci/no-tokio.sh`; log `.tools/core5/logs/20260914T193522690254.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193502093938.log`.
- **PASS** (exit 0): `bash scripts/ci/install-wasmtime.sh`; log `.tools/core5/logs/20260914T193540263252.log`.
- **PASS** (exit 0): `python3 scripts/ci/soak.py`; log `.tools/core5/logs/20260914T193522696985.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193527713930.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193555693238.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193557560306.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193559222599.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193600971128.log`.
- **FAIL** (exit 101): `cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193603038586.log`.
- **FAIL** (exit 1): `python3 .tools/core5/cross.py`; log `.tools/core5/logs/20260914T193501997269.log`.
- **PASS** (exit 0): `cargo fmt --all --check`; log `.tools/core5/logs/20260914T193712119592.log`.
- **FAIL** (exit 101): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193710974432.log`.
- **FAIL** (exit 1): `python3 .tools/core5/cross-arm64.py`; log `.tools/core5/logs/20260914T193710827902.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193713196396.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193719081965.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193723085449.log`.
- **PASS** (exit 0): `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2`; log `.tools/core5/logs/20260914T193713123085.log`.
- **PASS** (exit 0): `cargo +stable check --locked --workspace --all-targets --all-features`; log `.tools/core5/logs/20260914T193727297062.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193754518322.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193755587773.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193757052484.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-unknown-freebsd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193806247170.log`.
- **FAIL** (exit 101): `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-linux-android -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193807831936.log`.
- **FAIL** (exit 1): `python3 .tools/core5/checks.py`; log `.tools/core5/logs/20260914T193711964517.log`.
- **PASS** (exit 0): `git diff --check`; log `.tools/core5/logs/20260914T193830731467.log`.
- **PASS** (exit 0): `python3 scripts/ci/run-tests.py native`; log `.tools/core5/logs/20260914T193503162708.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193931224375.log`.
- **PASS** (exit 0): `cargo +nightly-2026-09-07 clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193932284486.log`.
- **PASS** (exit 0): `cargo fmt --all --check`; log `.tools/core5/logs/20260914T194014884567.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T193947001057.log`.
- **PASS** (exit 0): `cargo test --locked -p turnloop --lib backend::unix::udp_tests -- --test-threads=1`; log `.tools/core5/logs/20260914T194015665663.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194022832725.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194023885481.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194024985302.log`.
- **PASS** (exit 0): `cargo test --locked -p turnloop --lib backend::unix::udp_tests --features turnloop/executor -- --test-threads=1`; log `.tools/core5/logs/20260914T194025825848.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194026925001.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194028377286.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194029442560.log`.
- **PASS** (exit 0): `python3 .tools/core5/cross-arm64.py`; log `.tools/core5/logs/20260914T193931123761.log`.
- **PASS** (exit 0): `cargo test --locked -p turnloop --lib backend::unix::udp_tests --all-features -- --test-threads=1`; log `.tools/core5/logs/20260914T194030130189.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194032687811.log`.
- **FAIL** (exit 1): `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3`; log `.tools/core5/logs/20260914T193933444557.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194033957801.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194046371843.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194047435480.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194048410664.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194049679674.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194050677001.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract --lib --all-features --target aarch64-linux-android -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194052080676.log`.
- **PASS** (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/core5/logs/20260914T194052913584.log`.
- **PASS** (exit 0): `python3 scripts/ci/feature_modes.py`; log `.tools/core5/logs/20260914T194057396214.log`.
- **PASS** (exit 0): `git diff --check`; log `.tools/core5/logs/20260914T194057543394.log`.
- **PASS** (exit 0): `python3 .tools/core5/final-checks.py`; log `.tools/core5/logs/20260914T194014761877.log`.
- **FAIL** (exit 1): `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3`; log `.tools/core5/logs/20260914T194236982179.log`.
- **PASS** (exit 0): `python3 .tools/core5/repeat.py`; log `.tools/core5/logs/20260914T194341322883.log`.
- **PASS** (exit 0): `cargo fmt --all --check`; log `.tools/core5/logs/20260914T194413531691.log`.
- **PASS** (exit 0): `cargo +stable check --locked --workspace --all-targets --all-features`; log `.tools/core5/logs/20260914T194413996451.log`.
- **PASS** (exit 0): `cargo test --locked --workspace -- --test-threads=1`; log `.tools/core5/logs/20260914T194414728427.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194443380342.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194443792187.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194444131743.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194444482731.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194455677888.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194456038059.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194456391488.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194456803077.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194457155524.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194457838275.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/executor,turnloop-contract/executor -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194502122094.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194502494164.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194502857236.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --features turnloop/epoll-timerfd,turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194503222965.log`.
- **PASS** (exit 0): `env CC_aarch64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cc-aarch64 AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/core5/.tools/core5/zig-cache cargo clippy --locked --workspace --all-targets --target aarch64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core5/logs/20260914T194503589325.log`.
- **PASS** (exit 0): `python3 .tools/core5/last.py`; log `.tools/core5/logs/20260914T194413450684.log`.
- **PASS** (exit 0): `python3 .tools/core5/audit.py`; log `.tools/core5/logs/20260914T194544890126.log`.
- **FAIL** (exit 134): `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test --locked -p turnloop-contract --test allocations --target wasm32-wasip2 --all-features --release -- --test-threads=1`; log `.tools/core5/logs/20260914T194543726536.log`.
- **PASS** (exit 0): `git diff --check`; log `.tools/core5/logs/20260914T194654531664.log`.
- **PASS** (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/core5/logs/20260914T194654580988.log`.
- **PASS** (exit 0): `env CARGO_TARGET_WASM32_WASIP2_RUNNER=scripts/ci/wasmtime-runner.sh cargo test --locked -p turnloop-contract --test allocations --target wasm32-wasip2 --all-features --release -- --nocapture --test-threads=1 < .tools/core5/stdin.txt`; log `.tools/core5/logs/20260914T194728641204.log`.
- **PASS** (exit 0): `git diff --check`; log `.tools/core5/logs/20260914T194819199334.log`.
- **PASS** (exit 0): `python3 .tools/core5/audit.py`; log `.tools/core5/logs/20260914T194819246808.log`.

- **PASS**: `cargo fmt --all` (initial formatting, before logged checks).
- **PASS** read-only: required document/source/manifest inspection; installed Rust
  toolchain/target inventory; IPv4/IPv6 Python SO_REUSEADDR duplicate-bind probe;
  upstream Linux UDP allocator and epoll manual inspection; final Git diff/status.
- **UNRUN**: Linux x86_64/arm64 runtime (all six modes and 200 repetitions each),
  Windows/FreeBSD/Android runtime, browser/Node runtime, hosted CI and Linux
  instruction gates. See LANE_REPORT.md for exact Linux commands.
- **UNRUN**: ignored external-server suites, including SQL sandbox prerequisites.
  No ignored test is counted as a pass.

- **PASS**: Linux integrator Python snippet parsed with `compile`; runtime remains UNRUN.
