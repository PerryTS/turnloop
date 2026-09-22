# turnloop-postgres

A pull-driven PostgreSQL v3 client core, built on `postgres-protocol` frontend
messages, framing and SCRAM. It never opens a socket, starts a thread, reads a
clock, schedules a timer or generates entropy. The sans-I/O code forbids unsafe.

## Getting started on turnloop

Enable `turnloop-postgres = { version = "0.1.0-alpha.2", features = ["turnloop"] }`
and use the `asynchronous` module: `Client::connect, query, execute, copy_in/copy_out and notification`. The default feature set remains sans-I/O.
The adapter reuses `turnloop-io`; the host owns `LocalExecutor` and calls `turn`.
Spawn local futures through its handle and keep their `JoinHandle`s until completion.

```sh
cargo run -p turnloop-postgres --features turnloop --example turnloop
```

[The complete example](examples/turnloop.rs) connects to `127.0.0.1:5432` by default;
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

`Pool::new` accepts the existing `pool::Config` (max, max_idle, idle/acquire
limits, max_uses); `acquire(deadline)` returns an exclusive lease. A transaction
still open at release destroys the connection. Named `ExtendedQuery` values use
the core prepared-statement cache. `notification(deadline)` is the async next
operation after `LISTEN`; query callbacks also receive interleaved notifications.
`copy_in` lends a chunk writer with `finish`; dropping it closes the session.
`copy_out` lends each chunk to a callback without buffering the complete transfer.
`cancel_token().cancel(...)` sends CancelRequest over a separate connection.
TLS authentication prefers SCRAM-SHA-256-PLUS. The client derives RFC 5929
`tls-server-end-point` data from the verified peer leaf; an explicit
`ConnectOptions::channel_binding` overrides that digest. RSA/ECDSA SHA-256/384/512
and RSA-PSS are supported; MD5/SHA-1 signatures use SHA-256. Unsupported algorithms
(such as Ed25519) or absent peer certificates fall back to SCRAM-SHA-256 with the
`n,,` GS2 header. `Config::channel_binding_required` rejects that fallback.


## Driving a connection

1. Construct `Connection::new(Config { .. })`. The host resolves the address,
   connects TCP/Unix transport and supplies any absolute connection deadline.
2. Transmit `output()`. Acknowledge **only actually written bytes** with
   `consume_output(n)`. Retain the borrow until write completion, or copy into a
   reusable host write buffer; never hold a raw pointer while mutating the core.
3. Feed plaintext with `receive(bytes)`, then repeatedly pull `next_event()` until
   it returns `None`. Flush any newly generated output before reading again.
4. `UpgradeTls` is a hard boundary. Finish the existing plaintext write, perform
   and verify TLS in the host. Derive binding with
   `turnloop_tls::tls_server_end_point` from the verified peer leaf, then call
   `tls_established_with_channel_binding(binding.is_some())`. The original
   `tls_established()` acknowledges TLS without binding data. TLS records never
   go into `receive`. Prefer mode can fall back after `N`; Require cannot.
5. For `ScramNeeded`, construct the reexported upstream `ScramSha256` **in the
   host** (its constructor reads entropy), then call `start_scram`. PLUS needs
   `ChannelBinding::tls_server_end_point` containing the certificate digest
   defined in RFC 5929. For plain SCRAM use `ChannelBinding::unsupported()`.
   The core checks the mechanism/binding selection and
   verifies the server signature. Iterations are capped by configuration.
6. Schedule `next_timeout()` in the host. Call `handle_timeout(now)` with supplied
   monotonic time. No method obtains time implicitly.
7. On transport EOF, TLS failure, or an error from `receive`/`next_event`, call
   `abort(error)` and drain events. Do not resume parsing after a protocol error.
   Each accepted token yields one terminal `Completed`; `Error` and command tags
   are informational and must not separately settle a promise. Aborting drains
   all tokens, then emits one `Closed`. Rejected command calls accept no token.

Every event borrows storage until the next mutable call. Materialize JS rows,
errors, notifications and fields before advancing, or retain them with
`Event::into_owned()`: the resulting `OwnedEvent` holds `OwnedRow`, `OwnedFields`
and `OwnedServerError` (also available from `Row`/`Fields`/`ServerError::into_owned`),
each one copy of its wire bytes. `OwnedEvent::as_event()` lends it back as the
borrowed `Event`, so one handler serves both forms. ParameterStatus events allow
hosts to retain whatever session parameters they need; the core does not copy
an unbounded parameter dictionary. No event calls a host callback itself.

## Commands and ordering

`query(token, sql, deadline)` supports multiple simple-protocol statements;
`CommandComplete` separates results and includes the server's actual row count.
`execute(token, ExtendedQuery { name, sql, oids, params, result_formats }, deadline)`
writes Parse/Bind/Describe/Execute/Sync. A nonempty name is cached after
ParseComplete. Reusing a name with different SQL/OIDs is rejected. Reuse before
ParseComplete returns backpressure. Failed parses can be retried. Each extended
operation has its own Sync, so one server error does not discard later tokens.
There is no portal cursor API or automatic statement eviction yet.

`CopyIn` requests host-provided chunks via `copy_data` then `copy_finish`, which
can send CopyFail. COPY OUT emits `CopyOut`, `CopyData`, `CopyDone`, a command tag
and the query completion. Submit COPY when no earlier or later pipelined command
can interfere; interactive COPY and queued query bytes cannot share the input
phase. CancelRequest bytes go on a **new** transport, using the saved backend
PID/secret; they never go on the active query stream. `end` requires drained
queries, sends Terminate, and reports Closed after output acknowledgement.

LISTEN/NOTIFY uses ordinary SQL commands and independent Notification events.
Transactions use SQL; ReadyForQuery tracks Idle/InTransaction/Failed accurately.

## pg conversion contract

`types::decode(oid, format, bytes)` decodes common scalar and array wire types.
The core has no JavaScript objects; the adapter applies these policies.

| PostgreSQL type | Core representation | pg default JS representation |
|---|---|---|
| int2/int4 | Int(i32) | Number |
| int8 | exact Int8(i64) | decimal string |
| float4/float8 | Float(f64) | Number, including NaN/infinity |
| numeric | exact Numeric string, including binary scale | string |
| bool | Bool | boolean |
| text/varchar/name/char | borrowed Text | string |
| bytea | borrowed binary or decoded text bytes | Buffer |
| date/timestamp/timestamptz | ISO text or PG epoch days/microseconds | Date (local-time policy for date/timestamp; instant for timestamptz) |
| json/jsonb | JSON text (jsonb version removed) | host JSON.parse |
| uuid | UUID text (binary formatted canonically) | string |
| arrays of these | nested Array; NULL preserved | nested JS arrays |
| unknown OID | text string or tagged binary Raw | custom parser or string fallback |

JS Date milliseconds discard microseconds; the host must handle timezone,
BC dates and infinity. Binary temporal values retain PostgreSQL's 2000 epoch and
infinity sentinels. Lower array bounds are discarded like pg. The generic codec
uses the same exact numeric semantics in both formats; pg-types' historical
binary numeric/array parsers have inconsistencies, so byte-for-byte JS binary
parser parity is not claimed. Host custom parsers receive OID, format and bytes.

ServerError exposes every field via `fields`/`get`, plus named accessors. Map
`internal_position`→`internalPosition`, `internal_query`→`internalQuery`,
`context`→`where`, `data_type`→`dataType`. Position and line remain strings as
in pg. Metadata includes real dataTypeSize/dataTypeModifier, unlike the old
sqlx binding's -1 placeholders. Callback/promise scheduling, rows-as-objects,
rowMode arrays, URL/environment defaults, host/port and socket options stay in
Perry. Existing Perry int8/numeric conversions differ from pg and are not copied.

## Pool and allocation

`pool::Pool` is lazy, bounded, FIFO for waiters and LIFO for idle reuse. The host
executes Connect/Close events and reports connected/connect_failed/closed.
Checkout/checkin uses generation-checked IDs and token-bearing leases. Idle
expiry, min, max, maxUses, lifetime, explicit destruction, checkout deadlines
and drain-on-end are implemented. Host dispatch supplies nextTick, event emitter
behavior and allowExitOnIdle/ref-unref. This is not a JS Pool wrapper.

RX/TX buffers and pending queues retain capacity. Borrowed rows and metadata
allocate nothing; named registration, SCRAM/startup, pool growth and owned result
materialization can allocate. Bytea text decoding, binary UUID/numeric, arrays
and `into_owned` allocate their result representation. Allocation tests measure
1,000 warmed queries with rows and 1,000 cached executions, each with **zero**
allocations. Buffers may grow for larger workloads; no constant-size claim.

## Tests

`cargo test -p turnloop-postgres` runs protocol, pool, conversion and allocation
tests. Real-server tests are explicitly ignored by default; missing fixture
variables panic when those tests are selected. Run from the workspace:

```sh
python3 scripts/test-servers.py run cargo test -p turnloop-postgres --test server -- --ignored
```

The script creates private data/certificates/logs under `.tools`, uses random
loopback ports, and stops its own servers in a finally block. `start`/`stop` are
also available. See `LANE_REPORT.md` for actual results and server blockers.

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

### Terminal server failures

`ErrorResponse` with nonlocalized severity `FATAL`/`PANIC` (or `S` when `V` is
absent) terminates the session immediately. `Connection::next_event()` returns
`Error::ConnectionAborted(ConnectionFailure)`; its `server_error()` view retains
SQLSTATE, message and all other fields. Close the transport and drain the core's
one `Outcome::Aborted` per pending token, followed by `Closed`. EOF/abort does not
replace the original diagnostic. Startup rejection is terminal too.

The async client closes its stream and returns `io::ErrorKind::ConnectionAborted`
with the typed `turnloop_postgres::Error` as its inner error. Such a client is not
reusable. Ordinary statement `ERROR` stays borrowed and becomes
`Outcome::ServerError` only after `ReadyForQuery`; the session can then continue.

`Error` and `Outcome` are `Clone`, no longer `Copy`. Terminal diagnostics share one
owned copy of the wire fields; cloning pipeline aborts allocates nothing.
Successful queries and reusable statement errors retain zero steady allocations.
