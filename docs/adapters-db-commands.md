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
