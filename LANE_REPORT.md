# adapters-net wave 3

Work in progress from 67690e9 (0.1.0-alpha.1). No Git writes; integrator owns commits.
Read DESIGN.md, CONTRIBUTING.md, integration report and relevant core/HTTP/Windows/WASM/CI reports. No applicable AGENTS.md.

## Implemented

- Implementing shared `turnloop-io`, executor connection/deadline support, async TLS/HTTP/WebSocket adapters, examples and gates.

## Verification

No implementation verification completed yet. Native and cross-target checks will be recorded here with exact commands. Linux/Windows runtime UNRUN (no hosts). SQL UNRUN (sandbox limitations supplied by user).

## Deviations / questions

- Existing AsyncIo retains per-operation staging for safe cancellation. Adapters must reuse it and their protocol buffers; borrowed futures-io buffers cannot remain submitted after a future drops.
- Existing web fetch backend currently exposes only a bounded complete-body GET, without response status/headers or request options. A richer async HTTP facade needs a backend extension.
- Upstream rustls/tungstenite allocations require measurement separately from adapter scratch.
- No DESIGN changes or dependency-soak exceptions introduced.

## Next steps

Complete adapters, runtime/interop/allocation/cancellation/no-spin tests, examples/docs and required CI wiring. Run serial builds with CARGO_BUILD_JOBS=4.

## Checkpoint 1

Implemented shared streams/listener/write-flush/cancellation guard and timeout
helpers; executor connect, resolve and absolute-deadline APIs; TLS client/server
stream; HTTP/1 and HTTP/2 event drivers, pooled client with redirect/proxy/streaming
decompression policy, accept loop and shutdown signal; WebSocket upgrade/frames;
HTTPS, TLS echo and WebSocket examples. Implementation and tests are still in progress.

| Command | Result |
| --- | --- |
| `cargo check -p turnloop-io -p turnloop-tls --all-features` | PASS |
| `cargo test -p turnloop-io -p turnloop-tls --all-features --test streams --test asynchronous -- --test-threads=1` | PASS, 3 tests; real TCP and TLS, ALPN, fragmented reads, close_notify, deadline |
| `cargo check -p turnloop-http --features turnloop` | Initial FAIL borrow conflict; fixed, PASS |
| `cargo test -p turnloop-http --features turnloop --test asynchronous -- --test-threads=1` | PASS twice, 2 tests, 4 × 32-KiB HTTP/1 and HTTP/2 pooled exchanges each, server drain |
| `cargo fmt --all` | PASS |

Subsequent commands are recorded in [the command ledger](docs/adapters-net-commands.md).
All checks not explicitly reported PASS remain UNRUN. No full-workspace or
cross-target completion claim yet.
