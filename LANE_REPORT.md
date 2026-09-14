# proto-http lane report

Status: implementation in progress. Authoritative draft 0.3 DESIGN.md sections 1–5b and 13 and local LANES.md read. Existing hyper fork removed. No changes to other lanes. Git commits are unavailable because .git is read-only.

## Surface and current status

| Surface | Implemented | Remaining verification / gaps |
|---|---|---|
| TLS client/server | Rustls unbuffered, caller-owned ciphertext, ALPN/SNI, injected wall time/deadlines, shared session caches, Mozilla/explicit/extra PEM roots, rejectUnauthorized, Node cause codes | Name/expiry error tests, cross-target runtime coverage |
| HTTP/1.1 | httparse heads, streaming borrowed bodies, CL/chunked/trailers, informational responses, HEAD/CONNECT/101 transitions, EOF framing, keep-alive, bounds, strict CL/TE rejection, reusable serializer | Interop and additional adversarial cases in progress; no pipelining API |
| HTTP/2 | Own frame codec, preface/SETTINGS, HPACK static/dynamic tables and Huffman, stream states, flow windows, PING/RST/GOAWAY, bounded CONTINUATION and headers | Interop and conformance in progress; push disabled; priority validated but not scheduled |
| Fetch/undici client | Origin+proxy pool reservations, h2 capacity, idle deadlines, redirect policy/method rewrites/credential stripping, abort/timeout lifecycle, host resolver trait, proxy route and CONNECT TLS request | Components not yet assembled into a single fetch dispatcher; body streaming replay fails explicitly |
| Compression | Bounded complete-body gzip/zlib/raw deflate/br/zstd decode | Streaming decompression and wasm zstd pending |
| WebSocket/ws | Tungstenite core over borrowed in-memory views, HTTP upgrade heads, nonce/key verification, subprotocol selection, message/ping/pong/close/fragmentation/limits from tungstenite | permessage-deflate declined; interop/close ordering verification in progress |

Perry references inspected: perry-ext-fetch shared client, proxy configuration, request/validation surface; perry-ext-axios method/body/response conversions; perry-ext-ws client/server/upgrade symbols; perry-stdlib fetch dispatch/transport_error. JS object/string/JSON/body representations and TypeError causes stay in the Perry adapter. Axios currently exposes verbs and response status/text/data/content-type; full npm Axios interceptors/configuration are outside the protocol layer.

## Verification (in progress)

- PASS: `cargo check --workspace`.
- PASS: `cargo test -p turnloop-tls`: 3 tests including real sockets, ALPN, payload comparison, session resumption, trust rejection/insecure option, exactly-once deadline.
- Initial FAIL: `cargo check --workspace --target wasm32-wasip2`: Apple clang has no wasm backend.
- PASS after selecting installed LLVM: `CC_wasm32_wasip2=/opt/homebrew/opt/llvm/bin/clang AR_wasm32_wasip2=/opt/homebrew/opt/llvm/bin/llvm-ar cargo check --workspace --target wasm32-wasip2`.
- Remaining tests and gates are UNRUN until recorded below.

## Allocation profile

- TLS wrapper adds no record staging or per-record allocation; rustls owns handshake/session state and required cryptographic storage. Encrypted input/output is supplied directly to the unbuffered states.
- HTTP/1 receive body events borrow input; host retains partial input. Heads/trailers own result strings/vectors. Serialization appends to caller-reused output. No per-body-chunk allocation.
- HTTP/2 DATA borrows input; stream slots and output/header assembly buffers are reused. HPACK dynamic table slots retain name/value capacity across eviction. Decoder allocates returned header representations; encoder table storage grows during warm-up. Huffman trie initializes once.
- Pool allocates for new origins/connections; reuse operations do not allocate. Redirect URL and result header/body representations allocate.
- Complete-body decompression currently constructs a decoder and a boxed memory reader per response; result buffer is reused. This does not yet meet the requested steady-state decompressor reuse goal.
- WebSocket uses tungstenite's reusable context buffers and owned Message payloads; no socket wrapper or per-event queue allocation added. Upstream frame/payload allocations need measurement before claiming the strict allocation target.

## Dependencies / supply chain

The existing 7-day publish-age configuration is unchanged. No override used. Cargo.lock is maintained for review.

Allowed direct dependencies: rustls (TLS engine), ring (portable crypto provider), webpki-roots (Mozilla trust), rustls-pemfile (host-supplied PEM), httparse (HTTP/1 syntax), http (validated types), bytes (payload representation), url (WHATWG-style URL parsing), flate2/brotli/zstd (content decoding), tungstenite default-features=false with handshake (WebSocket core), base64 (nonce encoding). rcgen is test-only. zstd is currently native-only to avoid a WASI libc/toolchain dependency; wasm support is an explicit gap.

ring is selected over aws-lc-rs because ring supports wasm32 with a wasm-capable C compiler and web getrandom support; its wasip2 build was checked. Neither rustls provider is pure Rust. No post-quantum default suite is promised. Entropy acquisition occurs inside upstream cryptography/masking; the crate does not read clocks or perform transport I/O. The host supplies wall time and all timer instants.

## Deviations / proposed design clarifications

- Document that C-backed crypto requires an LLVM/WASI cross compiler; target support cannot be inferred from native build success.
- Header result allocation is inherent, but decompressor/upstream WebSocket allocation requires further work before claiming full D3-style steady-state compliance.
- HTTP/2 conformance is not claimed before external tests; error scope (stream versus connection) needs focused review.
- Strict HTTP/1 parser rejects even equal repeated Content-Length and unsupported transfer codings instead of normalizing ambiguous input.
- Mozilla bundled roots + host-supplied extra/explicit roots are provided; OS-native root-store selection is not implemented.

## Sources

Protocol references: https://www.rfc-editor.org/rfc/rfc9112.html, https://www.rfc-editor.org/rfc/rfc9113.html, https://www.rfc-editor.org/rfc/rfc7541.html. Huffman numeric constants are generated from RFC 7541 Appendix B; codec logic is written in this lane, not copied from hyper/h2.
Behaviour references: https://fetch.spec.whatwg.org/, https://nodejs.org/api/tls.html, https://github.com/nodejs/undici/blob/main/docs/docs/api/Errors.md, https://github.com/websockets/ws/blob/master/doc/ws.md, https://axios-http.com/docs/req_config.

## Open questions / next steps

Complete interoperability and adversarial tests, external HPACK corpus and h2spec investigation, stable/clippy/target/dependency gates; then close correctness gaps and record remaining limitations accurately. A thin turnloop adapter waits for the executor wave as requested.
