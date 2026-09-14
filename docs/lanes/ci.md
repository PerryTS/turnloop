# CI lane report — draft 0.3 handoff

Status: CI/release automation is implemented and locally checked. **The integrated
repository is not yet proven green.** Missing core/contract/bench interfaces and
protocol harness migrations below are enforced failures, not skipped successes.

Read the authoritative `/Users/amlug/projects/perry/windlass/DESIGN.md` draft 0.3
completely, and local `LANES.md` completely. The user supersedes its old private
repository note: **PerryTS/turnloop is public, main is default, full PR matrix,
zero tokio without exceptions**. Reused the stopped run's installers and helpers;
removed the old h2/tokio exception machinery entirely.

Only this CI clone was changed. Other clones were read-only. Builds ran against a
copy at `.tools/fixtures/core`, with their output confined to this clone/Cargo and
Rustup caches. No pushes, remote changes, repository creation, crates.io uploads,
yanks or release creation were performed. Cargo's dry run explicitly printed
`aborting upload due to dry run`. No commit commands were attempted: `.git` is
read-only. Integrator checkpoints appeared during work, including `39ec213`;
these were external. The integrator must commit any final report/workflow changes.

## Implemented

- `.github/workflows/ci.yml`: every PR and main push; PR cancellation only;
  `queue: max` on main. Native lint/test matrices on Ubuntu x86_64/aarch64,
  macOS arm64 and Windows x86_64; optional owner-enabled `turnloop-windows` job.
  Default/all-feature native checks, denied undocumented unsafe blocks,
  rustdoc warnings denied, stable check, both WASM cross-lints, WASI 0.2/0.3
  contracts with their exact pins, Chrome+Firefox and Node test entry points.
- Linux PostgreSQL 16, MySQL 9, Redis 8 and Mongo 8 service containers; MySQL
  caching_sha2 account requires TLS with a generated CA and hostname-verified
  probe; PostgreSQL SCRAM; six-node Redis cluster and three-member Mongo replica
  set bootstrap/health/read-write assertions. Cleanup owns only extra CI containers.
- Metadata-driven package selection, explicit test target/filter metadata, and
  positive passed-test counts. Default and all-feature contracts run separately
  with one test thread. Both browser result groups must report positive execution.
  Spikes cannot become workspace/release members. No hard-coded crate list.
- `no-tokio.sh`: all eight policy triples, default/all features, normal/build/dev
  edges. Bans tokio, tokio-util, hyper, h2, async-std, smol, async-io, async-executor.
  `deny.toml`: equivalent bans plus advisories/licenses/sources and wildcard checks.
- `soak.py`: keeps pinned-nightly resolution under the existing seven-day policy,
  rejects environment overrides, checks every locked registry timestamp and checksum.
  A young lockfile cannot evade the resolver soak.
- Required loom and marked-core Miri jobs; deterministic iai-callgrind gate:
  pinned 0.16.1 runner/schema, cgu=1, three fresh processes, positive Ir counts,
  exact benchmark-set coverage, >3% regression failure, exactly unchanged control.
  Missing baselines/bench targets fail; no synthetic baseline was invented.
- `ci-gate`: needs every other job, explicitly inspects all result values, allows
  only the disabled optional Windows skip. Tests ensure the fan-in lists every job.
- `release.yml`: successful same-repository main-push CI trigger; exact SHA,
  workflow, event, latest attempt and ci-gate API verification. Release-plz authors
  version/changelog PRs with a short-lived GitHub App token so they trigger CI.
  Only a merged release PR can publish, behind `crates-io` environment review.
  All-crates dry-run, semver checks, soak and no-tokio precede OIDC. Exact-SHA API
  check is immediately before OIDC. Native Cargo batch publisher stages siblings
  and uploads in dependency order; verified archives precede crate tags/releases.
  Same-commit partial-upload recovery is checked against registry SHA-256/VCS info.
- `RELEASING.md`: owner environment/App/Trusted Publisher setup, manual first
  publication commands using metadata order, bootstrap token revocation, 0.x policy,
  Perry one-command version override plus archive/lock/source verification,
  recovery and yank/rollback commands. `verify-crate.py` implements the verifier.
- `CONTRIBUTING.md`: local commands, test metadata, canonical `TURNLOOP_TEST_*`
  environment, service topology, baseline schema/threshold rationale, tool updates.
- Dependabot Cargo/Actions weekly updates with seven-day cooldowns. All Actions
  use full official commit SHAs with version comments; tool archives have committed
  SHA-256 pins. Every job declares minimal permissions; no shared caches/artifacts
  feed the privileged release workflow.

## Verification commands and results

All relative paths below are from this clone. Define:

```text
F=.tools/fixtures/core/Cargo.toml
N=nightly-2026-08-20
P=nightly-2026-09-07
```

The fixture is a read-only snapshot copied from core before these checks; it is
not the final merged workspace. These are command abbreviations, not env overrides
used by the scripts. All Cargo invocations emitted by scripts are printed in logs.
Local full logs remain in ignored `.tools/`; the outcomes and relevant errors are
recorded here so this report does not depend on those logs being committed.

### CI scripts and workflow lint

| Command / probe | Result |
|---|---|
| `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck release-plz cargo-semver-checks cargo-deny wasm-pack` | **PASS**, downloaded official macOS arm64 binaries into `.tools/bin`, checked each committed SHA-256 before extraction |
| `bash scripts/ci/install-wasmtime.sh` | **PASS**, official Wasmtime 46.0.0 arm64 archive verified (`ab4bdab6…bdcd25`) |
| Python API verification of every `scripts/ci/tools.json` URL against its official release asset `digest` | **PASS**, all 17 macOS/Linux pins, including Linux-only iai-callgrind runner |
| Python GitHub commit-API verification of checkout v7.0.1, setup-python v7.0.0, setup-node v7.0.0, create-github-app-token v3.2.0, crates-io-auth-action v1.0.5 | **PASS**, all five full SHA pins match the official tags |
| `.tools/bin/actionlint -version`, `.tools/bin/zizmor --version`, tool/release/Cargo `--help` probes | **PASS**, actionlint 1.7.12, zizmor 1.30.1; verified publish workspace flags, semver baseline flags, wasm-pack combined browser flags, release-plz CLI and cargo-deny config argument placement |
| `.tools/bin/actionlint -color` (raw) | **FAIL**, latest official 1.7.12 rejects `queue` in both concurrency blocks; exact error below |
| `.tools/bin/zizmor --offline --min-severity low .github/workflows` (initial) | **FAIL**, generic `dangerous-triggers` on workflow_run; guarded-trigger rationale is now annotated at `on:` |
| `PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` (final) | **PASS**, separate strict queue validation, all remaining actionlint diagnostics, zizmor zero findings, ShellCheck clean |
| `.tools/bin/shellcheck scripts/ci/*.sh` | **PASS**, all seven shell scripts |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | **PASS**, 13 adversarial tests; see coverage below |
| `python3 -m compileall -q scripts/ci` | **PASS**, all Python scripts parse |
| `git diff --check` | **PASS**, no whitespace errors |
| `python3 scripts/ci/{check-ci,install-tools,instructions,no_tokio,release,run-tests,soak,verify-crate}.py --help` (each independently) | **PASS**; help-only for mutating or unavailable external operations |
| `bash scripts/ci/{bootstrap-services,bootstrap-redis-cluster,bootstrap-mongo-replica,cleanup-services,no-tokio}.sh --help` (each independently) | **PASS**, command interfaces; does not claim Docker execution |
| `NEEDS_JSON='{"lint":{"result":"success"},"self-hosted-windows":{"result":"skipped"}}' SELF_HOSTED_WINDOWS=false python3 scripts/ci/ci-gate.py` | **PASS**, inspected two mock job results; live fan-in remains UNRUN |

Adversarial tests prove rejection of every banned crate, malformed/empty graphs,
renamed publish dependencies/cycles, spike members, young locked versions, checksum
mismatch, missing/failed/cancelled/skipped jobs, wrong CI SHA/event/workflow, missing
CI gate, zero executed tests, stale/empty instruction summaries, changed controls,
missing benchmarks and >3% instruction growth. They also check complete fan-in,
full Action SHA pins, null Cargo package metadata, queue-policy weakening and
installer refusal to extract a digest mismatch.

Two helper bugs found during local execution were fixed before the final passes:
`cargo metadata` emits `metadata: null`, not `{}`; `settings()` now handles null.
The subprocess stdout pipe was initially left open (ResourceWarning); it is now
closed, and tests pass with Python warnings treated as errors. Initial native
wrapper run failed with `AttributeError: 'NoneType' object has no attribute 'get'`;
the final native run below passed without changing any Rust test.

### Dependency and Rust checks

| Command | Result |
|---|---|
| `bash scripts/ci/no-tokio.sh --manifest-path ../core/Cargo.toml` | **PASS**, all 8 targets × default/all features (16 trees), 6–9 package rows per tree |
| Same command with `../proto-sql/Cargo.toml`, `../proto-kv/Cargo.toml`, `../proto-mongo/Cargo.toml`, `../proto-http/Cargo.toml` (each independently) | **PASS**, all 64 additional trees; 107–181 rows. Other clones were not written |
| `python3 scripts/ci/soak.py --manifest-path "$F"` | **PASS**, 30 locked registry versions checked against index checksums/timestamps; pin and 7-day config retained |
| `python3 scripts/ci/run-tests.py native --manifest-path "$F"` | **PASS**, default and all features: 24 workspace tests + 21 contract tests independently per configuration, including allocation/lifetime binaries |
| `python3 scripts/ci/run-tests.py loom --manifest-path "$F"` | **PASS**, all 5 production models with `RUSTFLAGS=--cfg loom`, `--lib models -- --test-threads=1` |
| `cargo +$N fmt --manifest-path "$F" --all --check` | **PASS** |
| `cargo +stable check --manifest-path "$F" --locked --workspace --all-targets --all-features` | **PASS**; installed stable is 1.97.1, not the advertised 1.98 (`cargo +stable --version` confirms) |
| `cargo +$N clippy --manifest-path "$F" --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | **FAIL**, core `backend/socket.rs:65` missing preceding safety comment |
| Same Clippy command with `--target x86_64-unknown-linux-gnu` before `--` | **FAIL**, same core safety-comment error |
| Same Clippy command with `--target x86_64-pc-windows-msvc`, `wasm32-wasip2`, `wasm32-unknown-unknown` (each independently) | **PASS**, compilation only; copied core does not contain production Windows/WASM backend integration |
| `RUSTDOCFLAGS='-D warnings' cargo +$N doc --manifest-path "$F" --locked --workspace --all-features --no-deps` | **PASS** |
| `PATH="$PWD/.tools/bin:$PATH" cargo +$N deny --manifest-path "$F" --locked check --config deny.toml` (initial) | **FAIL**, CLI error: `--config` must precede `check`; workflows use the default root config correctly |
| `PATH="$PWD/.tools/bin:$PATH" cargo +$N deny --manifest-path "$F" --config deny.toml --locked check` | **FAIL**, unversioned workspace path dependencies are wildcards; advisories/licenses/sources PASS, bans FAIL. Also prints cargo-deny's `unresolved-workspace-dependency` diagnostics for helper paths |
| `python3 scripts/ci/release.py order --manifest-path "$F"` | **PASS**, discovers only publishable `windlass 0.1.0`; helpers excluded |
| `cargo +$N publish --manifest-path "$F" --registry crates-io --locked --workspace --dry-run` | **PASS**, 22 files packaged, library rebuilt; `aborting upload due to dry run`. Warns missing repository/homepage/docs metadata. This snapshot has one publishable crate, not the final sibling release graph |
| In disposable `.tools/fixtures/release`: `cargo +$N generate-lockfile`, `cargo +$N metadata --format-version 1 --no-deps --locked`, then `cargo +$N publish --registry crates-io --locked --dry-run -p turnloop-ci-preflight-foundation-fixture -p turnloop-ci-preflight-client-fixture` (package arguments discovered from metadata) | **PASS**, both unpublished fixture siblings packaged and rebuilt through Cargo's temporary registry with the unchanged seven-day config; both uploads aborted by dry run. Proves batch preflight mechanics locally, not real publication |
| `python3 scripts/ci/verify-crate.py --package libc --version 0.2.175 --source-commit 84e26e6b166a6634d679fbf44e957102846b8a03 --lockfile .tools/fixtures/core/Cargo.lock` | **PASS**, registry/download/lock/source all match; published 2025-08-11, SHA-256 `6a82ae493e598baaea5209805c49bbf2ea7de956d50d7da0da1164f9c6d28543` |

### WASM and missing interfaces

| Command | Result |
|---|---|
| `bash scripts/ci/wasmtime-runner.sh ../wasm/spikes/wasi-p3/target/wasm32-wasip3/release/windlass_wasi_p3_spike.wasm` | **PASS**, runner smoke only: 257 TCP bytes each direction, 8 short + 1 long timer completions. Read existing binary; no writes to WASM lane |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2 --manifest-path "$F"` | **FAIL**, binaries ran under Wasmtime but all report `0 passed`; wrapper rejects zero-test success |
| Same with `--target wasm32-wasip3` | **FAIL**, same zero-test error on nightly-2026-09-07; p3 binaries successfully launch under pinned Wasmtime |
| `python3 scripts/ci/run-tests.py miri --manifest-path "$F"` | **FAIL**, `Core must mark pure-Rust tests with package.metadata.turnloop-ci.miri-filters`; actual Miri execution **UNRUN** |
| Same with `web` and `node` (each independently) | **FAIL**, contract member lacks `web-tests`/`node-tests` metadata; browser/Node contract execution **UNRUN** |
| Same with `protocol` | **FAIL**, core snapshot has no protocol member; merged real-server execution **UNRUN** |
| `python3 scripts/ci/instructions.py --manifest-path "$F"` | **FAIL**, `windlass-bench: add an iai-callgrind =0.16.1 benchmark target`; Linux Valgrind benchmark execution **UNRUN**, no Linux host or real baseline |

Read-only source inspection included all lane reports, actual manifests and
harnesses, GitHub concurrency/Dependabot docs, pinned release-plz publication
source, Cargo workspace publication source, OIDC action.yml, and iai-callgrind
v0.16.1's summary v6 schema. Several initial upstream raw-source URLs returned
404 before the actual repository paths were located through GitHub's tree API;
no code or pins were inferred from those failures. Perry's perex source commit
could not be read at `/Users/amlug/projects/perry` (that directory is not a Git
checkout); the consumption procedure follows the authoritative DESIGN.md §13
and Cargo's documented one-command override, with an executed checksum verifier.

## UNRUN: needs a real host/GitHub repository

- Every actual GitHub workflow run, required-check behavior, queue behavior and
  its expression support, runner labels, App-created release PR CI triggering,
  exact-SHA live API verification, environment reviewer protection, Trusted
  Publisher exchange/revocation, release uploads, dependency-order partial recovery,
  tag/GitHub Release creation, and branch protection.
- Native runtime tests on Linux x86_64/aarch64, Windows hosted/self-hosted; no
  Linux/Windows hosts or Docker here. Cross-Clippy PASS is not runtime proof.
- All Docker bootstrap and cleanup scripts beyond `--help`/ShellCheck, including
  TLS/auth queries, Redis routing, Mongo election/majority writes and service health.
- Browser/Node shared contracts, real Miri, Linux iai-callgrind, final merged
  workspace semver checks and multi-crate publish dry run. Tools alone being
  installed does not prove those suites ran.
- Bootstrap first publications, owner environment/App settings, token revocation,
  Perry actual turnloop bump and rollback/yank procedures. No turnloop release
  version is claimed to exist.

## Deviations / proposed clarifications

1. **actionlint compatibility:** latest official 1.7.12 does not recognize GitHub's
   May 2026 `concurrency.queue`. Raw error:
   `unexpected key "queue" for "concurrency" section. expected one of "cancel-in-progress", "group"`.
   The wrapper strictly validates the required queue/cancel expressions and rejects
   unvalidated queue occurrences before filtering only that exact parser message.
   All other checks remain mandatory. Remove the filter once a released binary
   supports the field. Raw actionlint is explicitly FAIL, not misreported clean.
2. **GitHub limit:** `queue:max` prevents replacement of pending main runs only up
   to GitHub's 100-pending limit; beyond it GitHub cancels new runs. Absolute infinite
   queueing is not available in YAML. Documented monitoring/rerun requirement.
3. **Release implementation:** release-plz authors PRs; native Cargo batch upload
   plus `gh release create` handles publication instead of `release-plz release`.
   This keeps all-crates preflight and unpublished sibling staging under the soak.
   If a partial retry encounters an already-published sibling younger than seven
   days, it must wait rather than disable the soak. Final registry behavior needs
   real multi-crate release validation; no upload experiment was authorized here.
4. **Interfaces to other lanes:** explicit Cargo package metadata marks pure Miri,
   browser/Node, protocol integration and baseline capabilities. Empty interfaces
   fail instead of substituting standalone spikes or guessed test filters.
5. **Protocol convention:** one `TURNLOOP_TEST_*` contract is documented. Current
   SQL uses `TURNLOOP_PG_PORT`, `TURNLOOP_MYSQL_PORT`, `TURNLOOP_SQL_TOOLS` and
   hard-coded `fixture-password`/fixture users; KV uses `REDIS_PORT`,
   `REDIS_PASSWORD`, ACL user `lane` and private TLS/Sentinel fixtures. These are
   not falsely claimed to consume our URLs. Their lanes/integrator must adapt the
   harnesses and any additional auth/TLS/Sentinel fixture requirements.
6. The specific requested PR/main matrix is implemented. DESIGN.md's later
   FreeBSD nightly and long-running native churn/fault campaigns need additional
   host/suite definitions; they are not claimed by this first workflow.

## Open integrator actions and next steps

1. Merge the Cargo workspace and backends, rename crates, retain committed lockfile;
   add explicit versions to workspace path dependencies and fix socket.rs safety
   comment placement. Re-run strict Clippy and cargo-deny without weakening either.
2. Supply actual shared WASI contracts and browser/Node test targets; mark the
   metadata described in CONTRIBUTING. The current core snapshot's zero WASI
   tests must become nonzero real backend coverage before ci-gate can pass.
3. Mark core Miri-compatible tests. Add iai-callgrind 0.16.1 benchmarks and obtain
   real Ubuntu 24.04 Valgrind candidates, assert subjects, review and commit counts
   and exact controls. No Linux counts exist in this CI clone.
4. Merge all protocol crates and migrate their test harnesses to the canonical
   URLs/TLS settings with `TURNLOOP_TEST_REQUIRED=1`. Declare integration targets;
   preserve additional lane tests and provide missing fixtures instead of ignoring
   them. Current SQL/KV native fixture conventions differ materially.
5. Push a branch and run the full workflow. Check actual queue syntax, official
   runner availability, service health/TLS, both browsers, OIDC and main API gates.
   First merged green run is the evidence needed to enable required `ci-gate`.
6. Perform owner bootstrap from RELEASING.md: protected main, required reviewer
   environment, scoped GitHub App, manual first publish per name, Trusted Publisher
   entries, token revocation, initial tags/changelogs. Exercise multi-crate dry-run
   and semver gates before approving the first automated release.
7. Commit this report and remaining working-tree changes. No publishing action
   was performed by this lane.
