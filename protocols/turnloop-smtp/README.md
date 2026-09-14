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
Opportunistic. STARTTLS is followed by a fresh EHLO and capability parsing. AUTH
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
buffer defaults to 64 KiB. Message size is limited by server SIZE when supplied;
there is no streaming body API yet, so hosts must bound their MIME inputs.

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
