# turnloop-tls

Unbuffered rustls TLS for a host-driven event loop. The host owns transport,
ciphertext buffers, wall time and handshake deadlines. Handle each returned state,
consume borrowed plaintext before advancing, and acknowledge encrypted writes only
after their transport completion. No socket, scheduler or runtime is created.

By default client/server configurations use the ring provider (rustls defaults
are disabled), including native Windows and wasm32. Browser entropy and PKI web
features are enabled explicitly; building ring for wasm needs a wasm-capable C
compiler. WASI obtains entropy through its host. No FIPS/PQ guarantee is claimed.

The provider is a host choice: `ClientOptions::provider` and
`ServerConfig::with_provider` take any rustls `CryptoProvider` (aws-lc-rs, for
example). `ring` is a default feature; with `default-features = false` the crate
links no provider of its own, and `new` uses the provider the host passes or the
rustls process default. Without `ring`, `tls_server_end_point` is unavailable and
`tls_server_end_point_hash` names the hash for the host to compute.

Mozilla roots, host-supplied extra PEM, explicit replacement CAs, SNI and ALPN are
supported. See public API docs and `tests/tls.rs` for checked handshake examples.
Anything else rustls can configure (client certificates, SNI resolvers, custom
verifiers, ticketers) goes through `ClientConfig::from_rustls` /
`ServerConfig::from_rustls`: build the rustls config with
`builder_with_details(provider, host_time.time_provider())` so certificate checks
keep following the wall time passed to `process`.
PEM parsing uses rustls-pki-types through rustls’s maintained re-export.

## Getting started on turnloop

Enable `turnloop-tls/turnloop`. `TlsStream::connect` and `TlsStream::accept` take
an owned stream, shared TLS configuration, executor handle, absolute handshake
deadline and host wall time. Await the handshake, inspect `alpn_protocol()`, then
use `turnloop_io::{read, write_all, close}`. These helpers work on TCP and pipes
and compose with HTTP/WebSocket adapters. `close` flushes close_notify; dropping
aborts the transport. `turnloop_io::shutdown` flushes close_notify and half-closes
the transport while decryption continues until the peer's close_notify or EOF, which
is what a lingering server close uses. Refresh host wall time with `set_unix_seconds` when needed.
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
