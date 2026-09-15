# adb-fix2 command ledger

Commands ran from the clone root. Plain Cargo uses nightly-2026-08-20; p3 uses
nightly-2026-09-07. Logs are under `.tools/adb-fix2/`. The `run.py` wrapper records
child exit status without retries; `checks.py`, `windows.py` and `final.py` merely
sequence the individually recorded commands. Ignored test bodies are UNRUN.
The WASI runner used the existing environment and never started/stopped servers.
Its logs print each exact inner Cargo/Wasmtime invocation and positive test counts.

Historical failures are retained: real PostgreSQL before the fix; parallel
SIGCHLD interference (natural and controlled); an invalid new-fixture read after
locally closing its stream (corrected to assert the close acknowledgement and
exactly-once client stream drop); Windows compiler headers, then the inherited
missing Platform provider. The initial repetition campaign passed 20 processes
before SIGUSR1 killed all-feature process 1 (exit -30); the final campaign passes
all 30 after isolating the two competing SIGUSR1 fixtures. The final default
workspace run independently failed the unchanged core UDP bind assertion at
unix.rs:1073 (-1, errno absent); the all-feature workspace passed. No retry or
assertion change replaces that FAIL.

Additional PASS inspection/audit commands: `git status --short`, `git log -1`,
`git diff --stat`, source/diff reads with `rg`/`sed`/`cat`, all required full report
reads, environment/runner inspection, and positive-count log summation with Python.
One initial unlogged `cargo fmt --all` also passed before the recorded formatting
invocations. Initial guessed nonexistent source paths returned read errors; they
were replaced with paths from `rg --files`, with no verification claim.


| Command | Result | Log |
| --- | --- | --- |
| `zsh -c 'source .tools/sql-env.sh && cargo test -p turnloop-postgres --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture'` | FAIL | `pg-real-before.log` |
| `cargo test --locked -p turnloop-contract --test native_surface -- --nocapture` | FAIL | `native-surface-before.log` |
| `cargo test --locked -p turnloop-postgres --features turnloop` | PASS | `pg-first.log` |
| `cargo test --locked -p turnloop-contract --test native_surface -- --nocapture` | PASS | `native-surface-eintr.log` |
| `zsh -c 'source .tools/sql-env.sh && cargo test -p turnloop-postgres --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture'` | PASS | `pg-real-fixed.log` |
| `cargo test --locked -p turnloop-contract --test native_surface -- --nocapture` | PASS | `native-surface-early-waits.log` |
| `zsh -c 'source .tools/sql-env.sh && cargo test -p turnloop-mysql --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture'` | PASS | `mysql-real-async.log` |
| `cargo test --locked -p turnloop-contract --test native_surface -- --nocapture` | FAIL | `native-surface-early-waits-2.log` |
| `cargo test --locked -p turnloop-contract --test native_surface registered_processes_and_signals_do_not_spin -- --exact --test-threads=1 --nocapture` | FAIL | `no-spin-controlled-sigchld.log` |
| `cargo test --locked -p turnloop-postgres --features turnloop --test protocol --test asynchronous` | FAIL | `pg-regressions.log` |
| `zsh -c 'source .tools/sql-env.sh && cargo test -p turnloop-mysql --features turnloop --test server -- --include-ignored --test-threads=1 --nocapture'` | PASS | `mysql-real-server.log` |
| `cargo test --locked -p turnloop-postgres --features turnloop` | PASS | `pg-regressions-fixed.log` |
| `zsh -c 'source .tools/sql-env.sh && cargo test -p turnloop-postgres --features turnloop --test server -- --include-ignored --test-threads=1 --nocapture'` | PASS | `pg-real-server.log` |
| `cargo test --locked -p turnloop-contract --test native_surface -- --nocapture` | PASS | `native-surface-isolated.log` |
| `cargo fmt --all` | PASS | `fmt-apply.log` |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-native.log` |
| `zsh -c 'source .tools/sql-env.sh && source .tools/wasm-env.sh && python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-postgres --package turnloop-mysql'` | PASS | `wasi-p2-real-sql.log` |
| `bash scripts/ci/no-tokio.sh` | PASS | `no-tokio.log` |
| `cargo test --locked --workspace` | PASS | `workspace-parallel.log` |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-native-default.log` |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS | `stable.log` |
| `zsh -c 'source .tools/wasm-env.sh && cargo clippy --locked -p turnloop-postgres -p turnloop-contract --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks'` | PASS | `clippy-p2.log` |
| `zsh -c 'source .tools/wasm-env.sh && cargo +nightly-2026-09-07 clippy --locked -p turnloop-postgres -p turnloop-contract --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks'` | PASS | `clippy-p3.log` |
| `zsh -c 'source .tools/wasm-env.sh && cargo clippy --locked -p turnloop-postgres -p turnloop-contract --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks'` | PASS | `clippy-web.log` |
| `zsh -c 'source .tools/wasm-env.sh && cargo clippy --locked -p turnloop-postgres -p turnloop-contract --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks'` | PASS | `clippy-linux.log` |
| `zsh -c 'source .tools/wasm-env.sh && cargo clippy --locked -p turnloop-postgres -p turnloop-contract --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks'` | FAIL | `clippy-windows-all.log` |
| `cargo clippy --locked -p turnloop-postgres -p turnloop-contract --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `clippy-windows-lib.log` |
| `cargo clippy --locked -p turnloop-postgres --test protocol --test server --test allocations --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `clippy-windows-wire.log` |
| `python3 scripts/ci/soak.py` | PASS | `soak.log` |
| `python3 scripts/ci/check-paths.py` | PASS | `paths.log` |
| `python3 scripts/ci/feature_modes.py` | PASS | `feature-modes.log` |
| `cargo fmt --all --check` | PASS | `fmt.log` |
| `git diff --check` | PASS | `whitespace.log` |
| `zsh -c 'source .tools/sql-env.sh && source .tools/wasm-env.sh && python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-postgres --package turnloop-mysql'` | PASS | `wasi-p2-final.log` |
| `cargo test --locked --workspace --all-features` | PASS | `workspace-parallel-all.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --locked -p turnloop-postgres -p turnloop-contract --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `windows-lib-zig.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --locked -p turnloop-postgres --test protocol --test server --test allocations --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `windows-wire-zig.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --locked -p turnloop-postgres -p turnloop-contract --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `windows-all-zig.log` |
| `zsh -c 'source .tools/wasm-env.sh && python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3 --package turnloop-postgres'` | PASS | `wasi-p3.log` |
| `zsh -c 'source .tools/sql-env.sh && cargo test -p turnloop-postgres --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture'` | PASS | `pg-real-final.log` |
| `python3 .tools/adb-fix2/repeat.py` | FAIL | `parallel-repetitions.log` |
| `python3 .tools/adb-fix2/audit.py` | PASS | `final-audit.log` |
| `python3 .tools/adb-fix2/repeat.py` | PASS | `parallel-repetitions-final.log` |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-native-final.log` |
| `cargo clippy --locked -p turnloop-contract --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-contract-linux-final.log` |
| `cargo test --locked --workspace` | FAIL | `workspace-parallel-final.log` |
| `cargo test --locked --workspace --all-features` | PASS | `workspace-parallel-all-final.log` |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS | `stable-final.log` |
| `git diff --check` | PASS | `whitespace-final.log` |
| `cargo fmt --all --check` | PASS | `fmt-final.log` |
| `python3 .tools/adb-fix2/audit.py` | PASS | `source-audit-final.log` |
