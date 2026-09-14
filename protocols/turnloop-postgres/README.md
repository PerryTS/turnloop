# turnloop-postgres

A pull-driven PostgreSQL v3 client core, built on `postgres-protocol` frontend
messages, framing and SCRAM. It never opens a socket, starts a thread, reads a
clock, schedules a timer or generates entropy. Production code forbids unsafe.

## Driving a connection

1. Construct `Connection::new(Config { .. })`. The host resolves the address,
   connects TCP/Unix transport and supplies any absolute connection deadline.
2. Transmit `output()`. Acknowledge **only actually written bytes** with
   `consume_output(n)`. Retain the borrow until write completion, or copy into a
   reusable host write buffer; never hold a raw pointer while mutating the core.
3. Feed plaintext with `receive(bytes)`, then repeatedly pull `next_event()` until
   it returns `None`. Flush any newly generated output before reading again.
4. `UpgradeTls` is a hard boundary. Finish the existing plaintext write, perform
   and verify TLS in the host, then call `tls_established`. TLS records never go
   into `receive`. Prefer mode can fall back after `N`; Require cannot.
5. For `ScramNeeded`, construct the reexported upstream `ScramSha256` **in the
   host** (its constructor reads entropy), then call `start_scram`. PLUS needs
   `ChannelBinding::tls_server_end_point` containing the certificate digest
   defined in RFC 5929. The core checks the mechanism/binding selection and
   verifies the server signature. Iterations are capped by configuration.
6. Schedule `next_timeout()` in the host. Call `handle_timeout(now)` with supplied
   monotonic time. No method obtains time implicitly.
7. On transport EOF, TLS failure, or an error from `receive`/`next_event`, call
   `abort(error)` and drain events. Do not resume parsing after a protocol error.
   Each accepted token yields one terminal `Completed`; `Error` and command tags
   are informational and must not separately settle a promise. Aborting drains
   all tokens, then emits one `Closed`. Rejected command calls accept no token.

Every event borrows storage until the next mutable call. Materialize JS rows,
errors, notifications and fields before advancing. ParameterStatus events allow
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
python3 scripts/sql-servers.py run cargo test -p turnloop-postgres --test server -- --ignored
```

The script creates private data/certificates/logs under `.tools`, uses random
loopback ports, and stops its own servers in a finally block. `start`/`stop` are
also available. See `LANE_REPORT.md` for actual results and server blockers.
