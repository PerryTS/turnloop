# adapters-db command ledger

| Command | Result | Log |
| --- | --- | --- |
| `cargo check -p turnloop-postgres --features turnloop` | FAIL | `initial-pg.log` |
| `cargo check -p turnloop-postgres -p turnloop-mysql --all-features` | FAIL | `sql-check.log` |
| `cargo check -p turnloop-postgres -p turnloop-mysql --all-features` | FAIL | `pools-check.log` |
| `cargo check -p turnloop-postgres -p turnloop-mysql -p turnloop-redis -p turnloop-smtp --all-features` | PASS | `four-check.log` |
| `cargo check -p turnloop-mongodb -p turnloop-redis --all-features` | FAIL | `mongo-check.log` |
| `cargo check --workspace --all-features` | PASS | `mongo-check-fixed.log` |
| `python3 scripts/ci/install-wasm-toolchain.py` | PASS | `wasm-toolchain.log` |
| `cargo test -p turnloop-postgres -p turnloop-redis --features turnloop --test asynchronous -- --test-threads=1` | PASS | `first-async-tests.log` |
| `cargo test -p turnloop-postgres -p turnloop-mysql -p turnloop-redis -p turnloop-smtp -p turnloop-mongodb --features turnloop --test asynchronous -- --test-threads=1` | PASS | `five-async-tests.log` |
| `python3 scripts/test-servers.py --services redis run cargo test -p turnloop-redis --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture` | PASS | `redis-real.log` |
| `cargo fmt --all` | PASS | `format.log` |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `strict-first.log` |
| `python3 scripts/test-servers.py --services mongodb run sh .tools/adapters-db/mongo-servers.sh` | PASS | `mongodb-real.log` |
| `cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `strict-fix.log` |
| `python3 scripts/test-servers.py --services smtp run cargo test -p turnloop-smtp --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture` | PASS | `smtp-real.log` |
| `sh -c '. .tools/wasm-env.sh; cargo check -p turnloop --features executor --target wasm32-wasip2'` | PASS | `dns-p2-check.log` |
| `bash scripts/ci/install-wasmtime.sh` | FAIL | `install-wasmtime.log` |
| `cargo check --workspace --all-features` | FAIL | `pool-ack-check.log` |
| `cargo test -p turnloop-io --lib` | PASS | `dns-native.log` |
| `cargo test -p turnloop-postgres -p turnloop-mysql -p turnloop-mongodb --features turnloop --test asynchronous -- --test-threads=1` | PASS | `lifecycle-tests.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2'` | FAIL | `p2-all.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2'` | PASS | `p2-all-dns.log` |
