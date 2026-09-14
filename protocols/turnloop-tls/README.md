# turnloop-tls

Unbuffered rustls TLS for a host-driven event loop. The host owns transport,
ciphertext buffers, wall time and handshake deadlines. Handle each returned state,
consume borrowed plaintext before advancing, and acknowledge encrypted writes only
after their transport completion. No socket, scheduler or runtime is created.

Client/server configurations share the workspace’s ring provider (rustls defaults
are disabled), including native Windows and wasm32. Browser entropy and PKI web
features are enabled explicitly; building ring for wasm needs a wasm-capable C
compiler. WASI obtains entropy through its host. No FIPS/PQ guarantee is claimed.

Mozilla roots, host-supplied extra PEM, explicit replacement CAs, SNI and ALPN are
supported. See public API docs and `tests/tls.rs` for checked handshake examples.
PEM parsing uses rustls-pki-types through rustls’s maintained re-export.

## Getting started on turnloop

Enable `turnloop-tls/turnloop`. `TlsStream::connect` and `TlsStream::accept` take
an owned stream, shared TLS configuration, executor handle, absolute handshake
deadline and host wall time. Await the handshake, inspect `alpn_protocol()`, then
use `turnloop_io::{read, write_all, close}`. These helpers work on TCP and pipes
and compose with HTTP/WebSocket adapters. `close` flushes close_notify; dropping
aborts the transport. Refresh host wall time with `set_unix_seconds` when needed.
The host continues calling `LocalExecutor::turn` throughout the connection.

After a successful handshake, `peer_certificates()` borrows the peer chain,
leaf first, without copying. `tls_server_end_point(leaf.as_ref())` derives the
RFC 5929 certificate binding with ring and no heap allocation. It reads the outer
certificate signature algorithm, supports RSA/ECDSA SHA-256/384/512 and RSA-PSS,
and maps MD5/SHA-1 to SHA-256. Unsupported algorithms such as Ed25519 return
`None`. This helper does not verify certificates: verification follows the TLS
configuration, including any explicitly insecure host option.

The adapter retains ciphertext/plaintext storage and uses rustls's unbuffered
state machine. rustls 0.23's plaintext record representation still allocates;
transport scratch allocation freedom is distinct from upstream crypto storage.
The real-socket allocation gate checks 100 bidirectional records: 400 existing
rustls allocations and zero adapter overhead. Unread plaintext is bounded to 64 KiB.
WASI sockets use the same implementation. Browser TLS is managed by host fetch
or WebSocket and cannot expose raw TLS options.
