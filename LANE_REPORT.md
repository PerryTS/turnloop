# proto-sql lane report

Updated 2026-09-14. Both protocol crates and both stage-2 state machines are
implemented. **Not ready to claim full Node compatibility or real-server
validation.** The current suite has **23 passing tests and 6 explicitly ignored
real-server tests**. Both private server initializers failed before any server
launched. Their test bodies remain **UNRUN**.

Owned deliverables: root Cargo workspace/lockfile, `protocols/turnloop-postgres`,
`protocols/turnloop-mysql`, `scripts/sql-servers.py`, this report and `.tools/`
ignore entry. No core/Perry/other-lane source was changed. No commits, pushes,
publishing, Homebrew changes or system/default-port servers were performed.

Read DESIGN.md §§1–5b and §13 and LANES.md completely. Inspected Perry's ext
pg/mysql2 and stdlib pg/mysql2 bindings and the core lane's `time.rs`. The root
workspace deliberately does not depend on the unfinished loop/executor.

## Implemented surface and verification

“Wire PASS” below means deterministic protocol tests; it does **not** mean a
real PostgreSQL or MySQL server accepted the implementation. The READMEs contain
full host contracts and conversion tables.

### PostgreSQL / pg

| Feature | Implementation | Verification / remaining gap |
|---|---|---|
| Startup, user/database/application name, UTF8/DateStyle | Startup packet; ParameterStatus events | Wire PASS; host retains parameter dictionary if needed |
| SSLRequest, Disable/Prefer/Require | Explicit UpgradeTls and host acknowledgement; no plaintext after acceptance | Wire PASS; actual rustls/server negotiation UNRUN |
| Cleartext and MD5 | postgres-protocol password/MD5 codec | Wire PASS with fixed password and digest bytes |
| SCRAM-SHA-256 and PLUS | Host constructs upstream SCRAM session; core handles exchange, verifies signature and checks binding choice; configurable iteration bound | Wire PASS for both mechanisms and bad-server-signature rejection; real auth UNRUN |
| Simple query/multiple statements | Per-result CommandComplete and server row counts | Wire PASS; real DDL/DML assertions written, UNRUN |
| Parse/Bind/Describe/Execute/Sync, named caching | Text/binary params, format selection, same-name SQL/OID checks, cache after ParseComplete | Wire PASS; per-operation Sync isolates pipeline errors; pending parse reuse returns backpressure |
| Rows/fields | Borrowed, validated iterators; actual table/column/OID/size/modifier/format | Wire PASS; empty result metadata remains available |
| Common types and arrays | Exact integer/numeric, float, bool, text, bytea, temporal, JSON/JSONB, UUID; text/binary arrays | Representative unit PASS; real all-type and array matrix compiled, UNRUN |
| ErrorResponse/NoticeResponse | Every field preserved; named pg-style accessors and generic tag access | Error-field Wire PASS; notice server test UNRUN |
| LISTEN/NOTIFY | Independent PID/channel/payload event | Implemented; real notice/notification assertion UNRUN |
| CancelRequest | Saved backend PID/secret, 16-byte new-connection request | Implemented; active pg_sleep/server cancellation assertion UNRUN |
| Pipelining, transaction state, abort/end | Token FIFO, ReadyForQuery I/T/E, one terminal completion per token, final Closed | Wire PASS including error recovery and timeout |
| COPY IN/OUT | Borrowed format/data events, data/done/fail commands; fresh Sync after extended COPY IN | Wire PASS for COPY input and extended synchronization; real COPY byte equality/row effects UNRUN |
| pg.Pool | Lazy max/min, FIFO waiters, LIFO idle reuse, checkin/destroy, idle/acquire deadlines, maxUses/lifetime, graceful end, generation-checked leases | Unit PASS including min eviction, timeout, stale release and end; real session-reuse fixture UNRUN |

### MySQL / mysql2

| Feature | Implementation | Verification / remaining gap |
|---|---|---|
| Handshake v10/capabilities/TLS | mysql_common handshake packets, supported capability intersection, explicit TLS boundary | Wire PASS; rustls/server TLS UNRUN |
| caching_sha2 fast/full TLS | Fast success, full cleartext only after host TLS acknowledgement | Wire PASS |
| caching_sha2 full RSA | Public-key retrieval, explicit 20-byte entropy request, nonce XOR, OAEP-SHA1 | Wire PASS against independent Python hashlib/modular-exponentiation vector |
| mysql_native_password/auth switch | mysql_common scrambles and plugin selection | Wire PASS; MySQL 9.6 cannot provide a native-password real-server fixture |
| COM_QUERY/multiple results | Borrowed columns/text rows; explicit multipleStatements option | Fragmented wire PASS; real multiple-set fixtures UNRUN |
| PREPARE/EXECUTE/CLOSE/RESET | Parameter/column metadata, statement IDs, binary rows, mysql_common Value serialization | Wire PASS including binary row and exact execute packet; no automatic SQL-keyed LRU helper |
| OK/EOF/ERR and error properties | affectedRows/insertId/warnings/status, errno/sqlState/sqlMessage; common code-name table | Wire PASS; full mysql2 symbolic error registry remains incomplete |
| PING/QUIT/RESET_CONNECTION/changeUser | Explicit commands; reset/changeUser success invalidates statement IDs | Ping/close/timeout wire PASS; real reset/changeUser/quit assertions compiled, UNRUN |
| Transactions | Ordinary SQL and server transaction flags | Implemented; real rollback/server effects UNRUN |
| Common conversions/typeCast surface | Borrowed fields/raw values; default conversion helper/options; host-dispatched hook boundary | Unit PASS for decimal/BIGINT/JSON/BLOB/date policies; real binary/type matrix UNRUN |
| Compression | CLIENT_COMPRESS, retained zlib compressor/decompressor, sequence checks, 24-bit chunking | Wire PASS, including independent mysql_common encoder interoperability, byte fragmentation and exact 16-MiB continuation boundary |
| LOCAL INFILE | Disabled by default; filename event when opted in; host data and end commands | Wire PASS for refusal and explicit successful upload packets; real row effects UNRUN |
| createPool lifecycle | Lazy max, waitForConnections/queueLimit, FIFO waiters, maxIdle/idleTimeout, release/end, generation-checked leases | Unit PASS; real session-variable reuse fixture UNRUN |

## Driver contract and architectural decisions

- Production code forbids unsafe and has `deny(unsafe_op_in_unsafe_fn)`. The only
  unsafe is test allocator instrumentation forwarding unchanged contracts to
  std::alloc::System, with SAFETY comments.
- Feed bytes, pull events, write borrowed output and acknowledge only completed
  bytes. Never mutate a core while an asynchronous transport holds a raw pointer
  to its storage. Keep the Rust borrow or use a reusable host transport buffer.
- Events borrow receive storage. Materialize host results before polling again.
  Server errors/tags/OKs are informational; settle each token only on Completed.
  On receive/parser error, EOF or TLS failure the host **must call abort** and
  drain completions. Parsing errors are terminal and must not be ignored.
- No core opens sockets, reads files or entropy, spawns threads or gets time.
  PostgreSQL's upstream SCRAM constructor is called by the host in response to
  ScramNeeded. MySQL RSA consumes supplied entropy. Tests use std::net and
  rustls; no runtime or sidecar is involved.
- Native/WASI Instant is std::time::Instant. Browser Instant is a host-supplied
  Duration timestamp, matching the core lane's web time representation. Convert
  via as_duration/from_duration until a shared time type exists. This avoids
  std Instant::now's unsupported browser implementation. No timer is owned.
- PostgreSQL has pipelined tokens; MySQL accepts one active wire command and
  returns backpressure otherwise. Its adapter must queue commands FIFO.
- Pool Connect/Close are requests, with explicit connected/connect_failed/closed
  acknowledgements. Closing slots count toward the physical maximum until
  acknowledgement. Host wrappers supply nextTick/EventEmitter/ref-unref and
  must coordinate leases with connection operation state.

## Compatibility limits / Perry migration

Perry's current bindings expose endpoint credentials, client/pool creation,
parameterized query/execute, rows and field metadata, rowsAsArray (mysql2),
transactions (mysql2), and pooled release. They currently have lossy conversions:
pg int8 becomes a Number and NUMERIC is attempted as f64; mysql2 DECIMAL is also
attempted as f64. The new defaults preserve the Node libraries' exact string
semantics rather than copying those losses.

The protocol crates do not implement JS wrappers. These remain adapter work:
URI/environment parsing, host/port/Unix sockets and DNS, TLS verification config,
connection-time defaults, SQL placeholder/formatting helpers, Promise/callback
ordering, JS row objects/tuples, date/timezone construction and JSON parsing.
The typeCast **data surface** is available without callbacks inside the core;
mysql2 field.string/buffer single-consumption and geometry parsing remain host
work. Non-UTF8 MySQL text requires host charset decoding. Per-type dateStrings
arrays and JSON big-number policy are not implemented. Unknown MySQL errno keeps
its number but has no invented symbolic name. Full Node parity is not claimed.

PG temporal binary values retain epoch days/microseconds/infinity; host JS Date
construction must handle local timezone, infinity/BC and millisecond truncation.
Text numeric/int8 semantics are the target; historical pg-types binary
numeric/array inconsistencies are not emulated. Custom parsers can use raw bytes.
There is no PG portal cursor or statement eviction API, and no MySQL automatic
prepare cache/long-data streaming helper. These exceed Perry's immediate bound
surface, but are explicit gaps versus the full Node libraries. Interactive COPY
must be exclusive; detecting COPY IN with multiple pending tokens fails closed.

## Allocation profile (measured, not a universal throughput claim)

Receive/transmit, serialization, column metadata and queue buffers retain their
capacity. All row/field events borrow storage. After warm-up, allocation-counter
tests measured **zero allocations across 1,000 iterations each** for:

- PostgreSQL simple query + row + command tag + completion;
- PostgreSQL named extended execution + row + completion;
- MySQL plain ping and compressed ping;
- MySQL plain queries with 512-byte rows and compressed queries with those rows.

MySQL response fixtures are encoded outside the counter using upstream
mysql_common, exercising independent framing and retained decompression state.
Both result bytes and completion counts are asserted. Registration of new named
statements/IDs, connection authentication/SCRAM/RSA, initial compression state,
pool/buffer growth and materialized owned results may allocate. Text bytea,
binary numeric/UUID and arrays allocate their result representation. Allocation
counters have not yet measured every parameter/type/pool workload or real-server
traffic. Buffers retain the largest observed capacity within configured limits.

## Dependencies and supply chain

The original `.cargo/config.toml` 7-day publish-age soak remains unchanged.
Nightly resolution explicitly selected versions as of seven days ago and
excluded newer candidates. No override or publish-age exemption was used.
Cargo.lock records checksums; it is required for stable checks (stable does not
enforce the nightly-only resolver policy).

| Direct dependency | Purpose / features |
|---|---|
| postgres-protocol =0.6.12 | Allowed frontend codec, header parsing, MD5/SCRAM; browser-only `js` enables its entropy backend for host construction |
| mysql_common =0.38.2 | Allowed handshake/auth/value primitives; **all default features off**, no derive/binlog/zstd/C-zlib/chrono/time integrations |
| bytes 1.12.1 | Reusable buffers, upstream codec interface |
| flate2 1.1.10 | `default-features=false`, `rust_backend`; retained portable compression, no native zlib |
| getrandom 0.4.3 (mysql browser target only) | `wasm_js` enables upstream's mandatory entropy dependency to compile independently on the web; core does not call it |
| rustls 0.23.44 (dev only) | std/ring/tls12, no default aws-lc provider; std socket TLS test driver, host test entropy and SHA256 fixture hashing |

A cargo metadata audit of all 100 resolved packages found no tokio, tokio-util,
async-std, smol, async-io or async-executor. wasm-bindgen is only upstream browser
entropy glue, justified by DESIGN §13's web-target dependencies.

## Verification commands and results

| Command | Result |
|---|---|
| `cargo test --workspace --locked` | PASS: 23 tests; 6 real-server tests explicitly ignored; 0 doctests |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo +stable check --workspace --all-targets --locked` | PASS, Rust 1.97.1 |
| `cargo check --workspace --target wasm32-wasip2 --locked` | PASS (core libraries) |
| `cargo check --workspace --target wasm32-unknown-unknown --locked` | PASS (core libraries) |
| `cargo clippy --workspace --lib --target wasm32-wasip2 -- -D warnings` | PASS |
| `cargo clippy --workspace --lib --target wasm32-unknown-unknown -- -D warnings` | PASS |
| `cargo check --workspace --target x86_64-unknown-linux-gnu --locked` | PASS; compilation only |
| `cargo check --workspace --target x86_64-pc-windows-msvc --locked` | PASS; compilation only |
| `cargo metadata --format-version 1 --locked` + package-name audit | PASS; no forbidden runtime packages |
| Python AST parse of `scripts/sql-servers.py` | PASS |
| `python3 scripts/sql-servers.py stop` | PASS; state and both private PID files absent |
| `python3 scripts/sql-servers.py start` | FAIL: PostgreSQL initdb shmget denied (Operation not permitted), 56-byte System V segment |
| `TURNLOOP_SQL_SERVER=mysql python3 scripts/sql-servers.py start` | FAIL: mysqld 9.6.0 initialize-insecure SIGSEGV in memory::Aligned_atomic<long>; `.tools/mysql-init.log` |
| `python3 scripts/sql-servers.py run cargo test --workspace --test server -- --ignored` | UNRUN test bodies: private initialization blocked as above |
| `TURNLOOP_SQL_SERVER=mysql python3 scripts/sql-servers.py run cargo test -p turnloop-mysql --test server -- --ignored` | UNRUN test bodies: private MySQL initialization failed |
| Process-wide `ps -axo pid=,comm=,args=` inventory | FAIL: sandbox denies ps; revised stop script does not rely on ps |
| Linux/Windows/WASI/browser runtime executions | UNRUN (compile checks only) |

Initial development check failures were fixed, not hidden: compiler/clippy
issues, stable borrow checking of the SSL fallback inside the packet loop, and
browser getrandom feature configuration. No test assertion was weakened to pass.

Both failed initializers exited before `spawn` launched any server. Cleanup ran;
no tracked server state or private PID file remains. The script uses live owned
Popen children for same-process cleanup, and explicit private pg_ctl data / mysqladmin
socket paths for separate stop invocations; it never uses a default instance.

## Deviations / proposed DESIGN changes / open questions

1. **Server environment is blocking real acceptance.** PostgreSQL still needs a
   tiny System V segment even with mmap main shared memory. MySQL's initializer
   crash cause is not proven. An integrator needs a working permitted private
   server environment; no sandbox override was attempted.
2. **MySQL 9.6 cannot test native password.** MySQL removed that server plugin in
   9.0. Add a private MySQL 8.4 fixture with the plugin enabled for that case;
   keep 9.6 for caching_sha2/TLS/compression. The existing wire test is not a
   substitute for this missing real-server case.
3. Clarify §5b that protocol input may request host entropy / receive prepared
   auth sessions, and that wire cores can operate directly without the executor.
   This preserves strict sans-I/O despite upstream auth constructors.
4. Share the core lane's browser time representation through a future common
   API. Current wrappers preserve its supplied-monotonic-time semantics without
   adding a dependency on the unfinished driver.
5. A later adapter must enforce the terminal parser-error/abort contract and
   copy/lease borrowed data before yielding to host code. Decide whether to add
   an owned event layer or automatic error-poisoning API before broader public use.
6. A full mysql2 symbolic errno registry and exact versioned pg binary-parser
   behavior need a pinned Node compatibility target; current differences are
   documented in each README, not silently claimed as matches.

## Next steps

1. Fix the private server execution environment and run all six real-server
   tests. They assert rows, bytes, server-side effects, TLS/auth paths, pool reuse,
   COPY, cancellation, statement lifecycle, reset/changeUser and compression.
   Keep their UNRUN status until those commands actually execute successfully.
2. Add the private MySQL 8.4 native-auth fixture. Verify connection rejection and
   additional server fault/recovery cases with both real databases.
3. Use the preserved raw value/field interfaces in Perry wrappers; implement the
   remaining URI/JS type/date/charset/error-name and callback-order surface.
4. Add thin turnloop adapters once executor/transport APIs land; measure whole
   server workloads and additional pool/parameter allocation cases.

## Primary references

- [pg Client](https://node-postgres.com/apis/client),
  [pg Pool](https://node-postgres.com/apis/pool),
  [pg types](https://node-postgres.com/features/types),
  [pg-types parser implementation](https://github.com/brianc/node-pg-types/blob/master/lib/textParsers.js).
- [mysql2 docs](https://sidorares.github.io/node-mysql2/docs/documentation),
  [connection options](https://github.com/sidorares/node-mysql2/blob/master/typings/mysql/lib/Connection.d.ts),
  [text parser](https://github.com/sidorares/node-mysql2/blob/master/lib/parsers/text_parser.js).
- [MySQL native-plugin removal](https://dev.mysql.com/doc/mysql-security-excerpt/8.0/en/native-pluggable-authentication.html),
  [MySQL handshake protocol](https://dev.mysql.com/doc/dev/mysql-server/latest/page_protocol_connection_phase_packets_protocol_handshake_response.html),
  [PostgreSQL kernel resources](https://www.postgresql.org/docs/16/kernel-resources.html).
