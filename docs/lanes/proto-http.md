# proto-http lane report — 2026-09-14

Implemented three usable sans-I/O crates under `protocols/`: `turnloop-tls`, `turnloop-http`, and `turnloop-websocket`. The previous hyper fork and hyper/h2/http-body dependencies are removed. This is a tested protocol implementation, **not a claim of complete Node/undici/axios/ws API parity**. Remaining differences are listed below.

Read: authoritative `/Users/amlug/projects/perry/windlass/DESIGN.md` draft 0.3, sections 1–5b and 13, and local `LANES.md`. No changes to the authoritative design, other lanes, or Perry bindings. No commits or publishing performed by this agent; the integrator owns commits.

## Implemented surface versus the behavioural targets

| Surface | Implemented and verified | Gaps / boundaries |
|---|---|---|
| TLS client/server | Rustls unbuffered states, caller-owned ciphertext, borrowed plaintext records, completion acknowledgement, ALPN, SNI on/off, shared session resumption, injected wall time and handshake deadlines | Provider entropy is supplied internally by ring. No wrapper socket, thread, clock read or timer. Native OS root-store loading, client certificates and comprehensive Node TLS error-text parity are not implemented |
| TLS trust / Node options | Bundled Mozilla roots; host-supplied extra PEM; explicit CA replaces defaults; `rejectUnauthorized: false` still verifies handshake signatures; expiry, not-yet-valid, hostname, unknown issuer and common protocol codes | Host reads `NODE_EXTRA_CA_CERTS` and other environment/files. Rustls cannot distinguish every OpenSSL issuer/self-signed failure; unknown issuer maps to `UNABLE_TO_VERIFY_LEAF_SIGNATURE` |
| HTTP/1.1 | Request/response heads; borrowed streaming bodies; CL; chunked encoding/decoding and trailers; informational responses; HEAD/204/304; EOF framing; connection reuse/close; CONNECT/101 upgrade boundaries preserve following bytes; strict line/head/count bounds; CL/TE conflicts, duplicate CL, obs-fold and bare-LF rejection | Pipelining is unavailable, hence off by default. Header count has a stack-backed hard ceiling of 256. Only `chunked` transfer coding is accepted; unsupported/multiple transfer codings fail explicitly. Some unusual valid syntax is rejected conservatively |
| HTTP/1 client driver | Reusable wire storage, non-pipelined request lifecycle, streamed uploads/downloads, `100-continue` gating and host deadline fallback, mid-body abort, one terminal result per accepted request | Adapter must close a failed/aborted HTTP/1 transport and preserve pending write buffer ownership. JS body objects and promise delivery remain in Perry |
| HTTP/2 | Own frame codec, preface and SETTINGS/ACK, client/server streams, HPACK static/dynamic tables, own Huffman codec, stream/connection flow windows including negative windows, explicit capacity release, DATA backpressure, lengths/bodyless responses, PING/RST/GOAWAY, settings deadline, connection-loss stream draining | Push disabled using SETTINGS. Priority is checked but not scheduled, as allowed. Prior-knowledge h2c supported; HTTP/1 h2c Upgrade not implemented. Error classification is conservative in several stream-error cases; not exhaustively proven RFC 9113 conformant |
| Header abuse protection | Compressed block, decoded list, frame, stream and CONTINUATION-count limits; interleaved header blocks rejected; invalid HPACK poisons the context | Exhaustive fuzzing and sustained adversarial CPU-budget testing remain unrun |
| Fetch redirects | Follow/manual/error; 301/302 POST and 303 method rewriting; 307/308 preserve; default limit constant 20; relative URLs; cross-origin credential stripping, including mixed-case names; non-replayable preserved bodies fail | Full Fetch URL validation, forbidden-header rules, redirect error cause text and all Node validation diagnostics are not yet a complete compatibility layer |
| Pool | Origin + proxy key; connecting reservations enforce per-host limits; HTTP/1 reuse, HTTP/2 capacities, host-supplied idle deadlines and close requests | Host composes Pool/Route/connection events. No automatic retry of streams above a peer GOAWAY last-stream ID, connection coalescing, or process-wide dispatcher/JS adapter |
| Proxy and DNS | Host resolver trait; HTTP_PROXY/HTTPS_PROXY/NO_PROXY values supplied by host; domain/port exclusions; HTTP absolute-form; CONNECT then explicit TLS request; Basic proxy credentials including percent decoding; proxy credentials excluded inside tunnels | HTTP proxies only; HTTPS-to-proxy, SOCKS, PAC, CIDR exclusions and every platform's environment precedence rule are not implemented |
| Compression | Complete-body convenience API and incremental gzip, zlib/raw deflate, brotli, zstd; explicit input/output progress; truncated stream errors; decoded size bounds; concatenated gzip/zstd; decoder reset and scratch reuse | Stacked content-coding chains are not automatically composed. Convenience `decode` constructs algorithm state each call; use cached `StreamingDecoder::reset` for steady state. Long corpus/fuzz coverage remains pending |
| WebSocket / ws | HTTP client/server upgrade helpers, host-supplied nonce, accept-key verification, subprotocol negotiation, tungstenite core over in-memory views, text/binary/control messages, masking, fragmentation, message/frame limits, close/deadline handling | **permessage-deflate is not negotiated** because tungstenite does not support it. Compression extension implementation remains a gap. Close/error text and all ws connection-option semantics are not complete npm parity |
| Perry ABI | No JS/GC/runtime types in these crates; result conversion belongs to Perry | Actual Perry binding replacement and the turnloop executor adapter are the requested later wave |

Perry sources inspected: `perry-ext-fetch` shared client/proxy/TLS environment and request/validation fields; `perry-ext-axios` verb/body/response conversions; `perry-ext-ws` client/server/upgrade APIs; `perry-stdlib/src/fetch` dispatch, validation and transport error mapping. Axios exposes verb requests and status/status-text/data/content-type; JSON/string/JS-value conversions stay in the binding. No claim is made that protocol helpers replace Axios interceptors or the whole WHATWG object model.

## Verification results

Final checks after implementation changes:

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo test --workspace` | PASS: **34 tests** (3 allocation, 16 codec/policy/corpus, 7 HTTP socket interop, 5 TLS, 3 WebSocket) |
| `cargo +stable test --workspace` | PASS: same 34 tests, stable **1.97.1** |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS on pinned nightly |
| `cargo check --workspace --target wasm32-wasip2` with LLVM environment below | PASS, all three crates |
| `cargo clippy --workspace --target wasm32-wasip2 -- -D warnings` with LLVM | PASS |
| `cargo check --workspace --target wasm32-unknown-unknown` with LLVM | PASS; browser entropy/PKI features enabled |
| `cargo clippy --workspace --target wasm32-unknown-unknown -- -D warnings` with LLVM | PASS |
| `cargo test -p turnloop-http --test codecs --test allocations --target wasm32-wasip2 -- --test-threads=1` with LLVM and Wasmtime runner | PASS: **19 executed WASI tests**, including actual zstd decoding and allocation checks |
| `python3 scripts/check-dependencies.py` | PASS for macOS, wasip2, web and all-target graph; **tokio/hyper inverse-tree stdout empty**; rejects the other forbidden runtime/body crates too |
| `cargo build -p turnloop-http --example h2spec_server` then `python3 scripts/run-h2spec.py` | PASS: **147 h2spec tests, 146 passed, 1 skipped, 0 failed** |

WASM tool environment (installed compiler, no Homebrew changes):

```sh
export CC_wasm32_wasip2=/opt/homebrew/opt/llvm/bin/clang
export AR_wasm32_wasip2=/opt/homebrew/opt/llvm/bin/llvm-ar
export CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang
export AR_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/llvm-ar
export CARGO_TARGET_WASM32_WASIP2_RUNNER='/opt/homebrew/bin/wasmtime run'
```

Additional verification detail:

- RFC 7541 request vectors with/without Huffman, integer vector, all 256 Huffman symbols, malformed integers/EOS/padding and dynamic table eviction. Vendored **72 complete hpack-test-case stories / 3,754 cases**, all decoded and compared in order. Sources: nghttp2 (all 32 stories), go-hpack and python-hpack (00–19). Fixture license, commit and original SHA-256s are in `protocols/turnloop-http/tests/hpack-corpus/`.
- Real private `std::net` sockets: HTTP/1.1 to Node, native server to curl and Node fetch, HTTP/2 both directions with **100 streams**, HTTPS with unbuffered TLS, proxy CONNECT followed by TLS and HTTP, redirects/gzip/trailers/socket reuse, and abort after receiving body bytes with server-side EOF assertion.
- TLS tests assert payloads, ALPN, full then resumed handshake, trusted CA/leaf chain, server-observed SNI presence/absence, insecure option, expiry/name/not-yet-valid codes and one-shot timeout.
- Node **26.5.1** and curl **8.7.1** with HTTP2 enabled were exercised. The offline `ws` package was unavailable; Node 26's built-in WebSocket performed subprotocol negotiation, message exchange and clean close with the native server.
- Ten fresh Node/curl HTTP/2 interop rounds passed after fixing shutdown: the test server now sends GOAWAY, half-closes writes, and drains the peer instead of risking a TCP reset from unread control frames.
- h2spec's skipped test is `http2/6.9.2`, “Sends a SETTINGS frame for window size to be negative”, skipped by its short-response precondition. A separate crate test explicitly makes a window negative, asserts a stall, adds credit, and asserts the exact resumed byte count. The skip is **not counted as a pass**.
- `cargo tree -i tokio` / `cargo tree -i hyper` return Cargo's expected “did not match any packages” diagnostic on stderr (exit 101), with empty stdout. The audit checks this exact condition rather than hiding arbitrary Cargo failures. `all` also inspects dependencies of targets not executed here.
- All private test servers were stopped by scoped process guards, joined threads, or the conformance runner's `finally` cleanup. No persistent server PID/state file remains. System/default-port servers were never used. `scripts/http-server.sh start h1|h2` and `stop` were also exercised successfully; stop authenticates to the private instance and confirms its listener closed, without process scanning or killing unrelated PIDs.

Final detailed logs are under `.tools/final-*.log`; h2spec output/JUnit are `.tools/h2spec.log` and `.tools/h2spec.xml`. These are local artifacts, not committed fixtures.

### Failures encountered and resolved (tests were not weakened)

- Initial `cargo check --target wasm32-wasip2` FAILED: Apple clang had no wasm backend. Selecting the installed LLVM compiler fixed it.
- Initial browser check FAILED: rustls-pki-types lacked its `web` feature, so rustls's compiled-in time helpers could not resolve `UnixTime::now`. Enabling PKI `web` fixed compilation; the wrapper continues to use only its supplied TimeProvider.
- Initial socket interop FAILED because accepted macOS sockets inherited nonblocking mode; the blocking harness explicitly resets it. An intermittent HTTP/2 close timeout was then resolved with graceful shutdown and passed ten repetitions.
- Initial h2spec run: 142 passed, 1 skipped, 4 FAILED. Stream-window overflow now emits RST_STREAM/FLOW_CONTROL_ERROR; the driver retains body data during stalls. Final result above is green.
- An HPACK negative fixture accidentally encoded the allowed 4096-byte table limit. It was corrected to 4097, and an explicit positive assertion for 4096 was added.
- WASI allocation test FAILED: **600 allocations in 100 zstd bodies**. Two upstream allocation sites were patched; the identical zero-allocation assertion now passes.
- Intermediate compile/clippy failures (type inference, target-only enum size and a temporarily misclassified vendored workspace member) were fixed. The vendor is an independent workspace dependency through a root patch; upstream tests were not deleted.

UNRUN: Windows/Linux/BSD/mobile execution, browser runtime execution, Miri/fuzz/long soak, production load/CPU benchmarks, exhaustive Node error-message/option parity, and upstream ruzstd's full repository corpus (not included in its published crate). Example future commands include `cargo test --workspace --target x86_64-pc-windows-msvc`, `cargo test --workspace --target x86_64-unknown-linux-gnu`, and a browser wasm-bindgen harness. Build success is not presented as runtime coverage.

## Allocation profile

Measured with a scoped counting global allocator, with workload counters and byte assertions:

- **Zero** post-warm-up allocations for repeated HTTP/1 serialization, borrowed HTTP/1 body events, indexed HPACK encoding, and HTTP/2 DATA/flow-control exchange.
- **Zero** post-warm-up allocations for reused gzip, deflate, br and zstd decoders on native **and WASI**. Brotli receives a reusable scratch allocator; the same decoder's `reset` retains storage. Warm-up must cover required buffer/table sizes; larger/new shapes can grow retained storage.
- HTTP heads, trailers and decoded header results allocate their inherent owned representation. HPACK dynamic entries reuse slots and capacity. Huffman trie initializes once. Stream slots and compressed-header/output buffers are reused.
- TLS wrapper adds no record staging copies/allocations; rustls owns cryptographic, handshake and session storage. TLS record allocation counts were not independently instrumented.
- WebSocket adds no per-command I/O-object or event queue allocation. Tungstenite owns message payload/frame storage, and masking may require mutable payload storage. Exact upstream allocation counts beyond the result representation remain uninstrumented.
- Pool reuse/reservation bookkeeping uses retained slots. New origins/connections/redirect URLs and returned head representations allocate. `compression::decode` intentionally constructs state per call; the documented steady-state API is cached `StreamingDecoder` plus `reset`.

## Dependencies and supply chain

The 7-day Cargo publish-age configuration is unchanged; **no soak override was used**. Cargo.lock is current. No tokio, hyper, h2, http-body, futures executor, or async runtime dependency remains.

| Dependency | Justification |
|---|---|
| rustls + ring | Unbuffered TLS and explicit portable crypto provider; default rustls provider disabled |
| webpki-roots, rustls-pemfile | Mozilla trust anchors and host-supplied PEM |
| rustls-pki-types `web` (browser target) | Makes upstream rustls std helpers compile on web; wrapper time remains injected |
| httparse, http | HTTP/1 syntax and HTTP types/status validation; hot-path field validation does not construct disposable owned types |
| bytes | Runtime-independent message/payload representation |
| url, percent-encoding, base64 | URL/origin handling and proxy/WebSocket credential/nonce encoding |
| flate2, brotli | In-memory gzip/deflate/br codecs; brotli scratch storage recycled |
| zstd (native) | Reusable in-memory reference zstd decoder |
| **ruzstd 0.8.3 (WASM, additional dependency)** | Small established pure-Rust zstd decoder avoiding WASI libc/C toolchain requirements for compression; transitive twox-hash validates zstd checksums |
| tungstenite, default features off + handshake | Runtime-free WebSocket protocol core |
| getrandom `wasm_js` (browser target) | Entropy backend needed by tungstenite masking; ring's browser entropy feature also explicitly enabled |
| rcgen (dev only) | Generated CA/leaf and invalid-validity certificates |

The browser graph includes js-sys/wasm-bindgen/web-time and futures-core/task/util through those libraries; these are bindings/traits/combinators, **not an async executor or runtime**. The protocol crates do not run futures.

**Provider decision:** ring 0.17.14 is selected on every target. It was built for macOS arm64, wasip2 and web; WASI executes the HTTP/compression tests, while TLS handshake execution has only been verified natively. It needs a wasm-capable C compiler. aws-lc-rs was not selected or benchmarked; enabling rustls defaults would introduce its additional provider/build surface. No claim is made that aws-lc-rs cannot support a given target, and no PQ/FIPS capability is promised. Native Windows/Linux/mobile ring execution remains CI work.

**Small vendored allocation patch:** `protocols/turnloop-http/vendor/ruzstd/TURNLOOP_PATCH.md` records the two implementation changes and original verified `.crate` SHA-256:
`a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8`.
The root `[patch.crates-io]` must be retained by consuming workspaces until upstream ships the fixes. The upstream library, license and tests are preserved; this is not an HTTP/HPACK implementation dependency. HPACK/Huffman logic is our own; only RFC numeric constants and test vectors were sourced externally.

**h2spec tooling:** the v2.6.0 release had neither an asset digest nor published checksum. Instead, source commit `70ac2294010887f48b18e2d64f5cccd48421fad1` was downloaded and every archive file checked against the commit's Git blob IDs, then built locally. macOS required external linking and an ad-hoc signature. Executed binary SHA-256: `5eaa3aefa916971ba3f0011c761efb49eb62623774fcb55bcb30f9397e5417ff`. Reproduce with `python3 scripts/setup-h2spec.py`, then build/run the conformance driver as above. No unverified release binary was executed.

## Design deviations, open questions and next steps

No authoritative DESIGN.md edits proposed for the zero-runtime architecture. Clarifications worth recording:

1. Provider entropy acquisition is an upstream cryptographic requirement; all transport and wall/monotonic time access still belongs to the host. The unbuffered API and buffer lifetime contract map directly to completion I/O.
2. Header/message result representation allocations and algorithm buffer growth should be distinguished from avoidable per-command scratch allocations. This lane measures the latter instead of asserting an unmeasured global zero-allocation claim.
3. Downstream integration must preserve the temporary ruzstd patch, or its measured WASM allocation guarantee regresses. Prefer upstreaming the two small changes before publishing these crates; no network publication was performed here.

Next work: thin turnloop adapter and Perry FFI wiring once the executor lands; close the table's behavioural gaps (especially permessage-deflate and detailed Node error/option parity); add cross-platform runtime CI, fuzzing/soaks and allocation measurements for TLS/WebSocket. Exact production readiness and full Node parity remain open rather than inferred from passing interop.

## Reference material

- [RFC 9112 — HTTP/1.1](https://www.rfc-editor.org/rfc/rfc9112.html)
- [RFC 9113 — HTTP/2](https://www.rfc-editor.org/rfc/rfc9113.html)
- [RFC 7541 — HPACK](https://www.rfc-editor.org/rfc/rfc7541.html)
- [Fetch standard](https://fetch.spec.whatwg.org/)
- [Node TLS documentation](https://nodejs.org/api/tls.html)
- [Undici errors](https://github.com/nodejs/undici/blob/main/docs/docs/api/Errors.md)
- [ws API](https://github.com/websockets/ws/blob/master/doc/ws.md)
- [Axios request configuration](https://axios-http.com/docs/req_config)

## Integrator verification

- 2026-09-14, integrator, outside the Codex sandbox (macOS arm64): `cargo test --workspace --locked` — **PASS, 34 tests, 0 failed**; `python3 scripts/run-h2spec.py` (strict) — **147 tests, 146 passed, 1 skipped, 0 failed**; `cargo tree -i` for tokio, tokio-util, hyper and h2 — none present.
- **Publishing blocker to resolve at integration:** the root `[patch.crates-io] ruzstd = { path = "protocols/turnloop-http/vendor/ruzstd" }` only applies inside this workspace. Crates.io consumers of `turnloop-http` would get upstream ruzstd without the patch. Upstream the fix, publish the fork under its own name, or drop the dependency before publishing `turnloop-http`.
