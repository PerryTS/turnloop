# Native verification commands

Every row was executed by `python3 scripts/ci/run-tests.py native` on macOS arm64.
Ignored or cfg-excluded test bodies are not included in the pass counts.

| Mode | Result | Passed tests | Command |
| --- | --- | --- | --- |
| default (default features) | PASS | 236 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml --workspace -- --test-threads=1` |
| default (default features) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop -- --test-threads=1` |
| default (default features) | PASS | 26 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-http -- --test-threads=1` |
| default (default features) | PASS | 8 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-tls -- --test-threads=1` |
| default (default features) | PASS | 20 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mongodb -- --test-threads=1` |
| default (default features) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mysql -- --test-threads=1` |
| default (default features) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-postgres -- --test-threads=1` |
| default (default features) | PASS | 10 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-redis -- --test-threads=1` |
| default (default features) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-smtp -- --test-threads=1` |
| default (default features) | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-websocket -- --test-threads=1` |
| default (default features) | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-io -- --test-threads=1` |
| default (default features) | PASS | 49 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-contract -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 243 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml --workspace --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop --features turnloop/executor -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 26 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-http -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 8 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-tls -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 20 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mongodb -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mysql -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-postgres -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 10 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-redis -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 12 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-smtp -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-websocket -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-io --features turnloop/executor -- --test-threads=1` |
| executor (--features turnloop/executor,turnloop-contract/executor) | PASS | 56 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-contract --features turnloop/executor,turnloop-contract/executor -- --test-threads=1` |
| all-features (--all-features) | PASS | 276 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml --workspace --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 38 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-http --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 11 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-tls --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 21 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mongodb --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-mysql --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-postgres --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 11 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-redis --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 13 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-smtp --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 8 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-websocket --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 3 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-io --all-features -- --test-threads=1` |
| all-features (--all-features) | PASS | 56 | `cargo +nightly-2026-08-20 test --locked --manifest-path ./Cargo.toml -p turnloop-contract --all-features -- --test-threads=1` |
