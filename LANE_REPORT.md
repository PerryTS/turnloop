# adapters-net wave 3

In progress from 67690e9 (0.1.0-alpha.1). The integrator owns Git commits; this lane makes working-tree edits only.
Read DESIGN.md, CONTRIBUTING.md, integration report and relevant HTTP/core/Windows/WASM/CI and database lane reports. No applicable AGENTS.md.

## Implemented

- Publishable `crates/turnloop-io`: futures-io stream contract over AsyncIo/TCP/pipes, listener ownership, shared sans-I/O event/output driver, cancellation guards and executor-backed absolute deadlines. Added executor async connect, resolve, absolute timeout and browser fetch futures.
- Optional `turnloop` features on TLS, HTTP, WebSocket, Postgres, MySQL, Redis, SMTP and MongoDB. Database/mail adapters implement the one shared driver contract and preserve borrowed core events and terminal-close semantics.
- Generic async client/server `TlsStream<S>`: deadline handshake, ALPN, partial reads/writes and graceful close_notify. Native and WASI run the same rustls unbuffered cores.
- HTTP/1 and HTTP/2 streaming drivers, per-origin pooled client, replayable redirects, proxy CONNECT, incremental decompression, streaming uploads, total request deadlines and cancellation. `100-continue` handles informational replies, timeout fallback and early rejection. Server accept loop owns local tasks, stops accepts and drains active connections; HTTP/2 sends GOAWAY.
- Async WebSocket client/server handshake over HTTP upgrade, preserved unread bytes, frames, ping/pong and close handshake.
- HTTPS GET, TLS HTTP/1+HTTP/2 echo, WebSocket echo client/server examples. README/rustdoc getting-started sections and shared adapter contract. Integration report publish order puts turnloop-io after turnloop and before protocols.
- Required CI suites activate test target required-features; async protocol interop runs through existing required native/interop jobs. h2spec now tests the async server. WASI protocol jobs run real async HTTP/TLS socket tests. New browser fetch/abort contract is wired into existing web contracts. Examples are checked by all-targets/all-features Clippy.

## Verification

Every subsequent exact verification command, including failed attempts and fixes, is retained in [the command ledger](docs/adapters-net-commands.md). Logs are local under `.tools/adapters-net/`.

| Command | Result |
| --- | --- |
| `cargo check -p turnloop-io -p turnloop-tls --all-features` | PASS |
| `cargo test -p turnloop-io -p turnloop-tls --all-features --test streams --test asynchronous -- --test-threads=1` | PASS, 3 real TCP/TLS tests |
| `cargo check -p turnloop-http --features turnloop` | Initial FAIL borrow conflict; fixed, PASS |
| `cargo check --workspace --all-features` | PASS |
| `cargo test --workspace --all-features -- --test-threads=1` | PASS (latest subsequent HTTP additions tested separately; final rerun pending) |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS after fixing initial warnings; final rerun in progress |
| `python3 scripts/ci/h2spec.py` | PASS all 147 strict tests, zero skips, against async server |
| `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode all-features` | PASS; final added-test rerun pending |
| `cargo test -p turnloop-http --features turnloop --test asynchronous -- --test-threads=1` | PASS 8, 1 fixture test ignored here (executed by interop): pooled HTTP/1+2, 60 no-spin timer expiries, request/server cancellation, Node HTTPS/H2, curl, 100-continue |
| `cargo test -p turnloop-websocket --features turnloop --test asynchronous -- --test-threads=1` | PASS after fixing upgrade boundary and queued close delivery; real Node WebSocket and native async peers |
| `cargo test -p turnloop-io --test allocations -- --test-threads=1` | PASS 1,000 warmed TCP request/reply rounds with zero allocations; initial failure exposed consume_output(0), fixed in shared driver |
| `cargo test -p turnloop-websocket --features turnloop --test async_allocations` | PASS 100 HTTP exchanges, 300 inherent core-owned head allocations, zero added adapter allocations; 100 WebSocket reads and 100 writes with zero allocations |
| `python3 scripts/ci/install-wasm-toolchain.py` | PASS verified WASI SDK 34 |
| `bash scripts/ci/install-wasmtime.sh` | FAIL sandbox: inherited symlink pointed into another clone; no external writes made |
| `python3 scripts/ci/install-tools.py wasmtime --destination .tools/adapters-net/bin` | PASS verified Wasmtime 46; local .tools/bin symlink now points to this owned install |
| `source .tools/wasm-env.sh; python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | PASS all declared suites; 5 async HTTP and 2 async TLS tests use real wasi:sockets |
| `source .tools/wasm-env.sh; python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | PASS all declared suites under pinned nightly-2026-09-07 and Wasmtime 46; 5 async HTTP and 2 TLS tests |
| `cargo fmt --all` | PASS; final --check pending |

Cross-target Clippy, stable, final native modes, browser/Node runtime and supply-chain/automation checks are pending. Linux/Windows runtime and instruction-count comparisons are UNRUN (no hosts). SQL real-server runs are UNRUN (sandbox shmget/initialization restrictions supplied by user).

## Deviations / design questions

- Existing AsyncIo uses retained per-operation staging for safe cancellation. Adapters reuse transport storage and bounded protocol scratch; borrowed futures-io buffers never remain submitted after a dropped operation.
- Existing HTTP cores own decoded heads and allocate for them. Allocation gates measure and retain that baseline while asserting zero additional adapter allocations; WebSocket frame and shared TCP driver gates assert absolute zero.
- Upstream rustls 0.23.45 unbuffered decryption still owns/allocates plaintext records (already documented in CONTRIBUTING.md). TLS storage is retained by the adapter; no claim of whole-stack TLS allocation freedom.
- The pooled facade serializes requests on each Client; the low-level HTTP/2 driver supports multiplexed streams. Independent Client instances can dispatch concurrently.
- Existing browser fetch capability exposes only a bounded complete-body GET, without status/headers or request options. The browser facade exposes exactly that capability plus abort/deadline; streaming/custom methods, proxy CONNECT, explicit TLS/ALPN and listening servers are unavailable there. A richer web HTTP facade needs a backend capability extension.
- Database authentication and TLS-transition policy remain explicit core events managed by the host; their transport/event driving is shared.
- Windows runtime awaits the IOCP provider merge. No skip waiver or gate weakening added.
- No DESIGN.md changes or dependency-soak exceptions introduced.

## Next steps

Complete remaining interoperability/allocation checks, run final serial native/cross-target/WASI/web checks, update this report with every result and identify any environment-only UNRUN work for the integrator.

## Checkpoint 2 findings

- Native authenticated CONNECT → verified Node HTTPS succeeds. Curl HTTP/1, Node HTTPS/H2 and WebSocket interop pass. `100-continue` and response-close framing regressions are fixed and tested.
- Strict full-workspace Clippy PASS on Linux x86_64 (Zig cross C toolchain), WASI p2/p3 and browser wasm. Full-workspace stable native check PASS.
- Windows full all-targets Clippy FAIL: the base clone exports no `backend::Platform` for IOCP. This is a provider-merge prerequisite, not an adapter source error; generic-library-only cross-check pending. No mock provider, runtime waiver or skipped-success gate was introduced. Integrating IOCP is necessary before the complete Windows native job can pass.
- The core's resolver uses the native blocking pool; it returns Unsupported on WASI. A first attempt to treat WASI DNS like native DNS failed. Native cancellation/slot-reuse assertions remain intact, and a separate WASI test explicitly proves Unsupported and slot release. WASI clients use IP URLs or a host-resolved stream with the original TLS server name. Native DNS and generic adapter semantics are unchanged.
- Node web contracts PASS 12 tests, including new executor fetch/abort, with verified HTTP/WebSocket fixture traffic. wasm-pack's initial cache error was fixed by setting WASM_PACK_CACHE and XDG_CACHE_HOME inside the clone. Chrome browser runtime UNRUN (sandbox): matched ChromeDriver launched, but Chrome exited during session startup; zero browser tests ran. Firefox UNRUN (not installed).
- New TLS real-TCP allocation gate PASS: 100 warmed bidirectional exchanges, 400 existing rustls record allocations, zero adapter overhead. This gate also runs in required WASI/interop jobs. WebSocket and shared-I/O WASI suites added to metadata.
- no-tokio PASS across every policy target/default/all-feature graph. Soak PASS with the base branch's existing rustls security exception (unchanged); cargo-deny advisories/bans/licenses/sources PASS. No new dependency exception.
- Workflow lint PASS (actionlint/zizmor/shellcheck). Python gate tests PASS 91 after fixing an inherited feature-coverage mismatch: WASI/web features now require their actual target jobs, explicit forwarding/runtime commands and ci-gate dependencies, with negative controls. Every existing native matrix row and unknown-feature rejection remains mandatory.
