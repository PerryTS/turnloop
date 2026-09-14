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
