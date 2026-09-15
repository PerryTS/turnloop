# IOCP fix2 verification commands

Executed on macOS arm64 at base `87a44cd` plus the fix2 working tree.
Cross-Clippy compiles Windows code; all Windows runtime tests remain UNRUN locally.

## Build and policy gates

| Result | Command |
| --- | --- |
| PASS | `cargo clippy --locked --target x86_64-pc-windows-msvc -p turnloop -p turnloop-contract -p turnloop-io --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| PASS | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| PASS | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| PASS | `cargo +stable check --locked --workspace --all-targets --all-features` |
| PASS | `cargo fmt --all --check` |
| PASS | `python3 scripts/ci/check-paths.py` |
| PASS | `python3 scripts/ci/feature_modes.py` |
| PASS | `bash scripts/ci/no-tokio.sh` |
| PASS | `python3 scripts/ci/soak.py` |
| PASS | `git diff --check` |
| PASS | `python3 scripts/ci/run-tests.py native` |
| PASS | `cargo fmt --all` (after each edit group) |
| PASS | `python3 .tools/iocp-fix2/audit.py` |

The exact Windows Clippy command above also passed once before the allocation test
was added. No failed verification commands occurred in this follow-up.
Read-only source/reference inspection and `rg`/Git diff audits passed.

## Native runner commands

Each row was executed by `python3 scripts/ci/run-tests.py native`.
Ignored/cfg-excluded bodies are UNRUN and excluded from these counts.
Manifest paths are rendered relative to this clone; raw logs retain absolute paths.

| Mode | Result | Passed tests | Command |
| --- | --- | --- | --- |
| default | PASS | 236 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml --workspace -- --test-threads=1` |
| default | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop -- --test-threads=1` |
| default | PASS | 26 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-http -- --test-threads=1` |
| default | PASS | 8 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-tls -- --test-threads=1` |
| default | PASS | 20 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mongodb -- --test-threads=1` |
| default | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mysql -- --test-threads=1` |
| default | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-postgres -- --test-threads=1` |
| default | PASS | 10 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-redis -- --test-threads=1` |
| default | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-smtp -- --test-threads=1` |
| default | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-websocket -- --test-threads=1` |
| default | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-io -- --test-threads=1` |
| default | PASS | 49 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-contract -- --test-threads=1` |
| executor | PASS | 243 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml --workspace --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` |
| executor | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop --features turnloop/executor -- --test-threads=1` |
| executor | PASS | 26 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-http -- --test-threads=1` |
| executor | PASS | 8 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-tls -- --test-threads=1` |
| executor | PASS | 20 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mongodb -- --test-threads=1` |
| executor | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mysql -- --test-threads=1` |
| executor | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-postgres -- --test-threads=1` |
| executor | PASS | 10 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-redis -- --test-threads=1` |
| executor | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-smtp -- --test-threads=1` |
| executor | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-websocket -- --test-threads=1` |
| executor | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-io --features turnloop/executor -- --test-threads=1` |
| executor | PASS | 56 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-contract --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` |
| all-features | PASS | 276 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml --workspace --all-features -- --test-threads=1` |
| all-features | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop --all-features -- --test-threads=1` |
| all-features | PASS | 38 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-http --all-features -- --test-threads=1` |
| all-features | PASS | 11 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-tls --all-features -- --test-threads=1` |
| all-features | PASS | 21 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mongodb --all-features -- --test-threads=1` |
| all-features | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mysql --all-features -- --test-threads=1` |
| all-features | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-postgres --all-features -- --test-threads=1` |
| all-features | PASS | 11 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-redis --all-features -- --test-threads=1` |
| all-features | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-smtp --all-features -- --test-threads=1` |
| all-features | PASS | 8 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-websocket --all-features -- --test-threads=1` |
| all-features | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-io --all-features -- --test-threads=1` |
| all-features | PASS | 56 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-contract --all-features -- --test-threads=1` |

## Windows runtime handoff

All commands below are **UNRUN locally (no Windows host)**:

| Result | Command |
| --- | --- |
| UNRUN | `cargo test --locked -p turnloop --lib backend::iocp::process -- --nocapture` |
| UNRUN | `cargo test --locked -p turnloop-contract --test windows_lifetimes -- --nocapture` |
| UNRUN | `cargo test --locked -p turnloop-contract --test allocations -- --test-threads=1` |
| UNRUN | `python3 scripts/ci/run-tests.py native` (on Windows; all three modes) |

The lifetime suite must also run with normal libtest parallelism to check poison
recovery/isolation. Do not interpret native macOS zero-count Windows test binaries
as Windows passes. Linux runtime, WASI/web runtime and ignored service bodies are
UNRUN in this lane; SQL remains UNRUN (sandbox).
