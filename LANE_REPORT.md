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
