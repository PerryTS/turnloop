# turnloop-http

Own HTTP/1.1 and HTTP/2 wire engines, with no runtime or transport dependency.

- `http1::Decoder::receive` consumes a prefix and returns one event. Retain the unconsumed bytes, including bytes after a CONNECT/WebSocket upgrade. A zero-consumption event is progress; call again until both consumption and event are empty. `eof` distinguishes close-delimited completion from truncated framing.
- `client::Http1Connection` adds non-pipelined requests, streamed uploads, `100-continue`, output acknowledgements, deadline/abort state, and one terminal completion per accepted request. Take the completion before reusing the connection.
- `http2::Connection` handles both roles. Flush and acknowledge `output`, retain incomplete input, consume one frame/event at a time. `send_data` can accept only a prefix or zero when flow-controlled; retain the remainder. Return DATA capacity with `release_capacity` after the application consumes it. Queue application responses in the host during stalls. On transport loss call `eof`, then drain `poll_failed_stream`.
- `hpack` provides the independent bounded RFC 7541 codec. The encoder never indexes credentials/cookies. Decode failures poison a context.
- `client::Pool` reserves connections before DNS/connect so parallel commands respect per-origin/proxy limits. `Route` emits resolution/TLS requests and builds proxy CONNECT or absolute-form heads. `Resolver` belongs to the host. `Request::redirect` applies Fetch redirects; use `DEFAULT_MAX_REDIRECTS` (20). JS conversions and promise delivery remain with Perry.
- `compression::StreamingDecoder` accepts input and caller-owned output. Reuse it with `reset` to retain scratch buffers across bodies. Hosts may cache one per content encoding. `decode` is the convenience whole-body path and constructs algorithm state each time.

No engine samples a clock. The host passes `Instant` deadlines and invokes timeout handlers. Hold output storage stable until a completion-shaped write finishes: do not mutate the engine while an I/O operation borrows its output. Error codes are transport causes; Perry creates the JS error objects and detailed OS diagnostics.

The WASM zstd implementation uses the root workspace patch for ruzstd. Preserve that patch in a consuming workspace until the two upstream allocation fixes ship. See `vendor/ruzstd/TURNLOOP_PATCH.md`.

Tests use private ephemeral loopback servers, Node 26, curl, generated TLS certificates, RFC vectors and a vendored HPACK corpus. `examples/h2spec_server.rs` is a blocking conformance driver, not the future turnloop adapter. Exact commands and limitations are in the root `LANE_REPORT.md`.
