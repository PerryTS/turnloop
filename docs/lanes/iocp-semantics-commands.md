# IOCP semantics: expanded native CI commands

macOS arm64, 2026-09-15. Every command emitted by
`python3 scripts/ci/run-tests.py native` is recorded below. Counts come from the
runner's executed-test parser; ignored and cfg-excluded Windows tests do not count.
See [the lane report](iocp-semantics.md) for other commands and runtime limitations.

| Mode | Exact command | Result |
| --- | --- | --- |
| default | `cargo +nightly-2026-08-20 metadata --format-version 1 --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --no-deps` | **PASS** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --workspace -- --test-threads=1` | **PASS executed 252 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop -- --test-threads=1` | **PASS executed 14 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-http -- --test-threads=1` | **PASS executed 26 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-tls -- --test-threads=1` | **PASS executed 11 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mongodb -- --test-threads=1` | **PASS executed 21 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mysql -- --test-threads=1` | **PASS executed 12 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-postgres -- --test-threads=1` | **PASS executed 18 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-redis -- --test-threads=1` | **PASS executed 10 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-smtp -- --test-threads=1` | **PASS executed 12 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-websocket -- --test-threads=1` | **PASS executed 3 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-io -- --test-threads=1` | **PASS executed 5 tests** |
| default | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-contract -- --test-threads=1` | **PASS executed 51 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --workspace --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` | **PASS executed 259 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop --features turnloop/executor -- --test-threads=1` | **PASS executed 15 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-http -- --test-threads=1` | **PASS executed 26 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-tls -- --test-threads=1` | **PASS executed 11 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mongodb -- --test-threads=1` | **PASS executed 21 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mysql -- --test-threads=1` | **PASS executed 12 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-postgres -- --test-threads=1` | **PASS executed 18 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-redis -- --test-threads=1` | **PASS executed 10 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-smtp -- --test-threads=1` | **PASS executed 12 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-websocket -- --test-threads=1` | **PASS executed 3 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-io --features turnloop/executor -- --test-threads=1` | **PASS executed 5 tests** |
| executor | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-contract --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` | **PASS executed 58 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml --workspace --all-features -- --test-threads=1` | **PASS executed 308 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop --all-features -- --test-threads=1` | **PASS executed 15 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-http --all-features -- --test-threads=1` | **PASS executed 38 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-tls --all-features -- --test-threads=1` | **PASS executed 14 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mongodb --all-features -- --test-threads=1` | **PASS executed 25 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-mysql --all-features -- --test-threads=1` | **PASS executed 16 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-postgres --all-features -- --test-threads=1` | **PASS executed 24 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-redis --all-features -- --test-threads=1` | **PASS executed 13 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-smtp --all-features -- --test-threads=1` | **PASS executed 16 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-websocket --all-features -- --test-threads=1` | **PASS executed 8 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-io --all-features -- --test-threads=1` | **PASS executed 5 tests** |
| all features | `cargo +nightly-2026-08-20 test --locked --manifest-path /Users/amlug/projects/perry/windlass-lanes/iocp-semantics/Cargo.toml -p turnloop-contract --all-features -- --test-threads=1` | **PASS executed 58 tests** |
