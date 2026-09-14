# proto-sql lane report

Status: implementation in progress; no completed verification yet.

Read DESIGN.md §§1–5b and §13 and LANES.md completely. Read Perry ext and
stdlib pg/mysql2 bindings. Core and other lanes are read-only references.
No commits or publishing (sandbox mounts .git read-only).

## Surface and stages

| Surface | PostgreSQL / pg | MySQL / mysql2 |
|---|---|---|
| Connection, TLS and auth | In progress | In progress |
| Queries, rows, metadata, errors | Planned | Planned |
| Prepared statements and conversions | Planned | Planned |
| COPY / compression / LOCAL INFILE | Planned | Planned |
| Pool state machine | Planned | Planned |
| Private real-server verification | UNRUN | UNRUN |

## Binding findings

Perry exposes host/port/user/password/database, client creation/connect/end,
query parameters, lazy pools and result metadata. mysql2 also exposes execute,
URI config, rowsAsArray, transactions and pooled connection release. Existing
pg bindings convert int8 to a lossy JS Number and try numeric as f64; Node pg
defaults to strings. mysql2 bindings also currently try DECIMAL as f64. The new
cores preserve exact values and describe the Node conversions for the adapter.

Primary references: https://node-postgres.com/apis/client,
https://node-postgres.com/apis/pool, https://node-postgres.com/features/types,
https://github.com/brianc/node-pg-types/blob/master/lib/textParsers.js,
https://sidorares.github.io/node-mysql2/docs/documentation,
https://github.com/sidorares/node-mysql2/blob/master/typings/mysql/lib/Connection.d.ts,
https://github.com/sidorares/node-mysql2/blob/master/lib/parsers/text_parser.js.

## Dependencies and supply chain

7-day publish-age policy in existing .cargo/config.toml stays enabled.
postgres-protocol 0.6.12: allowed PostgreSQL frontend codecs and SCRAM.
mysql_common 0.38.2, all default features disabled: allowed packets, value codec,
auth and framing; avoids derive, binlog, zstd and C zlib.
bytes: reusable frontend/wire buffers required by upstream codecs.
flate2 rust_backend: portable Rust compression backend required by mysql_common.
rustls (dev only, std/ring/tls12): TLS over std::net in integration drivers.
No asynchronous runtime dependencies are permitted.

## Allocation profile

Design: retain receive/transmit buffers; lend row/packet data until the next
mutable operation. Named statement registration and connection setup may
allocate. Steady-state command queues retain capacity. Measurements pending.

## Deviations / proposed design changes

Upstream PostgreSQL SCRAM constructor reads entropy. Construct its SCRAM object
in the host in response to an explicit request, then pass it into the core, so
protocol progression never reads entropy or a clock. TLS belongs to the host.
No DESIGN.md edits proposed yet.

## Verification

UNRUN: cargo test --workspace; cargo fmt --check;
cargo clippy --workspace --all-targets -- -D warnings;
cargo +stable check --workspace --locked;
cargo check --workspace --target wasm32-wasip2 --locked.

## Open questions / next steps

Implement and test both stages. Verify whether MySQL 9.6 still provides
mysql_native_password (removed in MySQL 9.0); report unsupported server
capabilities rather than mislabeling fixture tests as real-server tests.

## Progress checkpoint

Both stages now have implementations in both crates. Current passing coverage:
wire authentication (clear/MD5/native/caching fast/full TLS and auth switch),
fragmented multi-result queries, extended/prepared execution, error fields,
COPY command generation, transaction status, cancellation packet generation,
LOCAL INFILE denial, bounded framing, 16 MiB MySQL packet continuation,
compressed framing, conversion helpers, and pool FIFO/timeouts/stale releases.
Allocation counters pass: PostgreSQL query/row and named execution and MySQL
plain/compressed ping each allocate zero after warming (1,000 operations).
WASI check passed; browser target feature wiring is being verified.

Real-server status is **UNRUN**, not a pass. Both startup commands **FAIL**:
`python3 scripts/sql-servers.py start`: PostgreSQL initdb's 56-byte shmget
request is denied with Operation not permitted. `TURNLOOP_SQL_SERVER=mysql
python3 scripts/sql-servers.py start`: MySQL 9.6.0 initialization SIGSEGV in
memory::Aligned_atomic<long> (see .tools/mysql-init.log). No server survived.
No sandbox override or system/default-port server was used.

MySQL docs confirm mysql_native_password was removed in 9.0:
https://dev.mysql.com/doc/mysql-security-excerpt/8.0/en/native-pluggable-authentication.html.
Thus MySQL 9.6 alone cannot satisfy the real native-plugin auth requirement;
proposed fixture addition is a private MySQL 8.4 with that plugin enabled.
