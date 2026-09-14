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
