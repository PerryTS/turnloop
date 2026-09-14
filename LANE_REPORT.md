# ci-fix3 lane report

Completed implementation and available local verification, 2026-09-14.
SQL/Docker confirmation is still required. The integrator owns commits and CI;
no commits, pushes, dependency changes or sandbox overrides were performed here.

## Implemented and findings

### PostgreSQL CI failure

The supplied backtrace reaches `Driver::connect` at the old server.rs:289: the
observer login as `postgres`. COPY has finished and CancelRequest has **not yet
been sent**. CI sets `POSTGRES_USER=turnloop`; the official Docker entrypoint
passes that name to initdb, so the local-only `postgres` role is not created.
This strongly supports a missing observer role as the cause. The original log
contains no PostgreSQL service error detail, so this remains an evidence-based
fix awaiting real SQL/CI confirmation, not a claimed live reproduction.

- The observer now authenticates as the provisioned `scram_user`, which can inspect
  its own sessions, and matches the exact BackendKeyData PID plus active query.
  The test retains COPY input/output byte assertions, requires SQLSTATE 57014,
  exactly one token-7 error completion with idle transaction state, and a successful
  subsequent query on the original session. CancelRequest uses a separate socket;
  the test waits for its protocol-defined EOF.
- Login diagnostics name the user/SSL mode and retain server SQLSTATE/message in
  the closing panic. A non-ignored real TCP peer regression receives the startup
  packet, rejects `postgres`, and proves the panic preserves 28000 and the message.
  That peer regression does not substitute for a PostgreSQL server run.
- `/opt/homebrew/bin/postgres --version` reports **16.13 (Homebrew)**. CI now uses
  **postgres:16.13**, replacing floating postgres:16.
- `bootstrap-services.sh` was already removed on main; its code is now
  `sql_ci_start` in `scripts/test-servers.py`. Both local and CI provisioning share
  the three SSL ALTER SYSTEM settings, reload, effective SSL/pending-restart checks,
  and version/role/timeout diagnostics. There were no timeout overrides in the
  failing bootstrap. No speculative timeout or server restart workaround was added.
- Local initdb uses the same `turnloop` administrator. Local max_connections now
  retains PostgreSQL's default, matching CI; only loopback port/socket paths differ.
  Both paths use the same HBA rules, certificate filenames, users and SSL settings.
- `--postgres-proxy run` adds a stdlib Python loopback forwarder with separate
  upstream connections, preserved bytes/TLS/half-closes, bounded writes and joined
  shutdown. It reports bytes, connections and CancelRequest counts. Real socket
  tests exercise fragmented cancellation, a 256-KiB query, distinct upstream ports,
  half-closes, and idle-relay shutdown. This models forwarding, not Docker namespaces.

Sources checked: [Docker entrypoint](https://raw.githubusercontent.com/docker-library/postgres/master/docker-entrypoint.sh),
[PostgreSQL statistics visibility](https://www.postgresql.org/docs/16/monitoring-stats.html),
and [CancelRequest flow](https://www.postgresql.org/docs/16/protocol-flow.html#PROTOCOL-FLOW-CANCELING-REQUESTS).

### Continue all protocol suites

`run-tests.py protocol` (and HTTP interop) attempts every declared integration
suite, including later targets within a failed crate. Missing/invalid metadata,
nonzero subprocess exits, spawn errors and zero passed tests all produce FAIL.
The runner prints a per-suite PASS/FAIL table, appends it to GITHUB_STEP_SUMMARY
when present, and fails after all suites have run if any failed. No positive-count
gate was relaxed. Regression subprocesses record their actual execution order;
one failed crate, an empty suite and invalid metadata cannot hide later work.

### Logs and server data cleanup

- CI prints and saves both `docker logs "$POSTGRES_CONTAINER"` and
  `docker logs "$MYSQL_CONTAINER"` whenever the protocol job fails. A shell
  regression proves MySQL logs are still attempted when PostgreSQL log retrieval fails.
- `scripts/test-servers.py logs` prints the existing bounded private-server tails
  and stages only known log files into flat **.tools/protocol-logs/**. Artifact
  upload is confined to that directory; the old `.tools/**/*.log` scan is removed.
  The only fixture cache remains **.tools/redis-build**; instruction artifacts
  retain their separate **.tools/instruction-baselines/** root.
- Mongo Docker wrappers use the host UID/GID so mongod cannot leave root-owned
  files in the bind mount. Cleanup checks container labels, removes containers,
  reaps their docker-run children and verifies closed ports before deleting each
  manifest-recorded data directory. Native Mongo children are stopped/reaped first.
  Unverified/live instances retain their manifest and data.
- Private PostgreSQL/MySQL data is removed after shutdown, including partial data
  from failed initialization. Restrictive directory modes are repaired before
  traversal; symlinks are not followed. Logs/certificates remain outside data roots.
  Tests execute child processes and failed initializers that create restrictive
  data, then verify reaping precedes deletion and logs survive.
- CONTRIBUTING documents the commands, result table and lifecycle changes.
  The supplied `docs/ci-run-34872077144-protocol.log` was deleted as requested.

## Verification ledger

All commands ran in this clone. Full output is under ignored `.tools/verification/`.
Cross-compilation is not runtime proof. The Python script tests initially passed
60, then 61 and 63 tests as coverage grew; the final run passes **64**.

| Command | Result |
| --- | --- |
| `/opt/homebrew/bin/postgres --version` | PASS: 16.13 |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS: final 64 tests; real subprocess/TCP/cleanup assertions |
| `python3 -m py_compile scripts/test-servers.py scripts/fixtures/tcp_proxy.py scripts/ci/run-tests.py` | PASS |
| `cargo fmt --all --check` | Initial FAIL: formatting only; `cargo fmt --all` applied it; final PASS |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS, initial and final |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS; Linux runtime UNRUN |
| `cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL: missing Windows SDK C headers (`assert.h`, etc.) in ring/zstd; Windows test-target checking and runtime UNRUN |
| `cargo clippy -p turnloop-postgres --lib --target x86_64-pc-windows-msvc -- -D warnings` | PASS; library cross-check only |
| `cargo +stable check --workspace --all-targets --all-features` | PASS, initial and final, stable 1.97.1 |
| `cargo test -p turnloop-postgres --test server rejected_startup_reports_server_sqlstate_and_message` | PASS: actual TCP rejection and diagnostic assertions |
| `cargo test --workspace` | PASS: 193 passed, 12 ignored; ignored subjects are not counted as executed |
| `scripts/ci/no-tokio.sh` | PASS: every configured target/default/all-feature graph; no bans changed |
| `python3 scripts/ci/soak.py` | PASS: 240 locked registry versions; only the inherited exact rustls security exception; seven-day policy unchanged |
| `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck` | PASS: committed checksum pins |
| `PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` | PASS: strict existing queue validation, supported actionlint checks, zizmor, ShellCheck |
| `.tools/bin/actionlint -color` | FAIL: only the two inherited concurrency.queue parser errors in actionlint 1.7.12; compatibility handling unchanged |
| `.tools/bin/zizmor --offline --min-severity low .github/workflows` | PASS: no findings |
| `scripts/test-servers.py --services postgres --postgres-proxy run cargo test -p turnloop-postgres --test server -- --include-ignored --test-threads=1 --nocapture` | UNRUN (sandbox): command exits 1 at initdb/shmget before PostgreSQL or proxy test bodies run |
| `scripts/test-servers.py run cargo test --workspace -- --include-ignored` | UNRUN (sandbox): command exits 1 at PostgreSQL initdb/shmget; no test bodies execute |
| `scripts/test-servers.py --services mysql run true` | UNRUN (sandbox): mysqld initialization exits 2, signal 11 / invalid mapped-object permissions; no MySQL server or tests run |
| `scripts/test-servers.py --services redis,mongodb,smtp,http run cargo test -p turnloop-redis -p turnloop-mongodb -p turnloop-smtp -p turnloop-http -p turnloop-tls -p turnloop-websocket -p turnloop-zstd-decoder -- --include-ignored --test-threads=1` | PASS: 145 passed, zero ignored, actual Redis/Mongo/SMTP/HTTP/TLS/WebSocket and decoder workloads |
| `scripts/test-servers.py logs` | PASS: 23 flat log files retained; no data directories traversed |
| Python post-run inspection of recorded Mongo ports/dbpaths and all service state files | PASS: all five ports closed, all five Mongo data directories removed, SQL data absent, all private state absent |
| `git diff --check` | PASS |
| `scripts/test-servers.py --ci-services run python3 scripts/ci/run-tests.py protocol` | UNRUN: Docker/Linux unavailable |

The existing workspace allocation gates executed unchanged. No new production
I/O operation or steady-state allocation path was introduced, so no allocation
budget needed extending. No unsafe block, I/O unwrap, dependency or backend change
was added. No test threshold, fixture requirement, no-spin rule or CI gate was weakened.
Windows and WASI/web required jobs are retained; wasm fixes belong to the other lane.

## Deviations, open questions and next steps

No DESIGN.md changes proposed. The authoritative design, contribution guide,
integration report and relevant CI/SQL/Mongo/KV/HTTP lane reports were read.

The remaining confirmation is environmental: does CI's now-correct observer
complete the existing COPY/cancel sequence, and do Docker cleanup/log upload pass?
There is no live SQL reproduction claim from this sandbox. The integrator should:

1. Run the exact PostgreSQL proxy command above outside the sandbox; expect all
   four server-target tests to pass and at least one logged CancelRequest over a
   distinct proxied connection. Review printed version/roles/settings.
2. Run `scripts/test-servers.py run cargo test --workspace -- --include-ignored`
   outside the sandbox, including both SQL suites.
3. Push the checkpointed tree and watch CI: confirm all protocol suite rows run,
   all rows pass, and artifact upload never visits a database directory. On any
   failure inspect both SQL container logs and the staged private logs.
4. Use native Windows CI for the test-target compile/runtime checks that require
   Windows SDK headers. Linux/Windows/Docker runtime is UNRUN here.
