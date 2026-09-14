# adapters-net verification commands

| UTC | Result | Command | Log |
| --- | --- | --- | --- |
| 2026-09-14T19:38:36.592604+00:00 | FAIL | `cargo test -p turnloop-websocket --features turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/websocket.log` |
| 2026-09-14T19:39:29.682842+00:00 | FAIL | `cargo test -p turnloop-websocket --features turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/websocket-fixed.log` |
| 2026-09-14T19:40:26.389002+00:00 | FAIL | `cargo test -p turnloop-websocket --features turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/websocket-boundary.log` |
| 2026-09-14T19:41:50.832011+00:00 | PASS | `cargo check --workspace --all-features` | `.tools/adapters-net/adapters-check.log` |
| 2026-09-14T19:42:33.013154+00:00 | PASS | `python3 scripts/ci/h2spec.py` | `.tools/adapters-net/h2spec.log` |
| 2026-09-14T19:44:03.520599+00:00 | FAIL | `cargo test -p turnloop-http -p turnloop-websocket --features turnloop-http/turnloop,turnloop-websocket/turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/async-regressions.log` |
| 2026-09-14T19:45:01.641462+00:00 | FAIL | `cargo test -p turnloop-http -p turnloop-websocket --features turnloop-http/turnloop,turnloop-websocket/turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/async-regressions-framing.log` |
| 2026-09-14T19:45:42.343409+00:00 | PASS | `cargo test -p turnloop-http -p turnloop-websocket --features turnloop-http/turnloop,turnloop-websocket/turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/async-regressions-fixed.log` |
| 2026-09-14T19:47:35.208103+00:00 | PASS | `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode all-features` | `.tools/adapters-net/interop.log` |
| 2026-09-14T19:49:49.385770+00:00 | PASS | `cargo test -p turnloop-websocket --features turnloop --test async_allocations` | `.tools/adapters-net/async-allocations.log` |
| 2026-09-14T19:50:43.558652+00:00 | FAIL | `cargo test -p turnloop-io --test allocations -- --test-threads=1` | `.tools/adapters-net/tcp-allocations.log` |
| 2026-09-14T19:50:58.345200+00:00 | PASS | `cargo test -p turnloop-io --test allocations -- --test-threads=1` | `.tools/adapters-net/tcp-allocations-fixed.log` |
| 2026-09-14T19:51:24.520027+00:00 | PASS | `python3 scripts/ci/install-wasm-toolchain.py` | `.tools/adapters-net/wasm-sdk.log` |
| 2026-09-14T19:51:56.201704+00:00 | FAIL | `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-native.log` |
| 2026-09-14T19:53:05.820201+00:00 | PASS | `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-native-fixed.log` |
| 2026-09-14T19:53:55.992366+00:00 | PASS | `cargo test -p turnloop-http --features turnloop --test asynchronous node_https_and_http2 -- --nocapture` | `.tools/adapters-net/node-tls.log` |
| 2026-09-14T19:54:12.441094+00:00 | FAIL | `bash scripts/ci/install-wasmtime.sh` | `.tools/adapters-net/wasmtime.log` |
| 2026-09-14T19:56:16.401692+00:00 | PASS | `cargo test --workspace --all-features -- --test-threads=1` | `.tools/adapters-net/workspace-test.log` |
| 2026-09-14T19:56:21.062107+00:00 | PASS | `python3 scripts/ci/install-tools.py wasmtime --destination .tools/adapters-net/bin` | `.tools/adapters-net/wasmtime-owned.log` |
