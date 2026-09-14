# turnloop-mysql

A sans-I/O MySQL v10 client, built on `mysql_common` handshake/auth/value packets.
No sockets, filesystem access, threads, implicit clocks, timers or entropy reads
in production. The crate forbids unsafe. TLS and entropy are explicit requests.

## Host contract

Construct Config and Connection, connect transport in the host, then feed
plaintext into `receive`. Pull `next_event` until None; `Progress` means a control
packet was consumed and polling should continue. Write `output`, acknowledging
only successfully written bytes via `consume_output`. Borrowed outputs/events
remain valid until the next mutable call. Preserve the output borrow through
write completion or copy into a reusable host transport buffer.

At `UpgradeTls`, flush the SSLRequest, verify and finish TLS in the host, and call
`tls_established`. For caching_sha2 full authentication on plaintext transport,
the core requests the server key. `RsaSeedNeeded` asks the host for **20 fresh
cryptographically random bytes**; `rsa_seed` performs nonce XOR and OAEP-SHA1
using those bytes. Never reuse a seed. Fast caching auth, TLS password auth,
mysql_native_password, and auth switches are handled explicitly. There is no
arbitrary authentication-plugin callback or multi-factor flow.

Provide absolute deadlines; schedule `next_timeout` and call
`handle_timeout(now)`. The host supplies mysql2's default 10-second connect
deadline; Config does not read a clock. On EOF/TLS failure or a parsing error,
call `abort(error)` and drain events. A parsing error is terminal; never continue
the byte stream after it. Each accepted command yields exactly one Completed;
server Error and result Ok events are informational. Close emits one Closed.
Command rejection accepts no token. There are no callbacks from this core.

MySQL permits one active command. Busy calls return backpressure; the adapter
queues commands in JS submission order. This prevents unsynchronized packet
sequence resets. COM_QUERY supports multiple result sets (multiple statements
are opt-in); EOF negotiation deliberately selects legacy EOF, which MySQL 9.6
supports. Result Ok includes affected_rows, last_insert_id, warnings and status.

Prepare emits parameter/column metadata and a Statement ID. Execute accepts
mysql_common Value parameters and emits binary rows. Reset/close statements,
ping, quit, reset_connection and change_user are commands. Successful reset and
changeUser invalidate statement IDs. COM_STMT_CLOSE completes after its bytes
are acknowledged because the server sends no response. There is no SQL-keyed
LRU/automatic prepare helper or COM_STMT_SEND_LONG_DATA API yet; large parameters
use packet continuation within the configured buffer bound. Transactions use
START TRANSACTION/COMMIT/ROLLBACK; status flags track the server state.

LOCAL INFILE is disabled by default and aborts an unsolicited request without
reading a file or returning file data. When enabled, LocalInfile lends the
requested filename to the host. The host decides whether/how to source bytes,
then calls local_infile_data and local_infile_finish; the final empty packet
terminates input. To reject an enabled request safely, abort the connection.

Compression negotiates CLIENT_COMPRESS. Plain and compressed framing handle
24-bit lengths, sequence wrapping, continuation and exact-boundary empty frames.
The outer zlib encoder/decoder states are retained and reset; upstream's
per-frame zlib construction did not satisfy this lane's allocation contract.
Zstd and MariaDB extensions are out of scope.

## mysql2 conversions and hooks

Row iterators return borrowed bytes or mysql_common numeric/calendar scalars.
`types::decode` implements a default conversion policy. `Column` retains names,
original names/table/schema, flags, charset, length, decimals and wire type.

| MySQL type | Default policy / host action |
|---|---|
| integer, FLOAT, DOUBLE | Number; TINYINT(1) remains numeric |
| BIGINT | Number by default; support_big_numbers preserves unsafe-range values as strings; big_number_strings forces strings when enabled |
| DECIMAL/NEWDECIMAL | exact string; decimal_numbers opts into f64 |
| DATE/DATETIME/TIMESTAMP | explicit Date request with raw text/calendar components; date_strings formats strings |
| TIME | string, including negative and >24-hour values |
| JSON | explicit host JSON parse request; json_strings returns raw text |
| BLOB/binary charset 63, BIT, geometry | Buffer bytes |
| text | UTF-8 string; other character sets require host decoding |
| NULL | Null |

The host selects timezone, creates JS Date (including zero-date/invalid-Date
behavior), parses JSON and materializes row objects or rowsAsArray tuples.
The protocol preserves microseconds; JS Date loses sub-millisecond precision.
`typeCast` can inspect Column and RawValue in the event-dispatch layer and invoke
`types::decode` for next(). Callback invocation, field.string/buffer single-use
semantics and field.geometry parsing are adapter work, not core callbacks.
Per-type dateStrings arrays are not implemented (only the boolean option).
BIGINT inside JSON is still host policy. This is documented surface support,
not a drop-in mysql2 API.

ServerError preserves errno, sql_state and sql_message. `code()` maps Perry's
common observed server errors and several auth/connection codes; unknown numbers
return None, preserving errno. Full mysql2 error-name registry parity is a gap.
URI parsing, named placeholders, query formatting/escaping, endpoint/socket
options, custom charsets, promise/callback order and TLS options belong to the
adapter. `.query` does not interpolate parameters; use prepare/execute for the
Perry bound parameter path. Existing Perry DECIMAL conversion is lossy and is
not reproduced by the default policy.

## Pool, memory and verification

The pool is lazy and bounded, queues waiters FIFO and reuses idle clients LIFO.
It implements waitForConnections, queueLimit, maxIdle, idleTimeout, release,
connection failures, generation-checked leases and end. Connect/Close are host
requests; report completion back before a slot can be reused. Event emitters,
nextTick/promise scheduling, auto-release query wrappers and graceful transport
close are adapter work. PG-style min/uses/lifetime limits are optional extras.

Packet/TX/RX/metadata/serialization buffers are reused. Numeric and borrowed-byte
rows and warmed query/ping commands allocate nothing. Tests count 1,000 warmed
plain and compressed commands; row tests exercise a 512-byte value generated by
the independent mysql_common compressed encoder. Authentication, RSA arithmetic,
new statement registration, pool capacity growth and owned result conversion
allocate. Compression state is allocated once at negotiation and retained.

Run `cargo test -p turnloop-mysql`; real server tests are ignored by default and
assert required fixture variables if selected. From the workspace:

```sh
TURNLOOP_SQL_SERVER=mysql python3 scripts/test-servers.py run cargo test -p turnloop-mysql --test server -- --ignored
```

The private fixture script starts/stops only its own instances and keeps files
under `.tools`. MySQL 9.6 cannot authenticate mysql_native_password users: that
plugin was removed in 9.0, so a separate private 8.4 fixture is needed for that
real-server case. See the root report for actual verification and open gaps.

Browser builds export a host-supplied `Instant::from_duration(ticks)`; native and
WASI builds reexport std::time::Instant. Use one monotonic epoch per connection
and pool. Map the browser core loop’s Instant through as_duration/from_duration.
The protocol never calls performance.now or std Instant::now. Browser runtime
execution is not yet verified; only target compilation has run.

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
