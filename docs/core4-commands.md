# core4 verification commands

PASS denotes exit zero; cross-checks are compilation only. Linux runtime UNRUN.

- **PASS** (exit 0): `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v`; log `.tools/core4/logs/20260914T185829573516.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T185835951325.log`.
- **PASS** (exit 0): `python3 scripts/ci/feature_modes.py`; log `.tools/core4/logs/20260914T185919637487.log`.
- **PASS** (exit 0): `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck`; log `.tools/core4/logs/20260914T185951330742.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T185926603146.log`.
- **PASS** (exit 0): `cargo fmt --all --check`; log `.tools/core4/logs/20260914T185957946018.log`.
- **PASS** (exit 0): `git diff --check`; log `.tools/core4/logs/20260914T185958430066.log`.
- **PASS** (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/core4/logs/20260914T185958490698.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190020067918.log`.
- **PASS** (exit 0): `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190022296493.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --target x86_64-unknown-linux-gnu --features turnloop/epoll-timerfd -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190024146888.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --target x86_64-unknown-linux-gnu --features turnloop/process-sigchld -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190029451434.log`.
- **PASS** (exit 0): `env PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py`; log `.tools/core4/logs/20260914T190038373963.log`.
- **PASS** (exit 0): `bash scripts/ci/no-tokio.sh`; log `.tools/core4/logs/20260914T190042083831.log`.
- **PASS** (exit 0): `cargo +stable check --locked --workspace --all-targets --all-features`; log `.tools/core4/logs/20260914T190030798062.log`.
- **PASS** (exit 0): `python3 scripts/ci/soak.py`; log `.tools/core4/logs/20260914T190045486013.log`.
- **FAIL** (exit 1): `python3 scripts/ci/run-tests.py native`; log `.tools/core4/logs/20260914T190117939037.log`.
- **PASS** (exit 0): `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v`; log `.tools/core4/logs/20260914T190421093113.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190447529050.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190450398060.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190451355675.log`.
- **PASS** (exit 0): `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190452318454.log`.
- **PASS** (exit 0): `cargo +nightly-2026-09-07 clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/core4/logs/20260914T190453275238.log`.
- **PASS** (exit 0): `python3 scripts/ci/run-tests.py native`; log `.tools/core4/logs/20260914T190513463990.log`.
- **PASS** (exit 0): `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode default`; log `.tools/core4/logs/20260914T190721892990.log`.
- **PASS** (exit 0): `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode executor`; log `.tools/core4/logs/20260914T190723241399.log`.
- **PASS** (exit 0): `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode all-features`; log `.tools/core4/logs/20260914T190724198374.log`.
- **PASS** (exit 0): `cargo run --release --locked -p turnloop-bench -- --portable --timers`; log `.tools/core4/logs/20260914T190725178252.log`.
- **PASS** (exit 0): `cargo run --release --locked -p turnloop-bench --features timer-btree -- --portable --timers`; log `.tools/core4/logs/20260914T190729233558.log`.
- **PASS** (exit 0): `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v`; log `.tools/core4/logs/20260914T190723036428.log`.
- **PASS** (exit 0): `python3 scripts/ci/feature_modes.py`; log `.tools/core4/logs/20260914T190736058656.log`.
- **PASS** (exit 0): `python3 .tools/core4/audit.py`; log `.tools/core4/logs/20260914T190834687597.log`.
- **PASS** (exit 0): `cargo fmt --all --check`; log `.tools/core4/logs/20260914T190835189017.log`.
- **PASS** (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/core4/logs/20260914T190835806612.log`.
- **PASS** (exit 0): `env PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py`; log `.tools/core4/logs/20260914T190838324006.log`.

- **PASS**: `cargo fmt --all` (initial formatting, before logged checks).
- **PASS** read-only: `rustup target list --installed`; `cargo metadata --locked --no-deps --format-version 1`; Git/source/manifest/workflow inspection.
- **UNRUN**: Linux per-mode runtime, Linux workspace all-features runtime and hosted matrix/instruction gate; exact commands in LANE_REPORT.md.
- **PASS** (exit 0): `git diff --check`; log `.tools/core4/logs/20260914T190956617258.log`.
