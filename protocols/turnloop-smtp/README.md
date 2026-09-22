# turnloop-smtp

Sans-I/O SMTP transport plus lettre's builder with all transport features off.

The host connects the socket, calls `connected(now)` and drains events. Complete
`UpgradeTls` externally with rustls (implicit TLS or STARTTLS), then call
`tls_established(now)`. Write `output()` and acknowledge actual bytes written with
`consume_output(n)`. Feed plaintext from the socket/TLS layer to `receive(bytes,
now)`. Drive `next_timeout()` / `handle_timeout(now)` using the host timer. Never
feed TLS ciphertext to the SMTP parser. Connection/DNS deadlines and certificate
policy are host responsibilities; greeting/socket deadlines are exposed here.

`Ready` means greeting, EHLO/HELO, TLS and configured AUTH succeeded, and implements
nodemailer's verify transport check. `send(token, envelope, message_id, mime,
now)` takes ownership of the envelope/ID, copies MIME into reusable DATA storage,
and reports one Sent/Failed for the accepted token. A synchronous error accepts
nothing. `Sent.info` includes response/code, accepted addresses, per-recipient
rejections, envelope and Message-ID. The host exposes `rejected` as addresses and
`rejectedErrors` from each rejection record. Failed events retain envelope and
recipient outcomes too. RSET drains a failed transaction before reuse; wait for
state Ready after an automatic reset. Manual `reset` emits Reset; `quit` and
`close` emit CloseTransport then Closed. Sent indicates SMTP acceptance, not
final mailbox delivery; the core never retries a message automatically.

TLS options correspond to secure/requireTLS/ignoreTLS: Implicit for secure=true
(or host's port-465 default), Required, None for ignoreTLS, otherwise
Opportunistic. Required never reaches `Ready` in the clear: if the server does not
offer STARTTLS or refuses it, the session fails with code `ETLS` before any AUTH is
sent, so hosts need no `Ready` guard of their own. STARTTLS is followed by a fresh
EHLO and capability parsing. AUTH
PLAIN, LOGIN and XOAUTH2 are selectable; OAuth token acquisition/refresh is host
policy. No CRAM-MD5. PIPELINING sends MAIL and all RCPT commands together, drains
all responses and sends DATA only with an accepted recipient. SIZE uses
normalized content octets before dot-stuffing; 8BITMIME and SMTPUTF8 are checked
before any envelope bytes are emitted. Non-ASCII envelope addresses require
SMTPUTF8. MIME headers should be RFC-encoded by the message builder.

`message::build(Mail)` supports text/html alternatives, attachments, cc/bcc,
custom headers and lettre's transfer encodings. The caller supplies Date,
Message-ID and a boundary seed, avoiding lettre's implicit clock/random calls.
For full builder customization lettre types are re-exported; callers using them
directly must supply Date/IDs/boundaries themselves. Header and envelope injection
are rejected. MIME building materializes owned representations; this is separate
from the transport hot path. The transport reuses TX/RX, response, SASL and DATA
buffers; warmed success allocates only the returned accepted-vector, recipient
string and response string (verified by counting allocator). Maximum response
buffer defaults to 64 KiB. Message size is limited by server SIZE when supplied.

`send` is the one-shot convenience: it copies the whole message into DATA storage.
For large messages, `start_send(token, envelope, message_id, StreamBody { size,
eight_bit }, now)` runs the same MAIL/RCPT/DATA exchange, then emits `BodyReady`
once the server answers 354. Pass the content in any number of `send_chunk(bytes,
now)` calls and end it with `finish_body(now)`; the single `Sent`/`Failed` follows
as for `send`. Draining `output()` between chunks keeps memory bounded by one
chunk. `DataEncoder` normalizes line endings and dot-stuffs incrementally, so a
CRLF pair or a CRLF.CRLF split across chunks encodes exactly as in one piece.
`size` is the declared SIZE parameter (omitted when `None`) and `eight_bit`
declares BODY=8BITMIME up front, since both are sent before any content. Content
cannot be retracted mid-DATA, so undeclared 8-bit bytes or a server reply while
the body is open fail the message and close the transport instead of sending a
terminator.

Error fields follow nodemailer response/responseCode/command/code conventions.
AUTH failures include mechanism and server response in the message. Full exact
Node message parity for every unusual SMTP response, plugin transports, DKIM,
DSN, LMTP, pooling, proxy/service presets and OAuth refresh are outside this core.
Perry's old generic lettre error prefixes are not preserved: its later adapter
must select the desired JS compatibility layer.

`cargo test -p turnloop-smtp` runs scripted blocking TCP and rustls tests.
`cargo test -p turnloop-smtp --test smtp installed_postfix -- --ignored` additionally
runs an installed `/usr/libexec/postfix/smtp-sink` on a private loopback port and
asserts the saved message bytes. It starts/stops its own child and writes only
under workspace `.tools/`. Test certificate keys are public fixture data.

## Getting started on turnloop

Enable `turnloop-smtp = { version = "0.1.0-alpha.2", features = ["turnloop"] }`
and use the `asynchronous` module: `Transport::connect and send`. The default feature set remains sans-I/O.
The adapter reuses `turnloop-io`; the host owns `LocalExecutor` and calls `turn`.
Spawn local futures through its handle and keep their `JoinHandle`s until completion.

```sh
cargo run -p turnloop-smtp --features turnloop --example turnloop
```

[The complete example](examples/turnloop.rs) connects to `127.0.0.1:2525` by default;
`TURNLOOP_DB_ADDR` overrides that development endpoint. All operations take an
absolute `turnloop_io::Instant` deadline, shared across authentication, I/O and retries.
Callbacks borrow row/reply storage; copy values only when retaining them.
Dropping a pending operation closes its transport before a pool can reuse it.

For TLS, set the protocol's TLS mode and provide
`turnloop_tls::asynchronous::ClientTls` with the trust configuration, verified
server name and current Unix seconds. Certificates are verified; a requested TLS
upgrade without a configuration fails. Native TCP and WASI 0.2 sockets share the
same driver. Browser raw TCP is unavailable; Windows accepts a host-provided
`Backend` until the repository's production IOCP provider is integrated.

`send` returns accepted/rejected recipient details from the server, including
partial success; errors preserve the full envelope in `SendFailure`. Authentication,
STARTTLS/implicit TLS and advertised PIPELINING are handled by the existing core.
The allocation gate counts only the three existing owned-result allocations for
a one-recipient delivery; the async transport adds zero allocations after warm-up.


## Getting started on turnloop

Enable the `turnloop` feature and construct `asynchronous::AsyncConnection::new`
with a connected `turnloop::AsyncIo` (TCP/pipe) or `turnloop_tls::TlsStream` and
this crate's sans-I/O `Connection`. Submit commands through `core_mut()`, then
`next(&executor_handle, |event| { /* consume the event */ Ok(()) }).await`.
Callbacks finish consuming borrowed rows before the next event. Authentication,
entropy and TLS-upgrade requests are explicit events; acknowledge them through
the core. For an upgrade, `into_parts` preserves the stream, core and unread bytes.
Apply `turnloop_io::deadline` using the core's next deadline. Dropping a pending
drive future closes the stream and aborts the core; terminal core events remain
available for draining. See `turnloop-io` for the shared adapter ownership pattern.
