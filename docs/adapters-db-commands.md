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
| `cargo test -p turnloop-postgres -p turnloop-mysql --features turnloop --test asynchronous -- --test-threads=1` | PASS | `async-alloc-sql.log` |
| `cargo test -p turnloop-redis -p turnloop-mongodb -p turnloop-smtp --features turnloop --test asynchronous -- --test-threads=1` | PASS | `async-alloc-rest.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/test-servers.py --services redis run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-redis'` | FAIL | `wasi-redis-real.log` |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `strict-second.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/test-servers.py --services mongodb run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-mongodb'` | PASS | `wasi-mongo-real.log` |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `strict-third.log` |
| `python3 scripts/test-servers.py --services redis run cargo test -p turnloop-redis --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture` | FAIL | `redis-failover-native.log` |
| `sh -c '. .tools/wasm-env.sh; cargo clippy --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks'` | PASS | `p2-clippy.log` |
| `sh -c '. .tools/wasm-env.sh; cargo clippy --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks'` | PASS | `web-clippy.log` |
| `python3 scripts/test-servers.py --services redis run cargo test -p turnloop-redis --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture` | FAIL | `redis-failover-restart.log` |
| `cargo +stable check --workspace --all-features` | PASS | `stable-all.log` |
| `bash scripts/ci/no-tokio.sh` | PASS | `no-tokio.log` |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS | `ci-unit.log` |
| `python3 scripts/ci/soak.py` | PASS | `soak.log` |
| `python3 scripts/test-servers.py --services mongodb run sh .tools/adapters-db/mongo-servers.sh` | PASS | `mongo-retry-cleanup.log` |
| `cargo check --workspace --all-features --examples` | PASS | `examples-check.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-linux-cc AR_x86_64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-ar cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `linux-clippy.log` |
| `env CC_x86_64_pc_windows_msvc=clang AR_x86_64_pc_windows_msvc=llvm-ar cargo clippy --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `windows-lib.log` |
| `cargo test --workspace` | FAIL | `workspace-default.log` |
| `python3 scripts/test-servers.py --services redis run cargo test -p turnloop-redis --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture` | PASS | `redis-failover-final.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-linux-cc AR_x86_64_unknown_linux_gnu=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-ar cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `linux-clippy-fixed.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-ar cargo clippy --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `windows-lib-zig.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/test-servers.py --services redis run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-redis'` | FAIL | `wasi-redis-failover.log` |
| `cargo test --workspace -- --test-threads=1` | PASS | `workspace-serial.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | `windows-lib-fixed.log` |
| `cargo test -p turnloop-mongodb --features turnloop --test asynchronous --test spec_selection -- --test-threads=1` | FAIL | `retained-selection.log` |
| `env ZIG_GLOBAL_CACHE_DIR=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-cache CC_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-windows-cc AR_x86_64_pc_windows_msvc=/Users/amlug/projects/perry/windlass-lanes/adapters-db/.tools/adapters-db/zig-win-ar cargo clippy --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | `windows-lib-final.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/test-servers.py --services redis run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-redis'` | PASS | `wasi-redis-stack-fixed.log` |
| `cargo test -p turnloop-mongodb --features turnloop --test asynchronous --test spec_selection -- --test-threads=1` | PASS | `retained-selection-fixed.log` |
| `cargo test -p turnloop-mongodb --features turnloop --test asynchronous -- --test-threads=1` | PASS | `mongo-cancel-idle.log` |
| `cargo fmt --all` | PASS | `format-all.log` |
| `sh -c '. .tools/wasm-env.sh; python3 scripts/test-servers.py --services smtp run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-smtp'` | PASS | `smtp-wasi-real.log` |
