# adapters-db — wave 3 part 2

Status: implementation and verification in progress from c506775. The integrator
owns commits. All mandatory design, contribution, integration and lane reports
have been read.

## Implemented

- Shared turnloop-io exchange cancellation guard, transport flushing, TLS upgrade
  access, address resolution helper and timer-driven pool scheduler. Protocol
  policy remains in the existing SQL and MongoDB sans-I/O pool state machines.
- PostgreSQL authenticated TLS client, simple/extended queries with the existing
  prepared cache, notifications, streaming COPY, separate cancel connection and
  pool; MySQL TLS/RSA authentication, text/binary commands, statements,
  transactions and pool.
- Redis pipeline client, cancellation, real-timer reconnect/replay, subscriber,
  cluster slot routing/MOVED/ASK and Sentinel discovery.
- SMTP TLS/STARTTLS transport and pipelined send with per-recipient results.
- MongoDB authenticated TLS connection, timer-driven SDAM, CMAP pools, selection,
  operation coordinator retries and cursor batches. SRV resolution, cursor
  cleanup and additional cancellation checks remain in progress.
- Executable socket tests for all five adapters; native fixture tests for Redis,
  MongoDB, SMTP and SQL. Test metadata declares portable WASI socket suites and
  required async integration suites.

## Verification

Every verification command, including superseded failures, is recorded in
[docs/adapters-db-commands.md](docs/adapters-db-commands.md). Raw output is retained
under `.tools/adapters-db/` (not committed).

- PASS: `cargo check --workspace --all-features`.
- PASS: all five `--test asynchronous` suites, 7 executed tests. These exercise
  sockets, TLS transitions, cancellation/timeouts, SQL pools and Redis replay.
- PASS: real Redis async suite via `scripts/test-servers.py`, including TLS,
  pipeline, connection kill/reconnect, pub/sub, blocking deadline, cluster and
  Sentinel.
- PASS: real MongoDB base and async suites via the unified runner, including
  authenticated standalone/TLS, CMAP, cursor batches, timed heartbeats and
  replica-set primary step-down.
- FAIL (fix pending): strict all-feature Clippy, three collapsible-if warnings
  in the shared pool scheduler. Earlier compiler failures and fixes are in the
  ledger.
- UNRUN: final fmt/Clippy/stable/workspace/no-tokio/soak/cross-target gates;
  allocation and idle no-spin gates; SMTP real test; WASI runtime suites.
- UNRUN (sandbox): PostgreSQL/MySQL real fixtures (shmget / initializer crash).
  Exact attempted commands will be recorded when the suites are compiled.
- UNRUN (host unavailable): Linux and Windows runtime tests.

## Deviations / proposed DESIGN changes

No gate or dependency-soak policy has been weakened. Public server APIs use
absolute turnloop deadlines and borrowed callback views for allocation-sensitive
responses; owned subscriber/cursor values allocate when retained by the caller.
Windows production Platform backend remains an inherited integration dependency;
generic adapters can use any conforming Backend. WASI ip-name-lookup support is
being added; SRV/TXT needs a native DNS capability or resolved seeds.

## Open questions and next steps

- Complete cancellation/cleanup review, native SRV and WASI name resolution.
- Extend real failure/retry tests, per-thread async allocation gates and pooled
  idle no-spin checks; run all required verification without weakening gates.
- Finish examples, getting-started documentation and required protocol/WASI CI.
- Keep runtime limitations explicit for the integrator's out-of-sandbox rerun.
