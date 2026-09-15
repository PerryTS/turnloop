# adapters-db — wave 3 part 2

Status: implementation and available native/WASI verification complete, with
the explicit integration/runtime limitations below. Based on c506775. The
integrator owns commits. Mandatory design, contribution, integration and relevant
lane reports have been read completely.

## Implemented

- Shared `turnloop-io` exchange cancellation guards, retained receive buffers,
  TLS transport upgrades, resolution helpers and timer-driven pool scheduling.
  SQL/CMAP policy stays in the existing sans-I/O machines. Physical close
  acknowledgements are awaited before pool replacement/end completes.
- PostgreSQL TLS/authenticated Client, simple/extended queries, prepared cache,
  notifications, streaming COPY, separate cancel connection and policy-backed
  Pool (limits, acquire/idle timeouts and maxUses).
- MySQL TLS/caching_sha2/RSA Connection, text/binary commands, multiple results,
  prepared statements, transaction guards and policy-backed Pool.
- Redis pipelines, borrowed subscribers, blocking deadlines, real-timer
  reconnect/replay, ClusterClient slot routing/MOVED/ASK and Sentinel discovery.
- SMTP STARTTLS/implicit TLS, auth, pipelining and per-recipient send results.
- MongoDB authenticated TLS, real-timer SDAM heartbeats, CMAP, retained server
  selection, retryable reads/writes, cursor streaming and drop cleanup. Native
  SRV/TXT uses turnloop's blocking pool (Unix resolver / Windows DNS API).
- WASI 0.2 `ip-name-lookup` integrated with the backend poll set, including
  cancellation and no-spin behavior. Native blocking jobs and physical-close
  futures are exposed by the executor for shared adapter use.
- Per-crate examples and getting-started README sections. CI metadata selects
  async socket suites and real fixture suites; required protocol CI also runs
  WASI 0.2 real servers. The runner rejects zero executed tests.

## Verification

[docs/adapters-db-commands.md](docs/adapters-db-commands.md) records **every
verification command**, including superseded failures. Raw logs are in
`.tools/adapters-db/` (untracked). Superseded failures remain visible; the statuses
below describe final results.

- PASS: `cargo fmt --check`, strict native default/all-feature Clippy, stable
  all-feature check, examples,
  no-tokio cross-target dependency checks, dependency soak, 93 CI script tests, pinned workflow lint
  (actionlint, zizmor and shellcheck), and `git diff --check`.
- PASS: Linux Clippy (all targets), Windows library Clippy, WASI 0.2/0.3 and web
  Clippy (all targets). These are compile checks, not Windows/Linux execution.
- PASS: `cargo test --workspace -- --test-threads=1` and
  `cargo test --workspace --all-features -- --test-threads=1`. The exact default parallel
  workspace run failed twice at the existing process/signal no-spin assertion
  (`crates/turnloop-contract/src/native_surface.rs:440`, `zero <= 1`); retained
  in the command ledger. CI uses serial tests. No assertion was weakened.
- PASS: all five scripted async socket suites (14 tests), including per-thread warmed
  allocation gates. PG/MySQL/Redis/MongoDB reusable operations allocate zero;
  SMTP allocates exactly its existing three owned SendInfo result allocations,
  with zero additional adapter allocation. Counters prove they observe an
  allocation; response/server counts prove commands ran.
- PASS: PostgreSQL/MySQL cancelled queued acquires, pooled idle parking and
  MySQL idle-timeout retirement; MongoDB cancellation, replacement and SDAM idle
  parking; SMTP future-drop EOF and idle parking; Redis real-timer reconnect and
  cancellation. MongoDB tests cover all five read preferences and run retained
  selection against the official selection/staleness corpus. Local CMAP discard
  preserves the other lease and generation.
- PASS: native Redis fixtures: TLS, pipeline, pub/sub, blocking timeout,
  connection kill, real ASK migration/MOVED, actual cluster node restart and
  Sentinel. All pass under WASI 0.2 too.
- PASS: native MongoDB fixtures: auth/TLS, three-node SDAM, timed heartbeat,
  primary step-down, cursors, forced read/write retry (exactly one retry),
  dropped-cursor cleanup verified by CursorNotFound. The final WASI 0.2 real
  suite passes the same checks, including retry counts and cleanup.
- PASS: native SMTP fixture and WASI SMTP delivery with dumped payload checked.
- PASS: full WASI 0.2 portable protocol suites and DNS resolve/cancel slot reuse;
  final rerun includes all newest cancellation/selection/idle tests.
- UNRUN (sandbox): PostgreSQL real suite. Attempted
  `python3 scripts/test-servers.py --services postgres run cargo test -p turnloop-postgres --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture`.
  `initdb` failed at `shmget`, Operation not permitted, before tests launched.
- UNRUN (sandbox): MySQL real suite. Attempted
  `python3 scripts/test-servers.py --services mysql run cargo test -p turnloop-mysql --features turnloop --test async_server -- --include-ignored --test-threads=1 --nocapture`.
  `mysqld --initialize-insecure` crashed with SIGSEGV before tests launched.
  The ledger records these fixture commands as FAIL; adapter runtime is UNRUN.
- FAIL (inherited integration dependency): Windows all-target Clippy cannot
  compile socket tests because `turnloop::backend::Platform` has no Windows
  provider yet. Generic libraries, including Windows DNS bindings, pass strict
  cross-Clippy. No tests were cfg-disabled to conceal this dependency.
- FAIL (pinned compiler): WASI 0.3 debug custom-allocator executable traps before
  main in argument lowering. The documented WASI-lane release workaround is
  declared explicitly in metadata; identical assertions and positive execution
  counts remain required. PASS: full `protocol-wasi --target wasm32-wasip3`,
  including all 14 database/mail async tests in release and existing portable
  suites in their declared profiles.
- PASS: live native SRV/TXT through turnloop's blocking pool, asserting actual
  XMPP service and SPF records. This external-DNS probe is explicitly invoked;
  required deterministic tests also cover positive/malformed wire records,
  off-loop worker completion and native resolver validation.
- UNRUN: Windows/Linux runtime (hosts unavailable).

## Deviations / proposed DESIGN changes

No gate, dependency soak or assertion has been weakened. The existing temporary
rustls security exception remains owned by the repository's security policy.

Proposed additive backend revision: optional `Backend::resolve` plus
`Outcome::Resolved` allow WASI DNS pollables without native blocking workers.
Executor `blocking` and `close` futures share existing completion ownership.
The WASI backend includes DNS readiness in the same bounded OS wait/no-spin
contract; native backends retain the blocking DNS fallback.

Streaming APIs use inherent async `next`/notification methods or borrowed
callbacks, following adapters-net; owned cursor/subscriber values necessarily
allocate when retained. Connection setup allocates buffers once. Dropping an
in-flight wire exchange closes it; the next pool lease can create a replacement.
Cursor drop queues best-effort kill while the Client lives; explicit close awaits
acknowledgement.

Windows' production Platform backend remains the inherited WINDOWS_HANDOFF.md
integration dependency; generic adapters accept any conforming Backend. Web
browsers have no raw database TCP capability. WASI 0.2 supports TCP/A/AAAA;
SRV/TXT requires native DNS or explicitly resolved seeds. These capability limits
are documented rather than silently simulated.

## Open questions / next steps

- Integrator: investigate the parallel process/signal contract failure with
  the core lane; serial default/all-feature suites pass without test changes.
- Integrator: run PostgreSQL/MySQL native and WASI fixture suites outside the
  sandbox, then Linux/Windows runtimes when those hosts/providers are available.
- Confirm the additive DNS/physical-close API with the core integrator.
- A private MongoDB SRV DNS zone would let CI test the entire `mongodb+srv`
  bootstrap against a controlled database fixture; live DNS resolution and core
  SRV domain/TXT policy tests already pass separately.

## Review and rerun entry points

- Shared glue: `crates/turnloop-io/src/{lib,pool,dns}.rs`; executor close/worker
  futures and WASI lookup ownership under `crates/turnloop/src/`.
- Protocol async modules re-export implementations in `client.rs`,
  `connection.rs`, `transport.rs` and `async_pool.rs` as applicable.
- Deterministic suites: `tests/asynchronous.rs` in each of the five protocol
  crates; real suites: `tests/async_server.rs`. Protocol fixture suites are
  intentionally ignored without the unified runner, which requires execution.
- Native real runnable subset (PASS):
  `python3 scripts/test-servers.py --services redis,mongodb,smtp run python3 scripts/ci/run-tests.py protocol --package turnloop-redis --package turnloop-mongodb --package turnloop-smtp`.
- WASI 0.2 real runnable subset (PASS, first source `.tools/wasm-env.sh`):
  `python3 scripts/test-servers.py --services redis,mongodb,smtp run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-redis --package turnloop-mongodb --package turnloop-smtp`.
- Out-of-sandbox SQL WASI rerun (UNRUN): source `.tools/wasm-env.sh`, then
  `python3 scripts/test-servers.py --services postgres,mysql run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-postgres --package turnloop-mysql`.
- CI retains `protocol` and `protocol-wasi` in required `ci-gate` dependencies.
  No remote CI, release or publication was performed in this lane.

## adb-fix1 — completed; SQL runtime awaits integrator

### Root cause and fix

TLS alone selected SCRAM-PLUS, but the async client never derived certificate
binding. It now hashes the verified peer leaf after upgrade, using the shared
`turnloop_tls::tls_server_end_point` helper and allocation-free peer-chain access.
The bounds-checked DER reader selects the **outer signature algorithm**: RSA and
ECDSA SHA-256/384/512, RSA-PSS parameters (including SHA-1 defaults), and RSA
MD5/SHA-1 mapped to SHA-256. An explicit `ConnectOptions::channel_binding` wins.
Unknown algorithms/absent certificates use plain SCRAM with `n,,`.

The sans-I/O machine owns mechanism selection through the additive
`tls_established_with_channel_binding(available)` acknowledgement. The original
`tls_established()` means no binding available. Required binding fails clearly
without data or a PLUS offer and cannot be bypassed by bare AuthenticationOk.
The synchronous driver derives from its actual verified peer certificate and
uses the same helper and fallback. READMEs describe both paths.

This follows [RFC 5929 §4.1](https://www.rfc-editor.org/rfc/rfc5929.html#section-4.1),
[RSA-PSS parameters](https://www.rfc-editor.org/rfc/rfc4055.html#section-3.1), and
[libpq prefer semantics](https://www.postgresql.org/docs/16/libpq-connect.html#LIBPQ-CONNECT-CHANNEL-BINDING).
No new registry packages or versions: all **251** locked registry versions and
checksums are unchanged. Existing ring, rcgen and base64 workspace dependencies
are reused; no soak exception or override was added.

### Tests and verification

**Every verification invocation, including intermediate failures, is recorded in
[docs/adb-fix1-commands.md](docs/adb-fix1-commands.md)**; raw logs live in
`.tools/adb-fix1/`. Final results:

| Command / scope | Result |
| --- | --- |
| `cargo fmt --all --check`; `git diff --check` | PASS |
| Strict `cargo clippy --locked --workspace --all-targets`, default/all features, `-D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test --locked --workspace -- --test-threads=1` | PASS: 245 passed, 13 service tests ignored/UNRUN |
| Same workspace tests with `--all-features` | PASS: 300 passed, 20 service tests ignored/UNRUN |
| `cargo test --locked -p turnloop-postgres -p turnloop-tls --all-features -- --test-threads=1` | PASS: final 34 passed, 5 real SQL tests ignored/UNRUN |
| Strict all-target/all-feature Clippy for both touched crates: WASI p2/p3, web, Linux x86_64 | PASS; p3 uses nightly-2026-09-07 |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --package turnloop-postgres --package turnloop-tls` | PASS: 22 tests, six positive-count suites |
| Same protocol command with `--target wasm32-wasip3` | PASS: 22 tests; existing declared p3 release profile for PG async |
| Windows strict library Clippy and explicit `channel_binding`, `protocol`, `server` test-target Clippy | PASS; execution UNRUN |
| Windows strict all-target Clippy | FAIL: inherited missing `backend::Platform` IOCP provider; no cfg exclusion added |
| `bash scripts/ci/no-tokio.sh`; `python3 scripts/ci/soak.py` | PASS: all policy targets; seven-day policy and existing rustls exception retained |
| Strict rustdoc for both crates; `python3 scripts/ci/feature_modes.py` | PASS |
| `python3 scripts/ci/check-paths.py` | FAIL only because new modules are untracked in the read-only Git index. Same unchanged checker over exact working-tree names PASS (233 references); integrator must stage and rerun |
| Source/lock audit and exact startup-rejection diagnostic test | PASS; original async real-server file preserved byte-for-byte, separate test appended |

Scripted TLS peers execute five cases: rcgen ECDSA P-256 automatic PLUS, explicit
override, Ed25519 `n,,` fallback, missing required binding, and required PLUS.
They verify mechanism, GS2 flag, decoded cbind bytes against an independent SHA-256
of the leaf, client proof, server signature, and a subsequent row. The rejected
connection must drop its owned stream exactly once and send zero startup bytes.
Pure wire tests cover offer/data combinations, legacy acknowledgement, required
binding errors and authentication bypass. Fourteen real OpenSSL certificates
cover every supported algorithm (including PSS with a different MGF1 hash), plus
Ed25519; independent hashlib digests, every truncated prefix and malformed DER/
PSS parameters are checked. These suites are declared for WASI execution.

Allocation gates prove **zero** allocations for 1,000 peer-chain/hash accesses
and 5 × 1,000 hash/fallback operations. Existing query/pool zero gates and TLS's
400 upstream allocations per 100 bidirectional records remain unchanged.

Initial new-fixture failures were teardown assertions: native reset and WASI p2
last-operation-failed do not always report EOF. The final rejection fixture adds
an independent exactly-once stream-drop probe and still rejects any startup
bytes and any client error other than the specified binding error. All SCRAM,
proof, work-count and allocation assertions remain active. No backend/gate was
changed to handle that platform difference.

### Startup panic and real-server status

The reported panic is **expected**, not a swallowed real-server failure.
`rejected_startup_reports_server_sqlstate_and_message` catches its synchronous
test-driver panic, asserts SQLSTATE 28000 and the missing-role message, and joins
the scripted peer. Its exact `--nocapture` rerun prints that panic and passes one
test. A comment now explains this behavior. Actual cancellation observers already
use `scram_user`, not an assumed `postgres` role.

The original `real_async_tls_queries_copy_cancel_pool_and_connection_kill` is
unchanged. A separate ignored `real_async_required_channel_binding_authenticates_over_ssl`
uses auto-derived binding with the required policy and asserts one true `pg_stat_ssl`
row. PostgreSQL 16's `hostssl ... scram-sha-256` fixture offers both mechanisms;
[pg_stat_ssl](https://www.postgresql.org/docs/16/monitoring-stats.html#MONITORING-PG-STAT-SSL-VIEW)
reports TLS, not the SCRAM mechanism. No role-level PLUS-only enforcement was
added; the new test proves PLUS through the client's required/verified policy,
while scripted peers inspect the actual mechanism and binding bytes.

Attempted integrator reproduction:
`python3 scripts/test-servers.py --services postgres,mysql run python3 scripts/ci/run-tests.py protocol --package turnloop-postgres --package turnloop-mysql`.
Fixture command FAIL at PostgreSQL initdb `shmget`, Operation not permitted;
**both SQL suites UNRUN (sandbox)** in this invocation. MySQL startup was never
reached. User-supplied pre-fix outside-sandbox results remain: MySQL server/async
PASS (3 + 1), PostgreSQL sync PASS (4), PostgreSQL async FAIL (binding bug).
No post-fix real SQL pass is claimed.

### Deviations, open questions and next steps

No DESIGN.md changes proposed; no backend, no-spin rule, existing test threshold,
dependency version or security/soak policy changed. No implementation question
remains. Integrator: stage/commit new modules and certificate fixtures, rerun the
index path gate, and rerun the SQL reproduction above outside the sandbox. Also
run `python3 scripts/test-servers.py --services postgres,mysql run python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-postgres --package turnloop-mysql`
after sourcing `.tools/wasm-env.sh` (UNRUN: sandbox). Linux/Windows runtime remains
UNRUN without those hosts; full Windows all-target compilation needs the provider
merge. Browser runtime is UNRUN; raw PostgreSQL TCP is not a browser capability.

## adb-fix2 — SQL and parallel no-spin fixes complete

Base `9862322`; no commits (`.git` is read-only). Integrator-owned PostgreSQL and
MySQL servers remained running: sourced `.tools/sql-env.sh`, never used fixture
start/run/stop or altered `.tools/sql`. No MySQL implementation changes were needed.

### Root causes and fixes

- PostgreSQL already recognized FATAL/PANIC, but replaced the diagnostic with
  `Transport`; async `complete` accepted `Outcome::Aborted` as success. The core
  now returns `Error::ConnectionAborted` immediately, retaining every error field
  in one shared owned copy. It discards unsent output, rejects reuse, and drains
  exactly one aborted completion per pending token followed by Closed. EOF cannot
  overwrite the diagnostic. Async readers return `io::ErrorKind::ConnectionAborted`
  with the typed error and close the stream. Ordinary ERROR remains borrowed and
  becomes `Outcome::ServerError` only after ReadyForQuery. This agrees with
  [PostgreSQL termination](https://www.postgresql.org/docs/16/protocol-flow.html#PROTOCOL-FLOW-TERMINATION)
  and the [nonlocalized severity field](https://www.postgresql.org/docs/16/protocol-error-fields.html).
- The no-spin failure was SIGCHLD interference: every child registration subscribes
  to the process-wide signal, so unrelated parallel child exits notify the quiet
  loop. Controlled 64-child churn reproduced the failure with only this test
  selected; tracing found zero-timeout waits, no EINTR. Its unchanged 60-expiry
  contract now runs in a fresh fixture process, with exit-status and exact output
  checks. Backend code, counters and all numerical bounds are unchanged.
- Repetition exposed a second fixture collision: two SIGUSR1 fan-out tests could
  release each other's waiters, restore SIG_DFL, then kill the test process with
  the other send (exit -30). A test-only mutex isolates those two fixtures. Each
  still exercises four concurrent loops; stress retains all 256 rounds.

### Coverage and verification

Scripted FATAL/PANIC-then-EOF runs **12 cases across six async readers**, checking
kind, SQLSTATE/message, retained detail and exactly-once stream drop. Fragmented
core scripts check severity precedence, pending-token completion and EOF handling.
The real PostgreSQL kill assertion is strengthened to require 57P01 and the exact
message. Everything after it is unchanged and now executes successfully, including
max_uses, pool.end and cancel-future timeout; no later failure was exposed.

The allocation gates retain the original workload and zero thresholds. Added
1,000 measured successful-query/statement-error pairs require zero allocations;
one terminal diagnostic allocation is shared by **64 zero-allocation aborts**.
PG's pure allocation suite is now mandatory on WASI too, using the existing p3
release workaround. `Error`/`Outcome` are **Clone rather than Copy**, an intentional
pre-alpha API change documented in the README. No DESIGN.md change is proposed.

**Every invocation, including failures, is in [docs/adb-fix2-commands.md](docs/adb-fix2-commands.md).**
Raw output, controlled reproducers and repetition commands are in `.tools/adb-fix2/`.

| Verification | Result |
| --- | --- |
| Native real PostgreSQL async_server / server, SQL env sourced | PASS **2 / 4**; server includes one scripted startup rejection |
| Native real MySQL async_server / server, SQL env sourced | PASS **1 / 3** |
| `protocol-wasi --target wasm32-wasip2 --real-servers --package turnloop-postgres --package turnloop-mysql`, both env files sourced | PASS **24**, including all **3 real async SQL tests**, no ignored tests |
| `protocol-wasi --target wasm32-wasip3 --package turnloop-postgres`, WASM env sourced | PASS **18** wire/async/allocation tests |
| `cargo test --locked --workspace` (default parallel threads) | Earlier PASS **248**, 13 ignored; final rerun **FAIL** at unchanged UDP rebind test, detailed below |
| `cargo test --locked --workspace --all-features` (default parallel threads) | PASS **304**, 20 ignored, including both touched crates and MySQL |
| Full native_surface, 10 fresh parallel processes each in default/executor/all-feature modes | Final PASS **30/30**, **480 tests**, **1,800 no-spin expiries**; initial campaign FAIL from SIGUSR1 collision after 20 passes |
| Strict workspace Clippy, default/all features; touched-crate all-target Clippy on Linux x86_64, WASI p2/p3, web | PASS; warnings and undocumented unsafe blocks denied |
| Windows strict library and PG protocol/server/allocation test Clippy with existing Zig wrapper | PASS; runtime UNRUN |
| Windows all-target Clippy | FAIL: inherited absent `backend::Platform` IOCP provider. Initial runs without the wrapper also failed on missing C headers |
| Stable workspace/all-target/all-feature check; fmt; whitespace; path and feature gates | PASS |
| `bash scripts/ci/no-tokio.sh`; `python3 scripts/ci/soak.py` | PASS: eight targets plus union; **251** locked versions, seven-day policy and sole existing rustls exception unchanged |
| Source audit against 9862322 | PASS: production core, shared no-spin body, original PG allocation workload, later real-server assertions, lock/policies unchanged; no new unwrap or unsafe |

### Remaining failures / next steps

The final default workspace rerun hit an **independent, unchanged** core test:
`backend::unix::udp_tests::cancelled_udp_with_cached_events_survives_exact_fd_and_port_reuse`,
`crates/turnloop/src/backend/unix.rs:1073`, `libc::bind` returned -1 instead of 0.
The assertion does not record errno, so the cause is **unconfirmed**. That invocation
ran 10 successful core tests and one failure; subsequent suites were UNRUN in that
invocation. The earlier complete default run and both all-feature workspace runs
passed. No retry was used to replace this final FAIL, and no UDP assertion changed.
Integrator should investigate the bind/reuse fixture separately.

Linux/Windows native runtime and browser runtime are **UNRUN** (hosts unavailable;
browsers have no raw SQL TCP). Full Windows test checking needs its existing
provider integration. The integrator should commit this coherent tree and run
those platforms. No other implementation question remains within adb-fix2; the
UDP default-workspace failure and Windows provider remain explicit quality limits.
