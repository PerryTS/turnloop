# adb-fix1 command ledger

All commands ran in the adapters-db clone. Logs are in `.tools/adb-fix1/`.
WASM commands source `.tools/wasm-env.sh` (wasi-sdk 34); Wasmtime 46 is provided
by the repository runner. The p3 toolchain is nightly-2026-09-07. Other Cargo
commands use pinned nightly-2026-08-20, except the explicit stable check.

FAIL entries are retained. SQL startup failed before tests; their bodies are
UNRUN. The initial PG native/WASI failures came from the new rejection fixture's
OS error-category assumption; the final fixture proves exactly-once stream drop,
zero startup bytes and the exact binding error. Windows all-target checking lacks
the inherited Platform provider. The ordinary path gate needs staged new files;
a separate audit uses the same strict checker against exact working-tree names.

Fixture generation (PASS, before logging): `openssl version` (3.6.3),
`python3 protocols/turnloop-tls/tests/certificates/generate.py`. The script records
independent hashlib digests and deletes temporary private keys. Read-only Git,
source, manifest and RFC/PostgreSQL documentation inspection also completed.

| Command | Result | Log |
| --- | --- | --- |
| `cargo test -p turnloop-tls --test channel_binding` | PASS | `binding-tests.log` |
| `cargo test -p turnloop-postgres --all-features -- --test-threads=1` | FAIL | `pg-tests.log` |
| `cargo fmt --all` | PASS | `fmt-apply.log` |
| `cargo test -p turnloop-postgres -p turnloop-tls --all-features -- --test-threads=1` | PASS | `touched-tests.log` |
| `cargo fmt --all` | PASS | `fmt-apply-2.log` |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `native-clippy.log` |
| `python3 scripts/test-servers.py --services postgres,mysql run python3 scripts/ci/run-tests.py protocol --package turnloop-postgres --package turnloop-mysql` | FAIL | `sql-fixtures.log` |
| `bash scripts/ci/no-tokio.sh` | PASS | `no-tokio.log` |
| `python3 scripts/ci/soak.py` | PASS | `soak.log` |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `native-clippy-all.log` |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS | `stable.log` |
| `cargo test --locked --workspace -- --test-threads=1` | PASS | `workspace-default.log` |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS | `workspace-all.log` |
| `cargo clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-p2.log` |
| `cargo +nightly-2026-09-07 clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-p3.log` |
| `cargo clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `clippy-web.log` |
| `cargo fmt --all --check` | PASS | `fmt-check.log` |
| `git diff --check` | PASS | `diff-check.log` |
| `env RUSTDOCFLAGS=-Dwarnings cargo doc --locked -p turnloop-postgres -p turnloop-tls --all-features --no-deps` | PASS | `rustdoc.log` |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --package turnloop-postgres --package turnloop-tls` | FAIL | `protocol-p2.log` |
| `cargo test --locked -p turnloop-postgres --test asynchronous --features turnloop --target wasm32-wasip2 --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"' tls_scram -- --test-threads=1 --nocapture` | FAIL | `pg-p2-diagnostic.log` |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3 --package turnloop-postgres --package turnloop-tls` | PASS | `protocol-p3.log` |
| `cargo fmt --all` | PASS | `fmt-apply-3.log` |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --package turnloop-postgres --package turnloop-tls` | PASS | `protocol-p2-fixed.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-linux-cc AR_x86_64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-ar cargo clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `linux-clippy.log` |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3 --package turnloop-postgres --package turnloop-tls` | PASS | `protocol-p3-fixed.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --locked -p turnloop-postgres -p turnloop-tls --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `windows-lib-clippy.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `windows-all-clippy.log` |
| `cargo test --locked -p turnloop-postgres -p turnloop-tls --all-features -- --test-threads=1` | PASS | `touched-tests-final.log` |
| `python3 scripts/ci/check-paths.py` | FAIL | `paths.log` |
| `python3 scripts/ci/feature_modes.py` | PASS | `feature-modes.log` |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `native-clippy-final.log` |
| `cargo clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `p2-clippy-final.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --locked -p turnloop-postgres -p turnloop-tls --test channel_binding --test protocol --test server --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `windows-deterministic.log` |
| `cargo +nightly-2026-09-07 clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `p3-clippy-final.log` |
| `cargo clippy --locked -p turnloop-postgres -p turnloop-tls --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `web-clippy-final.log` |
| `python3 .tools/adb-fix1/audit.py` | PASS | `audit.log` |
| `cargo test --locked -p turnloop-postgres --test server rejected_startup_reports_server_sqlstate_and_message -- --exact --nocapture` | PASS | `expected-panic.log` |
| `cargo fmt --all --check` | PASS | `final-fmt.log` |
| `git diff --check` | PASS | `final-diff.log` |

## Required external reruns

| Command / environment | Status |
| --- | --- |
| `python3 scripts/test-servers.py --services postgres,mysql run python3 scripts/ci/run-tests.py protocol --package turnloop-postgres --package turnloop-mysql` outside the sandbox | UNRUN post-fix; sandbox invocation above failed at initdb |
| `python3 scripts/test-servers.py --services postgres,mysql run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-postgres --package turnloop-mysql` | UNRUN (sandbox); source `.tools/wasm-env.sh` first |
| `python3 scripts/ci/run-tests.py native` on Linux/Windows | UNRUN (hosts unavailable); cross-Clippy results above are compilation only |
| Browser runtime | UNRUN; raw PostgreSQL TCP is unavailable in browsers |
