# turnloop-websocket

Runtime-independent WebSocket state and HTTP upgrade helpers over tungstenite.
The host supplies transport bytes, upgrade nonce and deadlines, drains events, and
acknowledges completed writes. Shared HTTP types and buffers come from turnloop-http.

Supports masking, fragmentation, control frames, message/frame limits, subprotocols
and close handshakes. `permessage-deflate` is not negotiated. This is a protocol
building block, not a complete npm ws or JavaScript object-model replacement.

Native Node interop runs through the shared `scripts/test-servers.py` runner;
no Node package installation is required. Browser wasm uses host entropy for
upstream masking and the same sans-I/O protocol implementation.

## Getting started on turnloop

Enable the `turnloop` feature. `WebSocketStream::connect` performs and validates
an HTTP upgrade on a connected TCP, pipe or TLS stream; supply a fresh random
nonce, subprotocol list, executor handle and absolute deadline. `accept` performs
the server upgrade. Both preserve coalesced first-frame bytes. Await `send` and
`receive`; ping/pong and close replies flush before received messages are returned.
`close` waits for the peer under a deadline. Dropping a pending frame operation
closes the stream. The host keeps turning its LocalExecutor.

```sh
cargo run -p turnloop-websocket --features turnloop --example echo_server -- 127.0.0.1:8080
cargo run -p turnloop-websocket --features turnloop --example echo_client -- 127.0.0.1:8080
```

The same framed adapter works on native and WASI streams. Browsers use the web
backend's host WebSocket capability instead of raw HTTP upgrades; browser APIs
control TLS, headers, masking and extension negotiation. permessage-deflate remains
unsupported by the sans-I/O core. See `turnloop-io` for shared transport glue.

For browser-hosted payload I/O, adopt a host WebSocket directly (its handshake and
framing belong to the browser), then await the same shared stream helpers:

```rust,ignore
let socket = handle.driver().websocket(url, turnloop_io::turnloop::Token(0))?;
let mut payloads = handle.io(socket);
turnloop_io::write_all(&mut payloads, b"hello").await?;
let count = turnloop_io::read(&mut payloads, &mut buffer).await?;
```

Do not wrap that payload stream in `WebSocketStream`, which expects raw framed
wire bytes. Host payload I/O does not expose raw ping/pong frames, custom upgrade
headers, application-controlled masking, or a server listener. Wrap operations
in `turnloop_io::deadline` for abortable waits; the web executor contract tests
exercise this host path with real WebSocket traffic.
