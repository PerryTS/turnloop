# adapters-net wave 3

Implementation and local verification are complete for the providers present in this clone. Full Windows validation remains blocked by the missing production IOCP provider. Browser execution needs an outside-sandbox rerun. This is not an all-platform release-readiness claim.

Started from 67690e9, version 0.1.0-alpha.1. Read DESIGN.md completely, CONTRIBUTING.md, the integration report and relevant HTTP/core/Windows/WASM/CI/database lane reports. No applicable AGENTS.md. The integrator owns commits; this lane changed the working tree only.

## Implemented

- Publishable `crates/turnloop-io`, with crates.io metadata: shared futures-io stream contract over AsyncIo/TCP/pipes, listener ownership, borrowed sans-I/O event/output driving, cancellation guards and absolute deadlines built on Timeout. Executor handles gained connect, resolve, absolute-timeout and web-fetch futures. Submitted buffers retain AsyncIo's cancellation-safe ownership.
- Optional `turnloop` features on Postgres, MySQL, Redis, SMTP, MongoDB, TLS, HTTP and WebSocket. Database/mail adapters implement the same small shared driver contract; authentication and TLS-transition decisions remain explicit host-managed core events. No duplicate transport loops were added to those five crates.
- Generic client/server `TlsStream<S>` with handshake deadlines, ALPN, retained bounded buffers, fragmented reads/writes and graceful close_notify. Native and WASI use the same rustls unbuffered cores.
- HTTP/1.1 and HTTP/2 stream drivers; per-origin pooled client; replayable redirects; authenticated proxy CONNECT; incremental decompression; streamed uploads and borrowed response chunks; absolute request deadlines and cancellation. Expect/100-continue handles informational responses, timer fallback and early final rejection. Graceful GOAWAY drains an accepted response and prevents reuse of the draining connection.
- Server listener/accept ownership and local service tasks, HTTP/1 keep-alive, HTTP/2, stop-accept/drain shutdown and drop cancellation. Idle HTTP/1 and HTTP/2 connections obey the no-spin contract.
- Async WebSocket client/server over HTTP upgrade, preserving coalesced unread bytes; frame streaming, ping/pong and close handshake. Browser usage documents the existing host WebSocket payload path and bounded host-fetch GET capability.
- Built examples: HTTPS GET; TLS echo with HTTP/1.1 + HTTP/2 ALPN; WebSocket echo client and server. Crate READMEs and rustdoc have getting-started sections. CONTRIBUTING documents the adapter/gate workflow; the integration report includes the current status and publish order with turnloop-io before consumers.
- Required CI uses test-target required-features and positive execution counts. Native jobs independently execute the adapter crate. Existing interop jobs run the async suites; h2spec targets the async server; required protocol-WASI jobs include real socket/adapter/allocation suites; Node/browser contracts include executor fetch/abort. All-target/all-feature lint builds examples. No required job or skip policy was relaxed.
- Corrected an inherited CI feature-coverage mismatch: six public features now map to their actual native/WASI/web jobs and forwarding/runtime commands. All eighteen native matrix arms remain mandatory. Negative-control Python tests reject missing jobs, forwarding, runtime commands and unknown features.

## Verification

[The exact command ledger](docs/adapters-net-commands.md) records PASS/FAIL for all instrumented commands, including intermediate failures and corrected reruns. Logs are retained in `.tools/adapters-net/`. Commands below summarize the final results; explicit UNRUN commands follow. `CARGO_BUILD_JOBS=4`; large builds ran serially.

| Command | Final result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS, stable 1.97.1 |
| `cargo test --workspace --all-features -- --test-threads=1` | PASS, final tree: 270 test passes, including doctests; service-dependent ignored tests run separately where available |
| `python3 scripts/ci/run-tests.py native` | PASS, all three macOS feature modes, workspace and independently counted members; 1,257 passes across repeated mode/member runs. Later GOAWAY/cancellation additions also pass the final all-feature workspace and interop runs |
| `env RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps` | PASS |
| `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode all-features` | PASS, final run: all eight suites, 38 tests, zero ignored; real curl, Node HTTP/HTTPS/HTTP2/WebSocket and authenticated CONNECT |
| `python3 scripts/ci/h2spec.py` | PASS, strict 147/147, zero skipped/failed, async server |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | PASS, 42 tests in 11 independently counted suites, including real async TLS/HTTP/WebSocket/shared-I/O sockets and allocation gates |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | PASS, same 42 tests/11 suites, pinned nightly-2026-09-07 and Wasmtime 46 |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS, 59 passes: core release tests, debug/release contracts and release allocation gates |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | PASS, 64 passes across the same required profiles |
| `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS, Zig cross C toolchain |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL: this clone has no Windows `backend::Platform`; async executable/test targets need the IOCP provider merge. C dependencies were cross-compiled successfully after configuring Zig |
| `cargo clippy --locked --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS, generic libraries only; does not replace the failing full gate |
| `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-wasip2` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown` | PASS |
| `python3 scripts/ci/run-tests.py node` | PASS, 12 actual tests; fixture observed five fetches, two aborts and six WebSockets/7,937 echoed bytes |
| `python3 scripts/ci/run-tests.py loom` | PASS, six models |
| `env MIRI_SYSROOT="$PWD/.tools/adapters-net/miri-sysroot" cargo miri setup` | PASS; first default-cache attempt failed sandbox permissions, then setup used this writable path |
| `env MIRI_SYSROOT="$PWD/.tools/adapters-net/miri-sysroot" python3 scripts/ci/run-tests.py miri` | PASS, both required pure-Rust test filters executed; isolation unchanged |
| `bash scripts/ci/no-tokio.sh` | PASS, every policy target plus all-target graph, default and all features |
| `python3 scripts/ci/soak.py` | PASS, 251 locked versions; seven-day policy retained; base branch's one rustls security exception unchanged |
| `cargo deny --locked check advisories bans licenses sources` | PASS |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS, 91 tests |
| `python3 scripts/ci/feature_modes.py` | PASS, six public features and eighteen mandatory native arms |
| `python3 scripts/ci/lint-workflows.py` | PASS, actionlint/zizmor/ShellCheck and strict existing compatibility validation |
| `python3 scripts/ci/check-paths.py` | PASS |
| `python3 scripts/ci/release.py order` | PASS, includes turnloop-io before consumers |
| `cargo publish --dry-run --locked --allow-dirty -p turnloop -p turnloop-io` | PASS, including extracted packaged dependency rebuild; no upload |

Earlier bootstrap commands not captured by the command logger: `cargo check -p turnloop-io -p turnloop-tls --all-features` PASS; `cargo test -p turnloop-io -p turnloop-tls --all-features --test streams --test asynchronous -- --test-threads=1` PASS (three tests at that checkpoint); `cargo check -p turnloop-http --features turnloop` initially FAIL (borrow conflict), then PASS after correction; `cargo check --workspace --all-features` PASS; `cargo fmt --all` PASS. Subsequent strict checks supersede those snapshots.

### Subjects proved by new tests

- Shared driver: 1,000 warmed real TCP request/reply operations allocate zero times. Native DNS cancellation, slot reuse and completed-future repoll are exercised repeatedly; WASI explicitly verifies Unsupported DNS and slot release.
- HTTP/1: 100 exchanges match the sans-I/O baseline of 300 core-owned decoded-head allocations, with zero adapter overhead. HTTP/2: 100 DATA deliveries plus flow-credit writes allocate zero times. The counters are calibrated with known allocations.
- WebSocket: 100 received frames and 100 sent frames allocate zero times. Separate real TCP and Node peers exercise upgrade, frame payloads, ping/pong and close.
- TLS: 100 warmed bidirectional real-TCP records incur exactly 400 existing rustls-owned allocations and zero adapter overhead. Separate tests verify ALPN, a 32,768-byte payload consumed one byte at a time, close_notify and silent-peer handshake timeout.
- Idle keep-alive: HTTP/1 and HTTP/2 each undergo sixty actual timer expiries across 0.5/2/10-ms deadlines while a real connection remains idle; bounded turn/wait counters prove no spinning.
- Cancellation: drop a body future, drop pooled HTTP/1 and HTTP/2 requests mid-body, and drop the server with an in-flight request. Tests verify single closure/task completion and fresh-connection recovery; poisoned leases never return to the pool.
- Interop includes verified HTTPS, authenticated CONNECT, redirects/gzip, 100 HTTP/2 request/response rounds, a 262,144-byte HTTP/2 exchange crossing flow-control windows, and all 147 strict h2spec cases.

### Environment and UNRUN work

| Command / runtime | Status and reason |
| --- | --- |
| `python3 scripts/ci/run-tests.py web --browser chrome` | UNRUN (sandbox): compilation and matching ChromeDriver startup succeeded, but Chrome exited during session creation. Wrapper command reports FAIL; zero browser tests executed |
| `python3 scripts/ci/run-tests.py web --browser firefox` | UNRUN, Firefox not installed |
| `python3 scripts/ci/run-tests.py native` on Linux and Windows | UNRUN, no hosts; cross-Clippy is recorded separately |
| `python3 scripts/ci/instructions.py` | UNRUN, requires Linux x86_64/Gungraun/Valgrind; committed baseline unchanged |
| Full `scripts/test-servers.py` SQL service run and Docker CI | UNRUN (sandbox/no Docker): PostgreSQL shmget denied and MySQL initialization crashes, as supplied in the task; async HTTP fixture runs succeeded |
| GitHub required `ci-gate` on the integrated tree | UNRUN locally; workflow retains all prerequisites and Windows full-target failure must be resolved before claiming green |

WASI commands source `.tools/wasm-env.sh` (verified WASI SDK 34), use the repository's pinned p3 toolchain and the verified local Wasmtime 46 install. Node/browser tools use the pinned wasm-bindgen CLI and clone-local `WASM_PACK_CACHE`/`XDG_CACHE_HOME`. Cross C builds use clone-local Zig wrappers/caches. An inherited Wasmtime symlink into another clone initially caused an install failure; it was replaced with a symlink to this clone's verified install, without external writes. Initial failures, including Windows, Chrome, allocator/upgrade/framing/GOAWAY regressions and their fixes, remain in the command ledger.

## Deviations and proposed DESIGN clarifications

- No DESIGN.md edits, dependency-soak exceptions, weakened test thresholds or skip waivers were introduced. The core trait remains unchanged.
- Existing HTTP decoded heads and rustls plaintext records allocate. New gates assert their measured core baselines plus zero added adapter allocations; shared TCP, HTTP/2 DATA and WebSocket gates require absolute zero. Whole-stack HTTP/TLS allocation freedom is not achieved by this lane. Proposed clarification: explicitly distinguish adapter transport overhead from these already documented core/crypto allocations; eliminating the latter requires a separate core/crypto design change, not a gate waiver.
- Bounded retained transport/TLS/protocol storage avoids allocation per adapter read/write after warm-up. AsyncIo still owns staging needed to keep submitted memory valid through cancellation. No future leaves a borrowed futures-io buffer submitted after drop.
- The pooled facade serializes requests on each Client. Lower-level HTTP/2 supports multiple streams; separate Client instances can run concurrently. Response callbacks consume borrowed chunks synchronously; low-level events allow application-driven pauses between reads.
- WASI 0.2/0.3 socket/TLS paths run, but the existing native-blocking-pool resolver returns Unsupported there. High-level WASI clients use IP URLs; host-resolved streams can retain the original DNS name for TLS verification/SNI through the lower-level drivers. Host DNS capability remains an open backend design item.
- Browser host fetch currently supports only a bounded complete-body GET without response status/headers or request options. The facade exposes this capability plus abort/deadline. Browser-controlled redirects, decompression and TLS are available; explicit TLS/ALPN, proxy CONNECT, custom methods, streaming bodies and listening/raw sockets are unavailable. Richer browser HTTP needs a backend fetch capability revision. Host WebSocket carries payload bytes; framing/masking/control frames stay with the browser.
- Windows library code is generic and cross-checks, but the missing IOCP provider prevents the full all-targets gate. No mock backend or cfg exclusion masks that prerequisite.

## Open questions and next steps

1. Integrator: merge the IOCP provider, rerun full Windows Clippy/native modes and unchanged no-spin/allocation/cancellation contracts. Run Linux native/instruction gates and the complete required GitHub fan-in on the merged tree.
2. Rerun Chrome/Firefox and SQL/Docker suites outside this sandbox. Node web traffic and both real WASI versions have already run here.
3. Confirm the documented high-level client serialization and browser fetch/WASI DNS capability boundaries for Perry's facade. Extend backend capabilities if richer host HTTP/DNS is required.
4. Decide whether a future owned-head/rustls core change must remove the measured inherent allocations; preserve the new calibrated allocation gates in the meantime.
5. Integrator commits the coherent working tree and reviews the publish dry run/order. Nothing was uploaded or released by this lane.
