# ci-fix2 lane report

Updated 2026-09-14. Requested changes implemented and locally verified. The
integrator owns commits, pushes and the hosted rerun. External checkpoints appeared
while this lane worked; no Git mutation was attempted. Full logs and command/exit
records are under ignored `.tools/ci-fix2-checks/`.

## Findings

Read DESIGN.md, CONTRIBUTING.md, docs/INTEGRATION_REPORT.md, and the CI, KV,
MongoDB and SQL lane reports completely.

The supplied job log and checked-out workflow contradict the suspected apt Redis
explanation. `--ci-services` prepended generated `redis-server` and `redis-cli`
Docker wrappers using floating **redis:8**; apt installed only Postfix. The first
five-second wait expired while the wrapper was still alive. An on-demand cold
image pull is a plausible cause, but discarded stdout/stderr prevents proving it.
The log does **not** establish rejected Redis directives, missing TLS or Redis 7.
SQL service bootstrap and its probes succeeded; no Rust protocol test ran before
Redis startup/cleanup failed. The supplied
`docs/ci-run-34869729240-protocol.log` was deleted as requested after this summary.

## Implemented

- **Pinned Redis 8.4.0 TLS build.** `scripts/ci/install-redis.py` downloads the
  official release tarball, verifies SHA-256
  `ca909aa15252f2ecb3a048cd086469827d636bf8334f50bb94d03fba4bfc56e8` before
  extraction/build, and builds server + CLI with `BUILD_TLS=yes`, modules disabled,
  libc allocator, and no systemd dependency. The version exactly matches local
  Homebrew Redis 8.4.0. A real source build and cache reuse both passed on macOS.
  CI installs build-essential/libssl-dev/pkg-config and caches only binaries plus
  build identity under `.tools/redis-build`, keyed by Ubuntu version, architecture
  and installer hash, with no fallback restore keys. Cache hits check exact versions
  and CLI TLS support; the unchanged real-server tests prove server TLS works.
  Installation exports the native binaries through GITHUB_PATH.
- **CI fixture topology retained.** Redis Docker wrappers are removed, including
  stale wrappers from an older run. Single/ACL/TLS, six cluster nodes (three masters
  and three replicas), and Sentinel remain. MongoDB still uses the same five private
  containers with ownership labels; its image is pulled before its readiness timer
  starts. SQL containers and fixtures remain intact. All private server `.log`
  files are retained in CI artifact `protocol-server-logs` for seven days.
- **Private server diagnostics.** Shared capture/readiness helpers save stdout and
  stderr together under `.tools/`. Redis logs everything to captured server.log,
  including pre-configuration errors. Mongo reports process.log and mongod.log;
  SQL reports initializer/console/internal logs; SMTP and its supervisor share
  server.log. Failed startup/timeout prints the last 40 lines, bounded to 16 KiB per
  file, with service name and exact path. Missing internal logs cannot replace the
  original failure. SMTP errors are labeled SMTP, and failed sink spawning cleans
  up its supervisor socket.
- **Crash-safe Redis stop.** Same-process cleanup owns Popen handles, reaps exited
  children immediately, terminates even children that never opened a port, and
  tolerates exit races. Separate invocations never signal stored PIDs: no process
  or a refused private port means stopped; live listeners must identify the exact
  config through INFO before SHUTDOWN. A failed shutdown command followed by a
  closed port counts as stopped. All records are attempted; only live/unverified
  failures retain records. Successful cleanup deletes instances.json and env.json.
- **Original errors remain primary.** Redis, SQL, MongoDB, the top-level start, and
  failed test commands chain cleanup failures beneath the original error. Cleanup
  aggregates actual exceptions rather than dropping their tracebacks. Stubborn
  instances still fail and retain state; no gate or test assertion was relaxed.
- **Regression tests.** `scripts/ci/test_servers.py` launches real deliberately
  crashing executables for Redis, MongoDB, SMTP, SQL initialization and SQL server
  startup, asserting stdout, stderr, exit status and removed state. It also checks
  bounded tails/timeouts, missing binaries, no-process/refused-connection cleanup,
  never-listening owned children, stubborn-child record retention, private-config
  mismatch, shutdown/crash races, original startup/test error chaining, and Mongo
  wrapper ownership with stale Redis wrapper removal. `test_redis_install.py`
  rejects corrupt downloads before extraction/build and executes binary probes
  rejecting Redis 7, near-match versions and missing TLS; Redis's optional CLI git
  suffix is accepted without relaxing the exact release check.
- **CONTRIBUTING.md** documents native CI Redis, caching, logs, artifacts and cleanup.

Primary pin evidence: [Redis's official release hashes](https://github.com/redis/redis-hashes/blob/master/README),
[Redis 8.4.0 build rules](https://github.com/redis/redis/blob/8.4.0/src/Makefile),
[actions/cache v6.1.0](https://github.com/actions/cache/releases/tag/v6.1.0).
The cache action is pinned to verified official commit
`55cc8345863c7cc4c66a329aec7e433d2d1c52a9`; its June 26 release is already soaked.

## Verification ledger

Pinned nightly-2026-08-20, stable cargo/rustc 1.97.1, macOS arm64. No Rust source or
operation path changed: no new unsafe blocks, I/O unwraps, runtime dependencies or
per-operation allocations. Existing allocation gates ran in workspace tests.

| Command / probe | Result |
|---|---|
| `redis-server --version`; `cargo +stable --version`; installed toolchain/target inventory | **PASS**, local Redis 8.4.0, libc; stable 1.97.1 |
| Official Redis hashes, source Makefile, actions/cache release/tag API reads; actual tarball SHA-256 | **PASS**, exact pins above verified |
| `python3 -m py_compile scripts/test-servers.py` | **PASS** |
| `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck` | **PASS**, checksum-pinned actionlint 1.7.12, zizmor 1.30.1, ShellCheck 0.11.0 |
| `python3 scripts/ci/install-redis.py` (final fresh source build) | **PASS**: verified official 8.4.0 archive, actual server/CLI compilation with TLS, exact version checks |
| `python3 scripts/ci/install-redis.py` (second invocation) | **PASS**: cached binaries verified and reused, no download/build |
| `scripts/test-servers.py run cargo test --workspace -- --include-ignored` | Command **FAIL** at PostgreSQL initdb `shmget` permission denial; all test bodies in this invocation **UNRUN (sandbox)**. Correct PostgreSQL log tail printed; state cleaned |
| `scripts/test-servers.py --services mysql run true` | Command **FAIL** at MySQL initialization, exit 2/fatal signal in Aligned_atomic/Shared_spin_lock/delegates_init. SQL runtime tests **UNRUN (sandbox)**. Console/internal log tails printed; state cleaned |
| `PATH="$PWD/.tools/redis-build/bin:$PATH" scripts/test-servers.py --services redis,mongodb,smtp run cargo test --workspace --exclude turnloop-postgres --exclude turnloop-mysql -- --include-ignored --test-threads=1` | **PASS**, **73 tests**, including all four external Redis/MongoDB/SMTP tests, no ignored tests in selected crates. Explicit SQL exclusion is local-only; CI's full required suite unchanged. All fixture cleanup succeeded |
| `PATH="$PWD/.tools/redis-build/bin:$PATH" scripts/test-servers.py --services redis start`, CLI topology/TLS assertions, separate `scripts/test-servers.py stop`, socket/state assertions | **PASS**, eight Redis processes, exactly three masters + three replicas; certificate-verified TLS PONG; all nine private TCP/TLS ports closed; instance/env/top-level state removed |
| `python3 -W error scripts/ci/test_servers.py -v` (initial regression development) | **FAIL**, four SQL subcases used a dict where the test double needed a directory supporting `/`. Corrected test setup; real crash assertions retained |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | **PASS**, **43 tests** in final run; earlier 39-test run also passed before additional installer/error-chain coverage |
| `cargo fmt --check` | **PASS** |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS**, native |
| `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS**, Linux compile check; runtime **UNRUN** |
| `cargo +stable check --locked --workspace --all-targets --all-features` | **PASS**, stable 1.97.1 |
| `cargo test --workspace` | **PASS**, **92 tests**; 10 external-service tests ignored by this default command and not counted as executed |
| `bash scripts/ci/no-tokio.sh` | **PASS**, eight targets × default/all features |
| `python3 scripts/ci/soak.py` | **PASS**, 193 locked registry versions; existing rustls security exception unchanged; seven-day policy active |
| `PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` | **PASS**, actionlint with existing strictly validated queue compatibility filter, zizmor zero new findings, ShellCheck |
| `.tools/bin/actionlint -color` | **FAIL**, only the two pre-existing unsupported `concurrency.queue` diagnostics in 1.7.12. The existing wrapper passes; no lint rule/filter was weakened |
| `git diff --check` and final scope/state inspection | **PASS** |
| Hosted Ubuntu Redis build/cache, actual Docker SQL/Mongo and protocol job, artifact upload | **UNRUN**, no Linux/Docker locally; integrator reruns CI |
| Windows, WASI and web runtime checks | **UNRUN**, outside this automation lane; their jobs/backends unchanged and wasm fixes owned by the other lane |

Two installer development failures were corrected before the final passes: the
first default-urllib request received HTTP 403 (explicit turnloop-ci User-Agent
succeeded with the expected digest); the first completed build was rejected because
redis-cli appends `(git:...)`. Validation now compares the exact version token and
has adversarial near-match/version tests. No checksum or TLS requirement changed.
The initial source download probe using a guessed plural hostname failed DNS;
the committed singular official download URL is verified and succeeded.

The MySQL initializer's partial data was moved to
`.tools/sql/mysqldata-sandbox-failed-20260914`, preserving diagnostics and leaving
`.tools/sql/mysqldata` absent for an outside-sandbox retry. Final inspection found
no top-level, Redis, MongoDB or SQL instance state and no SMTP control socket.

## Deviations and proposed DESIGN.md changes

No DESIGN.md change. No backend/no-spin/allocation contract, dependency soak,
forbidden-runtime policy, test count gate, CI requirement or Rust assertion was
relaxed. The implementation replaces the actual Redis Docker wrapper path rather
than an apt Redis installation that was not present in the supplied workflow.
Pre-pulling Mongo prevents the same hidden image-download delay on its first start.

## Open questions and next steps

No implementation questions remain. The exact discarded Redis startup diagnostic
from run 34869729240 cannot be recovered; the new logs make future failures visible.

1. Integrator commits final working tree, pushes and reruns full CI, especially
   `protocol`. Confirm Ubuntu builds/cache reuse and the full required SQL/Redis/
   Mongo/SMTP suite; inspect `protocol-server-logs` if a fixture fails.
2. Outside the sandbox, run the full local command from the ledger to execute SQL
   tests too. SQL remains UNRUN here; no cross-check substitutes for its execution.
3. Keep the existing unrelated wasm lane and actionlint queue-support follow-up.
   No Linux/Windows/WASI/web runtime success is claimed by this lane.
