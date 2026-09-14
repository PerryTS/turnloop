# Integration report

Updated 2026-09-14. HTTP integration produces a twelve-member workspace with ten
publishable crates. The current main merge retains HTTP/TLS/WebSocket and h2spec,
main's service diagnostics/cleanup, deterministic MySQL authentication and portable
Windows test gates. rustls 0.23.45 passes the security audit and the seven-day soak
gate with main's exact dated exception. Linux instruction baselines are committed.
SQL sandbox and production Windows/WASM backend prerequisites remain pending;
neither cross-compilation nor package dry runs establish release readiness.

## Current main merge

Resolved all conflict markers in `scripts/test-servers.py`,
`scripts/ci/run-tests.py` and `scripts/ci/test_servers.py` by combining both parents.
HTTP remains a default fixture and retains authenticated shutdown, Node/curl
interop, strict h2spec and both WASI protocol targets. Native default/all-feature
runs independently require positive core/protocol counts, with only main's
documented pending Windows production contracts marked pending. Redis uses main's
pinned native 8.4.0 TLS build in CI, config-verified detached cleanup and owned-child
reaping. Per-server logs, bounded failure tails, original-error chaining and the
MySQL auth-admin cache reset are preserved. HTTP startup now shares the failure-tail
helper; its new tests prove real child output, reaping and default fixture selection.

Semantic review includes `Cargo.toml`, `Cargo.lock`, CI workflow, policy/soak code,
this report and CONTRIBUTING. All 16 fixture tests from both parents survive;
three regressions were added. Locked metadata/build, all 49 Python tests, soak
(240 versions, exactly one exception), no-tokio, cargo-deny and workflow lint pass.
Default/all-feature Clippy passes natively and for Linux, WASI 0.2 and browser WASM;
stable 1.97.1 and rustdoc pass. Native and WASI package dry runs verify all ten
archives; all eight archives that contain rustls lock 0.23.45. The unpacked decoder
passes its allocation regression. No package needs a consumer path/patch override.

The full server command fails during PostgreSQL initialization (`shmget` denied),
and MySQL initialization reproduces the sandbox crash: SQL bodies are **UNRUN**.
The non-SQL subset passes 175 tests, with no ignored tests, and all private state
is removed. Full Windows cross-Clippy fails on missing SDK C headers; core and
contract/bench all-target checking passes. Windows/Linux runtime and Docker are
**UNRUN**. Raw actionlint still rejects only the inherited `concurrency.queue`
syntax; the existing strict compatibility wrapper, zizmor and ShellCheck pass.

An additional whole-workspace WASI 0.3 Clippy attempt fails in inherited
`bson 3.1.0 -> ahash/rand -> getrandom 0.3.4`: that getrandom version rejects p3.
HTTP/decoder protocol execution on p3 passes all 20 tests. The required workspace
WASI job remains intact; this dependency compatibility issue and the pending
production provider need resolution before that whole-workspace gate can pass.
The Linux instruction gate correctly refuses this macOS host; the committed
baseline is unchanged and merged-tree regression execution is **UNRUN** locally.
Exact commands, failures and limits are in [LANE_REPORT.md](../LANE_REPORT.md).
The integrator must stage and commit the merge; `.git` is read-only here.

## HTTP integration (current)

- Added publication metadata and unified dependencies for `turnloop-tls`,
  `turnloop-http`, `turnloop-websocket` and `turnloop-zstd-decoder`. All local
  dependency edges have explicit registry versions. rustls 0.23.45 and ring
  0.17.14/std/tls12 are shared with every database/SMTP TLS harness. Browser ring
  entropy/PKI features are explicit; WASI uses host randomness. LLVM supplies the
  wasm C compiler. rustls defaults/aws-lc are disabled on every target.
- Selected soaked tungstenite 0.30.0 to share SHA-1 0.11/getrandom 0.4 with the
  existing workspace; removed the HTTP lane's separate getrandom 0.3 configuration.
  Remaining incompatible random generations come from upstream BSON/ring and
  upstream decoder tests, not competing workspace API definitions. Removed
  rustls-pemfile and used rustls-pki-types PEM parsing. Mozilla trust-anchor data's
  permissive CDLA license was reviewed and its redistribution text included.
- Resolved the publication blocker with a separately named MIT decoder fork.
  Unmodified upstream 0.8.3 reproduces **6000 allocations for 1000 frames**;
  upstream 0.9.0 still contains the two private allocation sites. Public APIs or
  hash configuration cannot avoid them. The published fork retains the allocation
  patch and upstream license/source provenance, 101 ordinary and 207 dictionary
  frames, and 47 fuzz artifacts verified against upstream Git blobs. No root
  `[patch]` and no consumer patch requirement remain. Exact upstream submission
  text is in [docs/upstream/ruzstd.md](upstream/ruzstd.md).
- The fork runs upstream native tests plus an exact-byte, 1000-frame allocation
  regression. HTTP's native `pure-rust-zstd` feature exercises the same decoder
  as wasm. Corpus counts, checksum/byte assertions and zero thresholds are strict.
  WASM dictionary hash arithmetic was widened from isize to i64 to compile with
  identical native arithmetic. Native-only C-reference tests remain active on
  native CI; pure decoder checks build on Windows and both WASM targets.
- All fixture lifecycle operations now go through `scripts/test-servers.py`.
  Node HTTP/1 and HTTP/2 expose `TURNLOOP_TEST_HTTP_PORT`/`HTTP2_PORT`, authenticated
  private shutdown, and closed-listener assertions. Removed the six superseded
  HTTP lane scripts; the Node fixture implementation lives in `scripts/fixtures/`.
  Native HTTP/TLS/WebSocket tests run with pinned Node 26.5.1 in the matrix.
- `scripts/ci/h2spec.py` installs source commit
  `70ac2294010887f48b18e2d64f5cccd48421fad1` using SHA-256
  `791b995048c7e2a2895ed2c019eb9abe46015c3a7a6107cb9ea3f5e5f311da39`, originally
  verified against all 115 Git blobs. Strict JUnit validation rejects missing,
  duplicate, skipped, failed or inconsistent results. The response is now 16 KiB,
  so the negative-window test executes: **147 passed, 0 skipped, 0 failed**.
  Required `h2spec` and `protocol-wasi` jobs feed `ci-gate` without exemptions.
- WASI 0.2 **and 0.3** each execute 16 HTTP codec tests, three allocation tests,
  and the decoder allocation regression. The counting gates use standalone test
  harnesses: every previous function runs unconditionally and allocator calibration
  proves a known allocation is counted. This avoids pinned p3 libtest's CLI-argument
  lowering calling a generated custom allocator shim without a valid stack. The
  initial debug trap is recorded; no test or allocation threshold was removed.
- Local validation: default workspace suite **192 passed**, CI default/all-feature
  wrapper **639 passes including independent member/contract repetitions**, HTTP interop **15/15**,
  non-SQL private-server workspace subset **175 passed, zero ignored**; strict
  native/Linux/WASI/browser Clippy, stable 1.97.1, rustdoc, workflow lint and
  49 automation tests pass. All ten native/WASI package builds verify tarballs.
  The packaged decoder also executes its allocation regression independently.
  Exact commands, intermediate failures and limits are in [LANE_REPORT.md](../LANE_REPORT.md).

### Current security exception

Main replaces vulnerable rustls 0.23.44 with 0.23.45 for RUSTSEC-2026-0285 and
provides the reviewed exact-version exception in `scripts/ci/policy.toml`.
`cargo deny` now passes all four gates. The unchanged resolver configuration and
independent soak check still enforce every other age and all checksums. The gate
prints exactly one active exception and rejects expired/unused/malformed entries.
Remove it at **2026-09-21T15:11:17Z**, seven days after the registry index timestamp.
No additional exception, advisory ignore or resolver override was introduced here.

The following sections preserve the earlier wave-1 implementation and verification
evidence; current HTTP additions and merge results above supersede their old counts.

No commits, pushes, remotes, repositories or registry uploads were performed by
this agent. The integrator checkpoints the working tree. No other lane was
written. Builds, downloaded tools, private servers and logs use this checkout,
Cargo/Rustup caches and temporary build support paths. A temporary editing helper
was moved into `.tools/`; no external project was modified.

## Implemented

- Renamed core/helper directories, packages, identifiers, cfg/environment names,
  benchmark names, scripts, comments and documentation. Historical lane reports
  remain under `docs/lanes/`. Rewrote README with pre-alpha status and actual
  platform coverage. Removed the duplicate `core.yml` workflow.
- Root members are `crates/*` and `protocols/*`; `spikes/*` are excluded. Edition
  2024, resolver 3, MIT, public repository metadata, shared dependencies and
  inherited unsafe/cfg lints. All six publishable packages have descriptions,
  README, keywords, categories and documentation URLs. Path dependencies have
  explicit versions. Contract and benchmark helpers are private; no protocol
  depends on the contract helper, including for tests.
- Stable here is **rustc 1.97.1**, despite the environment description's 1.98.
  Declared and verified MSRV is 1.97.1; CI checks both that version and floating
  stable. The root lockfile was regenerated with nightly-2026-08-20 and the
  unchanged seven-day resolver soak, then independently age/checksum-audited.
- One port-based `TURNLOOP_TEST_*` convention is documented in CONTRIBUTING and
  consumed by every native protocol harness. Missing fixtures are fatal even
  without `TURNLOOP_TEST_REQUIRED=1`; real-server tests remain ignored by default.
- `scripts/test-servers.py` owns start/stop/run, private data and cleanup. Preserved
  PostgreSQL cleartext/MD5/SCRAM/TLS/PLUS users; MySQL caching-SHA2 fast/full/RSA/TLS,
  native-plugin availability probe, compression and LOCAL INFILE; Redis ACL,
  single/TLS, six-node cluster (three masters/three replicas) and Sentinel;
  MongoDB standalone, three authenticated replicas and TLS; Postfix SMTP sink
  with actual message-byte assertions. PostgreSQL fixture users now receive
  schema privileges needed by PostgreSQL 16 tests. Certificates refresh on expiry.
- Deleted superseded SQL/Redis/Mongo scripts and CI bootstrap/cleanup scripts.
  MongoDB shutdown is an explicit lifecycle example, not an ignored test that
  `--include-ignored` could execute before another suite. SMTP uses a private,
  authenticated local supervisor socket so separate start/stop invocations work
  without `ps`, PID-reuse guesses or system service shutdown.
- Linux CI uses the same runner/configuration/environment. It provisions the
  explicitly named PostgreSQL/MySQL service containers, performs independent
  verified TLS probes, and creates private Redis/Mongo containers with ownership
  labels. Mocked wrapper/lifecycle regressions run in CI; actual Docker execution
  remains UNRUN locally. The full service suite is still required.
- Kept native/default/all-feature, stable/MSRV, wasm cross-lints, docs, no-tokio,
  cargo-deny, soak, loom, Miri, instruction and ci-gate checks. Retained core's
  portable timer benchmark. All-feature Linux tests select the timerfd fallback;
  default tests select epoll_pwait2 where available.
- Added actual Callgrind workload functions: idle waits, notifier consumption,
  timer cancel/close and an independent integer control. Each asserts execution.
  Migrated the iai-callgrind gate to its maintained successor Gungraun 0.19.4;
  retained cgu=1, v6 summaries, three fresh processes, positive counts, exact
  controls and the 3% ceiling. No baseline was invented.
- Native harness dependencies are target-gated away from WASM; portable SQL/Mongo
  tests now use their crate's host-supplied browser clock. PostgreSQL SCRAM wire
  fixtures use portable SHA256 rather than requiring a native TLS provider.
  Native MongoDB test entropy uses rustls's provider instead of `/dev/urandom`.

## No-spin contract and core fix

`turnloop-contract::no_spin` opens a real registered TCP pair and leaves a read
pending. It measures twenty expiries each at **500 us, 2 ms and 10 ms**, asserts
one timer completion with the correct token/handle per expiry, no early expiry,
**at most two turns and one zero-event OS wait per expiry**, and actual wait
execution. The idle read must remain cancellable after all sixty expiries.

The new test initially failed at 500 us: cached readiness ending in EAGAIN consumed
a turn without waiting, and `Driver::turn` forced zero timeout whenever cached
work existed. The fix preserves the exact timeout, attempts cached I/O first, and
uses the one permitted OS wait when no completion or runnable work remains.
Queued completions still avoid waiting; output capacity remains bounded. Native
allocation, cancellation, liveness, fairness, timer and descriptor tests pass.

Backend contract revision 2 adds `PollInfo::zero_event_waits`, forwarded through
`TurnInfo`. The counter records **raw OS results**, including interrupted waits;
it is not inferred from the number of user completions. kqueue and epoll implement
it. Wave-2 providers must populate it when they implement the shared contracts.
Kqueue execution PASS; epoll and forced timerfd compilation PASS, runtime UNRUN.

## Verification commands and outcomes

[The complete verification command ledger](integration-commands.md) records each
invocation and exit status, including intermediate failures and deliberate failure
probes. Full local output is in `.tools/verification/`. `verify-integration.py`
passes through each command's exit status. An ignored or unbuilt test is never
counted as executed.

| Command / group | Final result |
|---|---|
| `cargo fmt --check` and `cargo fmt --all` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS; all-features variant also PASS |
| Same all-features Clippy with `--target x86_64-unknown-linux-gnu` | PASS, including no-spin contract and Gungraun benchmark compilation; execution UNRUN |
| Same with `--target wasm32-wasip2` and `wasm32-unknown-unknown` | PASS, whole workspace/all targets; WASM test execution UNRUN |
| Same with `--target x86_64-pc-windows-msvc` | FAIL in ring C compilation: `fatal error: 'assert.h' file not found`; full protocol test-target checking UNRUN without Windows SDK headers |
| `cargo clippy --workspace --lib --target x86_64-pc-windows-msvc -- -D warnings` | PASS for every library |
| Windows `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets ...` | PASS, including strict unsafe-comment lint; Windows execution UNRUN |
| `cargo +stable check --workspace` and `--all-targets --all-features` | PASS on installed 1.97.1 |
| `cargo test --workspace` | PASS; ten real-server tests ignored by default, not counted as executed |
| `python3 scripts/ci/run-tests.py native` | PASS: default and all features, with contract helper independently required to run positive tests; 25 contract tests per configuration |
| `cargo test -p turnloop-contract idle_socket_timers_do_not_spin -- --nocapture` | Initial FAIL reproduced the bug; final PASS with unchanged numerical limits |
| `python3 scripts/ci/run-tests.py loom` | PASS, all five production models |
| `env MIRI_SYSROOT=.tools/miri-sysroot cargo miri setup` then `env MIRI_SYSROOT=.tools/miri-sysroot python3 scripts/ci/run-tests.py miri` | PASS, timer and generation-table suites; initial default cache location was sandbox-denied, and relative-path handling was fixed |
| `env RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --all-features --no-deps` | PASS |
| `scripts/ci/no-tokio.sh` | PASS, eight targets × default/all features, normal/build/dev dependencies; no exceptions |
| `python3 scripts/ci/soak.py` | PASS, 193 locked registry versions, timestamps and checksums |
| `cargo deny check` with `.tools/bin` on PATH | PASS advisories, bans, licenses, sources; unavoidable transitive-major duplicates remain warnings |
| `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck cargo-deny` | PASS, official archives and committed SHA256 pins |
| `python3 scripts/ci/lint-workflows.py` with `.tools/bin` on PATH | PASS: exact queue-policy validation, all supported actionlint checks, zizmor and ShellCheck |
| `.tools/bin/actionlint -color` | FAIL only on the two `concurrency.queue` keys unsupported by v1.7.12; the inherited wrapper validates those exact expressions before filtering only that parser diagnostic |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS, 15 tests (13 gate tests plus failed-start cleanup and Docker-wrapper ownership/path tests) |
| `python3 -m py_compile scripts/test-servers.py` | PASS |
| Standalone spike Clippy: IOCP Windows, p2 WASI, p3 WASI using nightly-2026-09-07, web with all features | PASS after rename; all spike runtime tests in this session UNRUN |
| `cargo publish --dry-run --allow-dirty -p turnloop` and `--locked` for each protocol crate | PASS for all six, including packaged-library recompilation; upload explicitly aborted by dry run |
| `python3 scripts/ci/release.py order` | PASS, six packages, helpers excluded |
| `python3 scripts/ci/instructions.py` | FAIL precondition: requires Ubuntu 24.04 x86_64/Valgrind; measurements and regression comparison UNRUN |
| `git diff --check` | PASS after fixing whitespace |

Additional read-only verification: `rustc +stable --version`; `cargo info` and
installed manifests for the benchmark/encoding migration; upstream release API,
changelog, runner v6 schema and environment-name inspection; SHA256 verification
of the downloaded official Linux Gungraun 0.19.4 archive
(`f88194dac725ef0599b2812396518e9bd74aadaf75f5baf78df91ddb9eec67d3`); crates.io API
checks for all six names (all unpublished); `rg` rename/dependency/env-var checks;
workspace/package inventory and Git status/diff inspection. All PASS unless a
specific failure is listed below. Initial `.tools` and two guessed source paths
did not exist; subsequent reads used discovered paths. `DESIGN.md` and `LANES.md`
were read completely, and all lane reports/manifests were inspected for handoff
requirements and outstanding work.

### Real-server commands

| Command | Result |
|---|---|
| `scripts/test-servers.py run cargo test --workspace -- --include-ignored` | Command FAIL at PostgreSQL initdb; **all test bodies UNRUN (sandbox)** for this full invocation |
| `scripts/test-servers.py --services postgres run true` | Initialization FAIL: `FATAL: could not create shared memory segment: Operation not permitted`, failed `shmget`; PostgreSQL tests **UNRUN (sandbox)** |
| `scripts/test-servers.py --services mysql run true` | Initialization FAIL, exit 2 with fatal-signal stack in `memory::Aligned_atomic`, `Shared_spin_lock`, `delegates_init`; MySQL tests **UNRUN (sandbox)** |
| `scripts/test-servers.py --services redis,mongodb,smtp run cargo test -p turnloop-redis -p turnloop-mongodb -p turnloop-smtp -- --include-ignored --test-threads=1` | PASS twice, including final shared crypto/base64/PEM/supervisor changes. Four external-service tests execute, alongside the ordinary protocol suites |
| `scripts/test-servers.py --services smtp start` then `scripts/test-servers.py stop` | PASS, independent processes, private supervisor shutdown |
| `scripts/test-servers.py --services mongodb start` then `scripts/test-servers.py stop` | PASS when serialized; all five recorded private ports closed |
| `scripts/test-servers.py --services redis run false` | Expected exit 1 from the deliberate failing child; private cluster was started and cleaned up, state files absent afterward |
| CI `scripts/test-servers.py --ci-services run python3 scripts/ci/run-tests.py protocol` | **UNRUN**: no Docker/Linux host; CI retains this required command |

Earlier failed fixture probes are preserved in the ledger: cleanup initially
assumed an SMTP PID existed after SQL initialization failed; fixed and covered by
an adversarial test. A lifecycle probe was inadvertently started while another
fixture run still owned the state: start correctly refused, and the separately
issued cleanup timed out after the owner had already stopped MongoDB. Sequential
MongoDB start/stop was then verified. Lifecycle commands must be serialized within
a checkout. Final inspection found no private server state or SMTP control socket.

One loaded native CI-wrapper run failed the existing timer-precision median gate:
`systematic timer floor: median lateness 672.208 us` (required <500 us), while
compilation/server work was also active. The unchanged complete default/all-feature
wrapper passed after those operations finished. No timing threshold, sample count,
assertion or test was weakened; scheduler sensitivity remains a practical risk.

Other corrected intermediate failures: inherited safety-comment placement and
Clippy style diagnostics under the unified edition/MSRV; RustCrypto API changes
(`KeyInit`, MD5 formatting); an incorrect temporary assumption about PBKDF2's
return type; portable test clock/TLS imports; the mock Docker test's macOS
`/var` versus `/private/var` path alias; one trailing-whitespace edit and a final SQL-constant formatting syntax error
(caught by the fixture tests, restored, and all 15 automation tests rerun PASS). The ledger
keeps their original FAIL entries and later PASS commands.

## Dependencies chosen

All workspace consumers inherit one definition per shared dependency. The final
lock has one rustls, sha1, sha2, hmac, md-5, pbkdf2, flate2 and base64 version.

| Dependency | Locked version | Reason |
|---|---|---|
| libc | 0.2.175 | Retain core's reviewed Unix FFI baseline |
| bytes | 1.12.1 | Shared retained wire buffers |
| postgres-protocol | 0.6.12 | Lane's pinned sans-IO codec/auth implementation |
| mysql_common | 0.38.2 | Lane's pinned codec/auth, default features off |
| bson | 3.1.0 | Lane's pinned raw/serde codec, default features off |
| rustls | 0.23.45 | Shared TLS with ring/std/tls12; main's dated RUSTSEC-2026-0285 exception |
| base64 | 0.22.1 | Shared by DB codecs and SMTP/Mongo authentication |
| email-encoding | 0.4.1 | Explicit compatible lettre helper pin keeps base64 unified; 0.4.2 introduces base64 0.23 |
| lettre | 0.11.23 | Builder-only MIME implementation; no async transport |
| sha1 / sha2 / md-5 | 0.11.0 each | Unified with upstream DB codecs; real Mongo auth and vectors pass |
| hmac / pbkdf2 | 0.13.0 each | Same RustCrypto generation as the shared digests |
| stringprep | 0.1.5 | SCRAM SASLprep |
| flate2 | 1.1.10 | Pure-Rust compression with retained buffers |
| serde_json | 1.0.151 | Protocol JSON fixtures |
| getrandom | Direct 0.4.3 | Browser entropy feature unification for SQL/BSON upstream APIs |
| loom | 0.7.2 | Existing production model instrumentation |
| gungraun | 0.19.4 | Maintained iai-callgrind successor; Linux-only benchmark dev dependency |

`getrandom` 0.2/0.3/0.4, rand generations and proc-macro syn generations remain
transitively necessary because ring/BSON/DB codecs/macros require incompatible
major versions. They are not independently selected duplicate workspace APIs.
No registry forks, dependency patches, soak override or runtime-ban exception was
introduced. Newer bitflags, cc, crc32fast, hybrid-array, smallvec, tinyvec, uuid and
zerocopy releases were explicitly excluded by the seven-day resolver.

The first audit rejected unmaintained `rustls-pemfile`
([RUSTSEC-2025-0134](https://rustsec.org/advisories/RUSTSEC-2025-0134)); it was removed
in favor of rustls-pki-types' maintained PEM API, through rustls's re-export.
Adding the originally prescribed iai-callgrind 0.16.1 exposed unmaintained
`proc-macro-error2` ([RUSTSEC-2026-0173](https://rustsec.org/advisories/RUSTSEC-2026-0173)).
The [upstream 0.19.4 changelog](https://github.com/gungraun/gungraun/blob/v0.19.4/CHANGELOG.md)
records the maintained replacement. The gate, installer checksum and benchmark
pin were migrated together. The audited permissive **0BSD** license was added for
lettre's quoted_printable dependency; no advisory was ignored.

## Publish order and bootstrap

All packages currently use **0.1.0**. The metadata-derived order is:

1. `turnloop`
2. `turnloop-tls`
3. `turnloop-zstd-decoder`
4. `turnloop-http`
5. `turnloop-mongodb`
6. `turnloop-mysql`
7. `turnloop-postgres`
8. `turnloop-redis`
9. `turnloop-smtp`
10. `turnloop-websocket`

This is the current metadata-derived order (`python3 scripts/ci/release.py order`).
TLS and the decoder precede HTTP; HTTP precedes WebSocket. The independent database
and SMTP crates may otherwise be reordered. `turnloop-contract` and `turnloop-bench`
remain private. Every package is version 0.1.0.

`cargo publish --dry-run --locked --allow-dirty --workspace` stages unpublished
siblings together, packages each crate and recompiles each packaged library.
The all-feature WASI dry run additionally verifies the registry dependency on the
portable decoder. Neither run used `--no-verify`, and all uploads were explicitly
aborted. Packaged decoder tests also execute independently of the workspace.
The decoder's full upstream fixtures produce an approximately 9.6-MiB archive,
within the [registry’s default 10-MiB archive limit](https://github.com/rust-lang/crates.io/blob/main/src/config/server.rs); preserve the corpus tests.

First publications, Trusted Publisher bootstrap, semver comparison against an
existing release, tags/releases and OIDC exchange remain owner actions from
RELEASING.md. The existing platform/runtime gates
must pass before publication; dry-run success does not establish release
readiness or claim that any package was uploaded.

## Pending CI gates and exact follow-ups

| Gate | Follow-up; current status |
|---|---|
| Windows native contracts | Wave 2: implement production IOCP Backend and instantiate the unchanged shared tests, including no-spin. Run `python3 scripts/ci/run-tests.py native` on Windows. Required job is retained; runtime **UNRUN** |
| WASI 0.2/0.3 contracts | Integrate providers and nonzero shared suites; solve persistent p3 bounded waitable-set stepping. Run `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` and the p3 equivalent. Required jobs are explicitly pending; shared execution **UNRUN** |
| WASI 0.3 workspace dependencies | Whole-workspace p3 Clippy **FAIL**: BSON's ahash/rand dependencies bring getrandom 0.3.4, which rejects the target. Resolve with a reviewed compatible dependency/provider and unchanged soak/allocation gates; no cfg exclusion or unsupported entropy fallback was added |
| Browser/Node contracts | Add actual wasm-bindgen-test targets plus `web-tests`/`node-tests` metadata. Run `python3 scripts/ci/run-tests.py web` (Chrome AND Firefox) and `node`. Required job retained; shared execution **UNRUN** |
| Instruction regression | Main committed `crates/turnloop-bench/benchmarks/instructions.json` with four measured cases and an exact control. Run `python3 scripts/ci/instructions.py` on Ubuntu 24.04 x86_64 with pinned Gungraun/Valgrind. Merged-tree runtime comparison is **UNRUN** locally; no baseline was synthesized or changed here |
| SQL/full protocol services | Run the full local command outside the sandbox, and run the Docker CI job. SQL bodies, independent CI TLS probes and Docker cleanup are **UNRUN** here |
| Windows full cross-test lint | Supply real Windows SDK headers for ring or use the native Windows runner; no test-target cfg was removed to hide this limitation |
| Raw actionlint queue support | Upgrade to a checksum-pinned official release that understands `concurrency.queue`, then remove only the existing compatibility filter. Strict queue validation remains active |
| ci-gate/release workflow | Run on GitHub after production providers and p3 dependency compatibility are resolved; compare the merged code with the committed instruction baseline. The fan-in rejects failures, cancellations and every skip except the already documented optional self-hosted Windows job. No new skip exemptions |

A real GitHub run, Linux x86_64/arm64, Windows, FreeBSD/mobile/Android runtime
validation, Wasmtime/browser/Node shared contracts, instruction measurements,
long soak/fault campaigns and Perry migration/A-B measurements are all **UNRUN**.
No standalone spike or cross-compilation result substitutes for those tests.

## Remaining lane questions and spec clarifications

- **Core:** preserve explicit-release pooled leases; asynchronous detach may return
  WouldBlock until cancellation is delivered; external integration keeps the
  notifier logically parked. First shared-pool configuration wins. Host async
  jobs/DNS on WASM, processes/signals/TTY/pipes/files APIs, executor and Perry ABI
  adapters are later work. Confirm these API clarifications and collect Perry
  allocation/instruction/fault/soak evidence. Revision-2 wait counters must be
  wired into every new backend.
- **Windows:** decide APC callbacks versus feature-detected NT timer packets and
  minimum supported Windows; IOCP association prevents naive detach/reassociate;
  define socket/named-pipe transfer and fatal teardown policy. Parent child-stdio
  ends should be overlapped, child ends synchronous. Deferred CTRL_CLOSE cleanup
  is best effort. All 18 spike runtime tests, ETW/cycle/allocation/leak and precision
  distributions remain UNRUN; replace draft linear lookup/mutex queues at integration.
- **WASM:** allocation-free p2 storage remains unresolved; p3 convenience async
  collection is not bounded stepping. Browser host allocation accounting,
  cancellation lifetime and worker scheduling need explicit contracts. The lane
  proposes distinguishing nanosecond deadline representation from runtime wake
  precision, and documenting WASI terminal detection rather than terminal size.
  Inline blocking jobs would violate D1/D7; use Unsupported or host async requests.
- **SQL:** run all six real-server bodies outside the sandbox. MySQL 9 cannot test
  native-password auth; add a MySQL 8.4 instance with that plugin for real-server
  coverage (wire coverage exists). Pin Node/pg/mysql2 compatibility versions and
  finish JS conversions, charset/date/error mappings, prepare caching and adapters.
  Preserve borrowed-buffer and terminal parser-error/abort ownership rules.
- **Redis/SMTP:** choose ioredis version/retry policy; exactly-once completion does
  not imply exactly-once replayed Redis execution. Streaming RESP3/MIME, host result
  conversions, full pooling/topology policy and sustained failover remain open.
- **MongoDB:** agree on shared time/token/transport interfaces; complete the
  Node/Perry facade, full official CRUD/transaction/change-stream suites and
  backpressure-v2. Selected SDAM/selection fixtures and real-server tests do not
  establish full Node driver parity or allocation freedom for the entire facade.
- **CI/release:** FreeBSD nightly, mobile runners, long-duration native churn,
  exact-SHA hosted release/OIDC behavior and owner setup still need execution or
  definitions. No release baseline or first upload exists.
- **HTTP/TLS/WebSocket:** integration is implemented above. Maintain the rustls
  exception lifecycle, run native Windows/Linux CI and browser runtime coverage,
  then supply the executor/Perry adapters. permessage-deflate and detailed Node
  error/option parity remain documented protocol-lane gaps.

No DESIGN.md rule was relaxed. The only implementation-driven contract addition
is the observable empty-wait counter; the fixture convention and maintained
benchmark-tool migration are documented integration decisions. Outstanding lane
spec proposals above are recorded for review, not silently adopted.
