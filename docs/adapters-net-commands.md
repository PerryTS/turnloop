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
| 2026-09-14T19:59:58.248166+00:00 | PASS | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | `.tools/adapters-net/protocol-wasip2.log` |
| 2026-09-14T20:00:37.827933+00:00 | PASS | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | `.tools/adapters-net/protocol-wasip3.log` |
| 2026-09-14T20:03:09.893278+00:00 | FAIL | `cargo test -p turnloop-http --features turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/http-continue-curl.log` |
| 2026-09-14T20:04:12.756224+00:00 | PASS | `cargo test -p turnloop-http --features turnloop --test asynchronous -- --test-threads=1` | `.tools/adapters-net/http-continue-fixed.log` |
| 2026-09-14T20:04:37.399405+00:00 | PASS | `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-native-latest.log` |
| 2026-09-14T20:06:12.938796+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-p2.log` |
| 2026-09-14T20:06:27.126670+00:00 | FAIL | `python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v` | `.tools/adapters-net/python-gates.log` |
| 2026-09-14T20:06:31.106933+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-web.log` |
| 2026-09-14T20:06:48.835531+00:00 | PASS | `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-p3.log` |
| 2026-09-14T20:07:09.169615+00:00 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features` | `.tools/adapters-net/stable-native.log` |
| 2026-09-14T20:09:07.433351+00:00 | PASS | `python3 -m unittest discover -s scripts/ci -p test_feature_modes.py -v` | `.tools/adapters-net/feature-gates-fixed.log` |
| 2026-09-14T20:09:10.471890+00:00 | PASS | `python3 scripts/ci/install-web-tools.py` | `.tools/adapters-net/web-tools.log` |
| 2026-09-14T20:10:36.246278+00:00 | PASS | `cargo test -p turnloop-tls --features turnloop --test async_allocations` | `.tools/adapters-net/tls-record-allocations.log` |
| 2026-09-14T20:10:37.118436+00:00 | PASS | `bash scripts/ci/no-tokio.sh` | `.tools/adapters-net/no-tokio.log` |
| 2026-09-14T20:10:56.312773+00:00 | PASS | `python3 scripts/ci/soak.py` | `.tools/adapters-net/soak.log` |
| 2026-09-14T20:11:32.601557+00:00 | FAIL | `python3 scripts/ci/run-tests.py node` | `.tools/adapters-net/node-contracts.log` |
| 2026-09-14T20:11:45.578945+00:00 | PASS | `python3 scripts/ci/install-tools.py cargo-deny --destination .tools/adapters-net/bin` | `.tools/adapters-net/deny-tools.log` |
| 2026-09-14T20:11:45.666891+00:00 | PASS | `cargo test -p turnloop-http --features turnloop --test asynchronous node_https_via_authenticated -- --nocapture` | `.tools/adapters-net/http-proxy.log` |
| 2026-09-14T20:11:59.725594+00:00 | PASS | `cargo deny --locked check advisories bans licenses sources` | `.tools/adapters-net/deny.log` |
| 2026-09-14T20:12:09.797898+00:00 | PASS | `python3 scripts/ci/run-tests.py node` | `.tools/adapters-net/node-contracts-cache.log` |
| 2026-09-14T20:13:01.783098+00:00 | PASS | `python3 scripts/ci/install-tools.py zizmor actionlint shellcheck --destination .tools/adapters-net/bin` | `.tools/adapters-net/workflow-tools.log` |
| 2026-09-14T20:13:05.415634+00:00 | FAIL | `cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-linux.log` |
| 2026-09-14T20:13:58.582334+00:00 | PASS | `python3 scripts/ci/lint-workflows.py` | `.tools/adapters-net/workflow-lint.log` |
| 2026-09-14T20:14:16.472023+00:00 | PASS | `cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-linux-zig.log` |
| 2026-09-14T20:14:24.822781+00:00 | PASS | `python3 scripts/ci/check-paths.py` | `.tools/adapters-net/paths.log` |
| 2026-09-14T20:14:44.874586+00:00 | FAIL | `python3 scripts/ci/run-tests.py web --browser chrome` | `.tools/adapters-net/chrome-contracts.log` |
| 2026-09-14T20:15:29.607607+00:00 | FAIL | `cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-windows.log` |
| 2026-09-14T20:16:07.546926+00:00 | FAIL | `cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-windows-zig.log` |
| 2026-09-14T20:17:40.092743+00:00 | FAIL | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | `.tools/adapters-net/protocol-wasip2-final.log` |
| 2026-09-14T20:17:41.395520+00:00 | PASS | `python3 scripts/ci/feature_modes.py` | `.tools/adapters-net/feature-coverage.log` |
| 2026-09-14T20:17:52.009366+00:00 | FAIL | `cargo test -p turnloop-io --target wasm32-wasip2 --test streams resolve_and_cancel -- --nocapture --test-threads=1` | `.tools/adapters-net/io-wasi-dns.log` |
| 2026-09-14T20:17:55.173968+00:00 | PASS | `python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v` | `.tools/adapters-net/python-gates-final.log` |
| 2026-09-14T20:19:16.550532+00:00 | PASS | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | `.tools/adapters-net/protocol-wasip2-complete.log` |
| 2026-09-14T20:19:58.854281+00:00 | FAIL | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | `.tools/adapters-net/protocol-wasip3-complete.log` |
| 2026-09-14T20:21:50.253847+00:00 | PASS | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | `.tools/adapters-net/protocol-wasip3-harness.log` |
| 2026-09-14T20:22:00.692110+00:00 | PASS | `cargo fmt --all --check` | `.tools/adapters-net/fmt.log` |
| 2026-09-14T20:22:03.110308+00:00 | PASS | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-default.log` |
| 2026-09-14T20:22:05.120818+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-final.log` |
| 2026-09-14T20:22:07.043148+00:00 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features` | `.tools/adapters-net/stable-final.log` |
| 2026-09-14T20:22:08.520490+00:00 | PASS | `env 'RUSTDOCFLAGS=-D warnings' cargo doc --locked --workspace --all-features --no-deps` | `.tools/adapters-net/rustdoc.log` |
| 2026-09-14T20:25:20.002254+00:00 | PASS | `python3 scripts/ci/run-tests.py native` | `.tools/adapters-net/native-modes.log` |
| 2026-09-14T20:25:22.364698+00:00 | PASS | `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode all-features` | `.tools/adapters-net/interop-final.log` |
| 2026-09-14T20:25:25.776765+00:00 | PASS | `python3 scripts/ci/h2spec.py` | `.tools/adapters-net/h2spec-final.log` |
| 2026-09-14T20:25:29.423567+00:00 | PASS | `cargo publish --dry-run --locked --allow-dirty -p turnloop -p turnloop-io` | `.tools/adapters-net/io-publish-dry-run.log` |
| 2026-09-14T20:25:56.875862+00:00 | FAIL | `cargo test -p turnloop-http -p turnloop-io -p turnloop-websocket --all-features --test asynchronous --test streams --test async_allocations -- --test-threads=1` | `.tools/adapters-net/http-final-drain.log` |
| 2026-09-14T20:27:57.206420+00:00 | FAIL | `cargo test -p turnloop-http -p turnloop-io -p turnloop-websocket --all-features --test asynchronous --test streams --test async_allocations -- --test-threads=1` | `.tools/adapters-net/http-final-drain-fixed.log` |
| 2026-09-14T20:28:56.789734+00:00 | PASS | `cargo test -p turnloop-http -p turnloop-io -p turnloop-websocket --all-features --test asynchronous --test streams --test async_allocations -- --test-threads=1` | `.tools/adapters-net/http-final-drain-complete.log` |
| 2026-09-14T20:29:30.287669+00:00 | PASS | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | `.tools/adapters-net/wasi-core-p2.log` |
| 2026-09-14T20:29:57.655493+00:00 | PASS | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | `.tools/adapters-net/wasi-core-p3.log` |
| 2026-09-14T20:30:02.447581+00:00 | PASS | `python3 scripts/ci/release.py order` | `.tools/adapters-net/release-order.log` |
| 2026-09-14T20:30:03.764994+00:00 | PASS | `python3 scripts/ci/check-paths.py` | `.tools/adapters-net/paths-final.log` |
| 2026-09-14T20:30:20.937753+00:00 | PASS | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | `.tools/adapters-net/protocol-p2-last.log` |
| 2026-09-14T20:30:45.826850+00:00 | PASS | `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | `.tools/adapters-net/protocol-p3-last.log` |
| 2026-09-14T20:30:46.688691+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-last.log` |
| 2026-09-14T20:30:58.062220+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-p2-last.log` |
| 2026-09-14T20:31:10.208163+00:00 | PASS | `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-p3-last.log` |
| 2026-09-14T20:31:23.750087+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-web-last.log` |
| 2026-09-14T20:31:24.556579+00:00 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features` | `.tools/adapters-net/stable-last.log` |
| 2026-09-14T20:31:25.092939+00:00 | PASS | `cargo fmt --all --check` | `.tools/adapters-net/fmt-last.log` |
| 2026-09-14T20:32:09.118344+00:00 | PASS | `cargo clippy --locked --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-windows-libs.log` |
| 2026-09-14T20:32:20.306389+00:00 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | `.tools/adapters-net/clippy-linux-last.log` |
| 2026-09-14T20:32:36.709222+00:00 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-wasip2` | `.tools/adapters-net/stable-p2.log` |
| 2026-09-14T20:32:55.111931+00:00 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown` | `.tools/adapters-net/stable-web.log` |
| 2026-09-14T20:33:05.034905+00:00 | PASS | `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode all-features` | `.tools/adapters-net/interop-last.log` |
| 2026-09-14T20:33:09.893439+00:00 | PASS | `python3 scripts/ci/run-tests.py loom` | `.tools/adapters-net/loom.log` |
| 2026-09-14T20:33:10.263757+00:00 | FAIL | `python3 scripts/ci/run-tests.py miri` | `.tools/adapters-net/miri.log` |
| 2026-09-14T20:34:50.599677+00:00 | PASS | `cargo test --workspace --all-features -- --test-threads=1` | `.tools/adapters-net/workspace-final.log` |
| 2026-09-14T20:37:30.197824+00:00 | PASS | `cargo miri setup` | `.tools/adapters-net/miri-setup.log` |
| 2026-09-14T20:37:48.755027+00:00 | PASS | `env MIRI_SYSROOT=/Users/amlug/projects/perry/windlass-lanes/adapters-net/.tools/adapters-net/miri-sysroot python3 scripts/ci/run-tests.py miri` | `.tools/adapters-net/miri-local.log` |
| 2026-09-14T20:38:44.978587+00:00 | PASS | `bash scripts/ci/no-tokio.sh` | `.tools/adapters-net/no-tokio-last.log` |
| 2026-09-14T20:39:06.326604+00:00 | PASS | `python3 scripts/ci/soak.py` | `.tools/adapters-net/soak-last.log` |
| 2026-09-14T20:39:07.554706+00:00 | PASS | `cargo deny --locked check advisories bans licenses sources` | `.tools/adapters-net/deny-last.log` |
