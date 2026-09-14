# ci-fix3 lane report

In progress, 2026-09-14. Read DESIGN.md, CONTRIBUTING.md, the integration report,
and CI/SQL/Mongo/KV/HTTP lane reports. No design, dependency, soak, backend or
runtime-ban changes; no commits (the integrator owns Git writes).

## Findings and implementation

- The supplied CI backtrace reaches `Driver::connect` at server.rs:289, the
  observer login as `postgres`, before CancelRequest. CI uses POSTGRES_USER=turnloop;
  Docker passes that value to initdb and does not create a postgres role. This is
  the supported cause, pending real SQL/CI confirmation. Only three SSL settings
  were applied by the bootstrap, with no statement/idle timeout overrides.
- The old bootstrap-services.sh is already removed in main; its implementation
  lives in scripts/test-servers.py. Changes apply there.
- Local `/opt/homebrew/bin/postgres --version`: PostgreSQL 16.13 (Homebrew).
  CI image pinned to postgres:16.13.
- Observer uses scram_user and matches the actual BackendKeyData PID. COPY byte
  assertions remain, cancellation requires SQLSTATE 57014, exactly one token-7
  completion, idle transaction state and a successful subsequent query.
  Failed connection diagnostics include the server error fields.
- Local initdb now uses turnloop; local and Docker provisioning share the three
  ALTER SYSTEM settings, pg_reload_conf, effective SSL/pending-restart checks and
  version/role/timeout diagnostics. Local --postgres-proxy run mode adds distinct
  TCP upstream connections and retains CancelRequest/byte counters.
- Protocol runner collects every declared suite, preserves positive-execution
  gates, appends a PASS/FAIL table to GITHUB_STEP_SUMMARY and fails at the end.
- CI prints both SQL service container logs on failure. Known private logs are
  staged in flat .tools/protocol-logs; upload only visits that directory. Redis
  build cache remains .tools/redis-build, independent of fixture data.
- Mongo runs as the host UID/GID in Docker. Cleanup stops/removes containers,
  reaps children, checks ports, then removes recorded data paths, preserving logs.
  Private SQL data is removed after shutdown and failed initialization.

## Verification ledger

| Command | Status |
| --- | --- |
| `/opt/homebrew/bin/postgres --version` | PASS: 16.13 |
| Tests/lints/full private-server command | UNRUN: implementation in progress |
| Linux/Windows/Docker runtime | UNRUN: unavailable on this host |

## Deviations / design proposals / questions

No DESIGN.md changes proposed. No production I/O or allocation path changed.
The forwarding proxy is test tooling, not a backend. SQL sandbox limits may
prevent reproducing the observed missing-role failure against a live server.
Docker runtime, UID mapping, service logs and the final protocol table need CI.

Primary references checked:
- https://raw.githubusercontent.com/docker-library/postgres/master/docker-entrypoint.sh
  (initdb --username=POSTGRES_USER).
- https://www.postgresql.org/docs/16/monitoring-stats.html
  (ordinary users can inspect their own sessions).
- https://www.postgresql.org/docs/16/protocol-flow.html#PROTOCOL-FLOW-CANCELING-REQUESTS
  (CancelRequest uses a new connection, closed by the server without reply).

## Next steps

Complete regression coverage and local verification, remove the supplied CI log,
and hand the coherent tree and exact UNRUN rerun commands to the integrator.
