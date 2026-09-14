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
