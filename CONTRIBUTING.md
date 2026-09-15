# Contributing

`DESIGN.md` draft 0.3 is the specification. The repository is public and its full
required CI matrix runs on **every PR**, including documentation and automation
changes. The single branch-protection check is **`ci-gate`**. Run tests on the
platform where their subject executes; a cross-target check is not a test pass.

## Local checks

Use the pinned nightly (`nightly-2026-08-20`) to resolve dependencies with the
seven-day minimum publish age. Keep `Cargo.lock` committed. Stable Rust 1.97.1 (the verified MSRV) is also
required, with no nightly language features in the library. WASI 0.3 alone uses
`nightly-2026-09-07` and Wasmtime 46.0.0, based on the WASM lane's successful spike.

```bash
python3 scripts/ci/check-paths.py
python3 scripts/ci/feature_modes.py
cargo +nightly-2026-08-20 fmt --all --check
cargo +nightly-2026-08-20 clippy --locked --workspace --all-targets --all-features -- \
  -D warnings -D clippy::undocumented_unsafe_blocks
RUSTDOCFLAGS='-D warnings' cargo +nightly-2026-08-20 doc --locked --workspace --all-features --no-deps
cargo +stable check --locked --workspace --all-targets --all-features
python3 scripts/ci/run-tests.py native
python3 scripts/ci/soak.py
bash scripts/ci/no-tokio.sh
python3 scripts/ci/install-tools.py cargo-deny actionlint zizmor shellcheck
export PATH="$PWD/.tools/bin:$PATH"
cargo +nightly-2026-08-20 deny --locked check
python3 scripts/ci/lint-workflows.py
python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v
```

`check-paths.py` reads Git's index for exact path spellings and the working tree
for file references, so it also catches unstaged source edits on case-insensitive
disks. It rejects file/directory case collisions, Rust includes and module paths
(including disabled cfg branches and inline modules), and Cargo readme/license-file
paths. It understands literal/raw strings and literal `concat!` with
`env!("CARGO_MANIFEST_DIR")`; other computed include paths fail for manual resolution.
Comments and string contents are not scanned as Rust code. Stage new referenced
files with their exact case before the final check. Run its synthetic Git regression
with `python3 -m unittest discover -s scripts/ci -p test_paths.py -v`.

Every unsafe block must explain its safety with `// SAFETY:` and crate roots deny
`unsafe_op_in_unsafe_fn`. No `unwrap()` on I/O paths. Assert actual completions,
bytes or work counters; success with zero tests is an error. Contracts run with
`--test-threads=1` for global signal/process and allocator isolation. Native CI
selects each mode in `.github/workflows/ci.yml` independently: Linux x86_64 and
arm64 run default, `epoll-timerfd`, `process-sigchld`, both fallbacks (`fallbacks`),
`executor`, and `all-features`. macOS and Windows run default, executor, and all
features. Each arm runs the workspace, independently requires positive counts
for each core/protocol/contract member, and runs native Node/curl interop with the
same mode. Independent member runs retain the selected features belonging to that
member or its direct dependencies; a sans-IO crate with no core dependency has
no backend feature to select. The workspace run keeps the full feature selection.
Every arm independently requires positive contract counts, including Windows
IOCP. Every arm is required through the matrix job's `ci-gate` result.

`python3 scripts/ci/run-tests.py native` runs all applicable modes on the current
host; `--mode epoll-timerfd` selects just that Linux arm. `interop --mode MODE`
uses the same selection. The native matrix's JSON flow rows are a YAML subset
parsed directly by `feature_modes.py` and the runner. The required feature gate
compares those rows to `crates/turnloop/Cargo.toml`: every public core feature
must be explicitly named in a runtime arm. `all-features` does not grant implicit
coverage to newly added features. Missing fallback combinations, disconnected
matrix commands and optional/skipped mode configuration fail the gate.

The allocating BTree timer comparison lives only in `turnloop-bench`, selected by
that private crate's `timer-btree` feature. It cannot affect the core through Cargo
feature unification. Both benchmark implementations retain workload assertions
and run in native CI; production always uses the preallocated 4-ary heap.

`tokio`, `tokio-util`, `hyper`, `h2`, `async-std`, `smol`, `async-io` and
`async-executor` are forbidden in normal, build and dev dependency graphs. There
are no trait-only or HTTP exceptions. The audit resolves every target in
`scripts/ci/policy.toml` with both default and all features. `--manifest-path` can
inspect another checkout read-only; use a copied checkout for building/tests.
Cargo deny also checks the union of target graphs, licenses, advisories and
sources. Tools installed in `.tools/` are CI executables, not crate dependencies.

Dependency updates use Dependabot with a seven-day cooldown for Cargo and Actions.
The independent `soak.py` check examines **already locked** registry versions and
checks their index timestamps/checksums, because the resolver alone permits locked
young versions. A too-young version fails with its publish and eligible timestamps;
choose an older version. No soak override is accepted in CI.

### Security fixes younger than the soak window

For a reviewed security fix, add a `[[security-exceptions]]` entry to
`scripts/ci/policy.toml` with `crate`, one exact `version`, a `RUSTSEC-YYYY-NNNN`
`advisory`, a nonempty `reason`, and `expires` equal to the registry publish date
plus seven days (UTC). See the rustls 0.23.45 entry for RUSTSEC-2026-0285.
The gate still verifies the registry checksum and all other versions' ages,
prints every active exception, and fails if an entry is expired, unused, or has
the wrong expiry. Remove the entry when its exact seven-day timestamp is reached,
or immediately if that crate/version leaves the lockfile.

Scope the resolver override to the one precise update command. This environment
form was verified on the pinned nightly; do not export it or edit `.cargo/config.toml`:

```bash
env CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE='0 days' \
  cargo +nightly-2026-08-20 update -p rustls --precise 0.23.45
python3 scripts/ci/soak.py
cargo +nightly-2026-08-20 deny --locked check
bash scripts/ci/no-tokio.sh
```

Review the lockfile diff and the advisory's patched range; commit the exception
and lockfile together. Do not add a cargo-deny advisory ignore. rustls 0.23.45 was
published at `2026-09-14T15:11:17Z`, so its exception is no longer usable at
`2026-09-21T15:11:17Z`, even though the policy stores only the date `2026-09-21`.

## Wait and discovery counters

DESIGN §10 rule 3 allows at most one native wait/discovery call per turn. Queued
completions forbid blocking waits; one zero-time discovery poll is allowed only
with native operations pending and native output reserve. Queued pure-core work
with no native operations makes no OS call. Test `os_waits == 0` and
`discovery_polls <= 1` for queued turns with pending native work; require both
zero without native operations. For all-call bounds use their sum. Preserve the
raw `zero_event_waits` counter across both categories and every §10 rule 4a no-spin
bound. Callback/Event-helper queue draining is neither kind of native call.
See [Backend revision 2](docs/BACKEND_REVISION_2.md#blocking-waits-and-nonblocking-discovery-tl-i01b).

## Workspace discovery and test metadata

Scripts use `cargo metadata` and its `workspace_members`, never a crate list or
filesystem glob of manifests. `spikes/*` are rejected as workspace members and
cannot be published. `publish = false` excludes helpers from releases. All local
path dependencies of publishable crates need explicit registry version requirements.

Optional `[package.metadata.turnloop-ci]` describes test capabilities. The role
may be `core`, `contract`, `protocol`, `codec` or `bench`.
`codec` marks the portable Zstandard library, which needs corpus/allocation coverage
instead of external-service metadata. During integration, `*-contract`,
`*-bench` suffixes and direct `protocols/` members are recognized from metadata;
other crates default to core.
Use explicit roles for layouts that differ.

Core must mark **real pure-Rust** Miri-compatible library test filters. No default
Miri filter is guessed, and an empty suite fails. The existing core loom filters
are `models` (five production notifier/queue/pool models); new core crates should
mark their filters explicitly:

```toml
[package.metadata.turnloop-ci]
role = "core"
loom-filters = ["models"]
miri-filters = ["buffer::tests", "timer::tests", "table::tests"] # examples: use actual compatible names
```

The contract crate must provide `wasm-bindgen-test` integration test targets for
browsers and the Node subset, using actual target names from Cargo metadata:

```toml
[package.metadata.turnloop-ci]
role = "contract"
web-tests = ["browser"]
node-tests = ["node"]
# Optional feature names, also discovered from this package's metadata:
web-tests-features = ["browser"]
node-tests-features = []
```

```bash
bash scripts/ci/install-wasmtime.sh
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3
python3 scripts/ci/install-tools.py wasm-pack
python3 scripts/ci/run-tests.py web  # wasm-pack test --headless --chrome --firefox
python3 scripts/ci/run-tests.py node # wasm-pack test --node
python3 scripts/ci/run-tests.py loom
python3 scripts/ci/run-tests.py miri
```

Install the appropriate Rust targets and Miri component first. Linux browser CI installs the checksum-pinned Chrome for Testing/matching driver
and Firefox/geckodriver with `python3 scripts/ci/install-browsers.py`. The runner
uses explicit binary paths and prints persistent driver logs on failure; zero
browser tests is a failure. wasm-pack runs in no-install mode after the matching
wasm-bindgen CLI is built by `install-web-tools.py`. WASI gets network
access only inside the disposable test runner; each component has a 120-second
runtime timeout. A standalone spike is not a substitute for the shared contracts.

## Protocol service convention

Each protocol member declares **integration-test target names** under
`package.metadata.turnloop-ci.integration-tests`, e.g. `["servers"]`. These must
be real Cargo test targets, not names of ignored unit-test functions. CI runs each
with `--include-ignored --test-threads=1 --nocapture`; ignored real-server tests
must actually execute. The protocol job fails if any protocol member lacks a
declaration, or a selected suite runs zero tests.
The runner attempts every declared suite even after failures, prints a per-suite
PASS/FAIL table, and appends it to `GITHUB_STEP_SUMMARY` when set. Missing/invalid
metadata, command failures and zero executed tests remain failures of the job.

CI sets `TURNLOOP_TEST_REQUIRED=1`. A harness must **fail**, never return success,
if a required connection/certificate variable is absent, a service is unavailable,
or a requested capability is missing. Without that flag local tests may be ignored
explicitly; describe them as UNRUN. Tests must assert rows, authenticated sessions,
round trips, replicated writes, cluster redirections, etc., not merely open sockets.

The local runner and Linux CI use the same **port-based** fixture contract. Every
port is loopback-only; there is no fallback to a default port or system instance.

| Variable | Meaning |
|---|---|
| `TURNLOOP_TEST_POSTGRES_PORT` | PostgreSQL port; database `postgres`, users `scram_user`, `md5_user`, `clear_user`, `tls_user`, password `fixture-password` |
| `TURNLOOP_TEST_MYSQL_PORT` | MySQL port; database `turnloop_test`, users `sql_user`, `auth_rsa_user`, `tls_user`, password `fixture-password` |
| `TURNLOOP_TEST_SQL_TOOLS` | Directory containing `server.der`, `server.crt`, `server.key`; verified TLS and PostgreSQL channel binding |
| `TURNLOOP_TEST_REDIS_PORT` | Password/ACL Redis instance; user `lane` |
| `TURNLOOP_TEST_REDIS_PASSWORD` | Disposable Redis password |
| `TURNLOOP_TEST_REDIS_TLS_PORT` | Same instance with TLS, checked against committed test CA |
| `TURNLOOP_TEST_REDIS_CLUSTER_PORT` | Seed of six-node cluster: three masters, three replicas |
| `TURNLOOP_TEST_REDIS_SENTINEL_PORT` | Sentinel monitoring `turnloop`, with authenticated master |
| `TURNLOOP_TEST_MONGODB_PORT` | Fresh authenticated standalone; suite bootstraps user `lane`, password `pencil` |
| `TURNLOOP_TEST_MONGODB_REPLICA_PORTS` | Three comma-separated ports; set `turnloop_test`, keyfile auth |
| `TURNLOOP_TEST_MONGODB_TLS_PORT` | Fresh TLS standalone; same authentication tests |
| `TURNLOOP_TEST_MONGODB_TOOLS` | Directory containing MongoDB `cert.pem` |
| `TURNLOOP_TEST_HTTP_PORT` | Private Node HTTP/1.1 fixture (redirects, gzip, trailers and connection reuse) |
| `TURNLOOP_TEST_HTTP2_PORT` | Private Node HTTP/2 fixture (100 multiplexed streams) |
| `TURNLOOP_TEST_SMTP_PORT` | Postfix smtp-sink loopback port |
| `TURNLOOP_TEST_SMTP_TOOLS` | Directory containing sink message dumps for byte assertions |

```bash
scripts/test-servers.py run cargo test --workspace -- --include-ignored
scripts/test-servers.py start   # prints shell exports; explicitly source them to run tests
scripts/test-servers.py stop
# Reproduce PostgreSQL's CI configuration with separate proxied TCP connections:
scripts/test-servers.py --services postgres --postgres-proxy run cargo test \
  -p turnloop-postgres --test server -- --include-ignored --test-threads=1 --nocapture
# Explicit subset for machines unable to run SQL; does not count as a full pass:
scripts/test-servers.py --services redis,mongodb,smtp run cargo test \
  -p turnloop-redis -p turnloop-mongodb -p turnloop-smtp -- --include-ignored --test-threads=1
```

Run one fixture lifecycle command at a time in a checkout. The runner owns only private instances and data under `.tools/`, always cleans up
on command failure, and fails when a selected capability cannot start. It locates
installed binaries on PATH (with macOS/PostgreSQL installation fallbacks); it does
not install or upgrade system software. PostgreSQL supports cleartext, MD5,
SCRAM and TLS/PLUS fixtures; MySQL covers caching-SHA2 fast/full/RSA and TLS, and
records native-password plugin availability (MySQL 9 removed that plugin).
The MySQL auth test deliberately warms its RSA and TLS accounts, then clears
the server authentication cache with `FLUSH PRIVILEGES` through the private TLS
`auth_admin` account (password `fixture-password`, RELOAD privilege). It asserts
the reset's successful acknowledgement, full RSA auth, full TLS auth without RSA,
and later fast hits on both accounts. This also covers repeated tests after CI's
independent TLS probe. Run fixture suites serially; cache invalidation is global
to the disposable server.
MongoDB cleanup is an explicit example invoked only by `stop`, never a test that
could shut down a concurrently running suite. SMTP's installed-sink test uses the
runner; in-process TLS/auth SMTP peers remain normal tests.

In the managed macOS sandbox PostgreSQL initialization fails at `shmget` and
MySQL initialization crashes. Those real-server tests are **UNRUN (sandbox)**;
run the full command outside the sandbox.

Local PostgreSQL initializes the administrator as `turnloop`, matching CI's
`POSTGRES_USER`; a `postgres` database exists but a `postgres` role is not assumed.
Both paths apply the same three SSL `ALTER SYSTEM` settings and reload, then check
effective TLS settings and pending restarts and print versions, roles and timeout
settings. The optional loopback TCP proxy preserves bytes and half-closes, opens
one upstream connection per client, and reports forwarded bytes, connection counts
and CancelRequests. The cancel test observes its exact backend PID using another
`scram_user` session, requires SQLSTATE 57014 and checks subsequent session reuse.

HTTP fixture servers use the same lifecycle and authenticated private-instance
shutdown. Node **26.5.1** is pinned with setup-node on native and protocol CI.
Node fetch and `node:http2` legs are mandatory on every native runner. curl legs
use the installed executable's `curl -V` capabilities (`Protocols: http`, plus
`Features: HTTP2` for h2); missing HTTP2 skips only that curl leg. Tests print each
verified leg and run a real 100-stream Node regression with curl HTTP2 unavailable.
All fixture network connections use explicit IPv4 loopback; `localhost` in TLS
certificates/SNI or HTTP authority fields does not select the transport address.
The `service-group = "http"` metadata selects HTTP/TLS/WebSocket interop targets:

```bash
scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop
python3 scripts/ci/h2spec.py
python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2
```

`h2spec.py` downloads the pinned source commit in `scripts/ci/tools.json`, verifies
its SHA-256 before extraction, verifies Go modules, builds locally, and runs strict
conformance. Its JUnit gate requires 147 distinct successful tests, no failures,
errors or skips, and consistent suite counters. A 16-KiB response ensures the
negative-window test executes. Go and a native compiler must be installed.

`wasi-tests` metadata names portable test targets; HTTP declares codecs/allocations,
and the decoder declares its allocation regression. Allocation targets use standalone,
unconditionally executed harnesses with positive allocator calibration, preserving
every test and the zero-allocation thresholds. This avoids pinned WASI 0.3
libtest CLI-argument lowering calling the generated allocator shim without a
valid stack; these targets always run their full list, regardless of test filters. Both WASI 0.2 and 0.3 have a
required protocol job, independent of the pending production backend contracts.
For ring, install the same pinned C toolchain used by CI on Linux x86_64 or
macOS arm64 (no system installation required):

```bash
python3 scripts/ci/install-wasm-toolchain.py
source .tools/wasm-env.sh
cargo clippy --locked --workspace --all-targets --target wasm32-wasip2 -- -D warnings
cargo clippy --locked --workspace --all-targets --target wasm32-unknown-unknown -- -D warnings
bash scripts/ci/install-wasmtime.sh
python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2
python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3
```

The installer verifies [wasi-sdk 34.0's official release hashes](https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-34)
against `scripts/ci/tools.json` before extraction into `.tools/`. It provides
absolute target-specific `CC_*`/`AR_*` paths for p2, p3 and browser wasm; CI receives
them through `GITHUB_ENV`. It compiles a wasm object and archives it for each target
before reporting success. This fixes CI run 34874044440's missing `llvm-ar` and
Apple clang's absent wasm backend. The SDK includes a WASI sysroot; Rust retains
its own target linker/libc, and ring builds its freestanding C objects with clang.
No extra browser libc or default rustls provider is enabled. Re-source the file
in each local shell; do not set a global `CC` that would change native builds.

`turnloop-tls` additionally declares `wasi-tests = ["portable"]`: its in-memory
transport runs real ring-backed handshakes, encrypted records, ALPN, resumption,
certificate rejection and injected deadlines without native sockets or threads.
This establishes TLS correctness, not allocation freedom of upstream rustls records: a
separate probe measured four allocations per bidirectional record exchange in
rustls 0.23.45. Existing core/HTTP/decoder allocation gates remain mandatory.
The HTTP/decoder codec and allocation suites continue to execute on p2 and p3.
Database/SMTP real-server TLS suites still need their native fixture transports;
compilation of those targets is not counted as WASI server execution. Protocol
library and all-test-target Clippy covers browser wasm; browser runtime contracts
remain owned by the backend lane.

The shared rustls configuration disables defaults and selects ring/std/tls12.
SQL/Redis/SMTP/Mongo test transports and the TLS library use the same version and
provider. ring supports the native targets and builds on wasm32; browser entropy
uses its JS feature, WASI uses host randomness. The adapter still takes host time.
No aws-lc provider, FIPS or post-quantum guarantee is selected. Mozilla trust-anchor
data uses CDLA-Permissive-2.0; its redistribution text ships in turnloop-tls.

`turnloop-zstd-decoder` is an MIT fork of ruzstd 0.8.3. HTTP's versioned dependency
works for crates.io consumers without `[patch]`. Native `pure-rust-zstd` tests and
WASI tests exercise the same decoder. See `docs/upstream/ruzstd.md` for the exact
maintainer submission and reproduction; source/license/corpus provenance ships
with the decoder. `cargo publish --dry-run --locked --allow-dirty --workspace`
stages unpublished siblings together and verifies every packaged library. Never
use `--no-verify` to bypass this dependency chain. Do not remove their ignored test bodies
or treat a failed initializer as a test pass.

CI's PostgreSQL **16.13**/MySQL **9.6.0** service containers are provisioned by
`scripts/test-servers.py --ci-services`. Redis runs natively at **8.4.0**, matching
the local fixture. `python3 scripts/ci/install-redis.py` builds the official tarball
after verifying its committed SHA-256 against Redis's published release digest,
with TLS enabled for both server and CLI. CI caches only `.tools/redis-build`,
keyed by Ubuntu version, architecture and the complete installer hash; there are
no fallback cache keys. Cache hits still verify the binary versions and CLI TLS
support, and real Redis TLS tests remain required. Add `.tools/redis-build/bin`
to PATH when using this build locally; CI exports it through `GITHUB_PATH`.

The six-node Redis cluster (three masters and three replicas), single/TLS instance
and Sentinel use the native runner's configurations, auth and certificates. The
five MongoDB instances use private named Linux containers; CI pulls their image
before the bounded startup wait. SQL TLS probes precede the metadata-selected Rust
suites. Mongo containers run with the host UID/GID, and their ownership labels
are checked during cleanup. The Docker
path is **UNRUN locally** because the development sandbox has no Docker.

Private server stdout and stderr are captured together under `.tools/`: Redis
`redis/<instance>/server.log`, MongoDB `mongodb/<instance>/process.log` plus
`mongod.log`, SQL `sql/*init*.log`, `postgres.log`, `mysqld-console.log` and
`mysql.log`, SMTP `smtp/server.log`, and HTTP `http/server.log`. Startup failures
print the relevant last 40 lines (at most 16 KiB per file), including errors before internal logging is
initialized. `scripts/test-servers.py logs` prints bounded tails and copies only
known log files to the flat `.tools/protocol-logs/` directory. CI uploads that
directory as `protocol-server-logs`, without recursively scanning server data.
On protocol job failure, CI also prints and preserves both SQL service container
logs. The only fixture build cache is `.tools/redis-build`; it contains no data.
Mongo cleanup stops/removes containers or stops native servers, reaps owned
children and verifies closed ports before deleting recorded data directories.
Private PostgreSQL/MySQL data is deleted after shutdown, including partial data
from failed initialization. Logs and certificates are retained outside data roots.
Cleanup reaps owned Redis children, removes state for crashed instances, and
checks private config identity before a separate invocation sends SHUTDOWN.
An unresponsive or unidentified instance retains its record. Cleanup failures
are chained beneath the original startup/test error so both remain visible.

## Pending platform and measurement gates

The `wasi` and `web` jobs run the production revision-2 providers and remain
required. WASI 0.3 keeps its experimental feature; Linux must execute both pinned
browsers. WASI runners enable the executor and independently require positive core,
debug/release semantic and release allocation counts, supplying a real stdin
fixture to the semantic tests. The instruction job compares against the committed Linux
baseline. The strict `ci-gate` fan-in is retained. Windows `test-native`
runs the workspace and independently requires positive counts for core and every
protocol member, with default, executor and all features. The existing sans-IO unit, wire,
SCRAM, SDAM/selection fixture and allocation tests are portable. No unnecessary
Unix test cfg exclusions were found.

The production IOCP backend implements revision 2 and runs the shared contracts,
including no-spin and allocation gates, plus Windows process, console, GUI and
lifetime tests. The pending-contract metadata has been removed; zero counts fail
on Windows just as on Unix. Cross-compilation does not establish runtime coverage.

- **Windows:** run `python3 scripts/ci/run-tests.py native` on Windows for all
  three required modes. Keep platform-specific runtime results explicit in lane
  reports when developing on another host.
- **WASM providers and contracts:** production p2 and web adapters and the
  experimental p3 adapter now live in `crates/turnloop`. Run the mandatory
  `run-tests.py wasi --target wasm32-wasip2`, `... wasm32-wasip3`, `run-tests.py web`
  and `run-tests.py node` jobs. Each contract/allocation binary and each browser
  must execute positive test counts. The runner owns and checks actual HTTP,
  aborted fetch and WebSocket fixture traffic. See [docs/wasm.md](docs/wasm.md)
  for setup, explicit platform exclusions and outstanding p3 runtime limitations.
  These remain failing gates until the strict requirements pass.
- **Linux instruction regression:** run against the committed baseline on Ubuntu
  24.04 x86_64. Use the candidate command below only for a reviewed rebaseline;
  macOS timings must never substitute for Linux instruction counts.

## Instruction regression gate

The iai-callgrind instruction gate uses its maintained successor **Gungraun**.
The previous 0.16.1 pin pulls unmaintained `proc-macro-error2`
([RUSTSEC-2026-0173](https://rustsec.org/advisories/RUSTSEC-2026-0173)).
Gungraun 0.19.4 fixes that dependency; the gate still requires actual Callgrind
counts, three fresh runs, v6 summaries, the same 3% ceiling and exact controls.
See the [upstream changelog](https://github.com/gungraun/gungraun/blob/v0.19.4/CHANGELOG.md).


The bench crate must depend on **`gungraun = "=0.19.4"`**, with at least one
`[[bench]]` target using `harness = false`. The matching runner is SHA-256 pinned.
Mark `instruction-baseline = "benchmarks/instructions.json"` relative to its
manifest. Each measured function must assert its workload counter/bytes; include
idle turn, notify, timer start/cancel and an independent integer control workload.

Baselines contain `toolchain`, exact `valgrind --version`, `counts` (a map from
iai `module_path[/id]` to positive `Ir` count), and `controls` (control keys). On
Ubuntu 24.04 x86_64, generate candidates with:

```bash
python3 scripts/ci/instructions.py --record .tools/instruction-candidates.json
```

Declare every expected summary key in `instruction-cases` and the exact control
keys in `instruction-controls` in the bench package's CI metadata. Recording
requires all declared cases in all three fresh-process rounds, positive counts,
exact controls, and the same 3% ceiling. Candidate counts use the smallest measured
count for each case; all three rounds must pass comparison against those counts.
Candidate recording never overwrites the committed baseline.

Standard CI runs `python3 scripts/ci/instructions.py`. If a baseline is missing,
the job measures all benchmarks, uploads artifact **`instruction-baselines`**, and
**fails** with commit instructions. The artifact contains ready-to-review baseline
files at their repository-relative paths, `measurements.json` with all three rounds,
and `README.txt` with the exact commands. It contains no invented Linux counts.

To request a fresh baseline even when one exists, run the CI workflow manually
with the boolean `record_baselines: true` (or `gh workflow run ci.yml -f record_baselines=true`).
The instruction job then runs `instructions.py --record-baselines`, uploads the
same artifact, and succeeds after valid measurement without claiming a regression
comparison passed. All other required jobs still run normally.

Review the rounds and control counts, then from the repository root:

```bash
gh run download <RUN_ID> --repo PerryTS/turnloop --name instruction-baselines --dir .tools/instruction-baselines
mkdir -p crates/turnloop-bench/benchmarks
cp .tools/instruction-baselines/crates/turnloop-bench/benchmarks/instructions.json crates/turnloop-bench/benchmarks/instructions.json
git add crates/turnloop-bench/benchmarks/instructions.json
git commit -m "Record Linux instruction baselines"
```

Push the reviewed baseline and rerun ordinary CI to execute the regression check.
The first bootstrap cannot be verified by macOS execution; Linux runtime counts
must come from the Ubuntu 24.04 x86_64 runner.

The gate builds with one codegen unit, requires new v6 summaries and nonzero
counts, rejects missing/extra benchmarks and fails above **3%** growth in any
round. Instruction counts avoid scheduler timing noise; 3% leaves modest code
layout variation at cgu=1 while detecting material hot-path regressions (well
below the 8% variation observed with cgu=16). Controls must match **exactly**.
Compiler/Valgrind changes need a reviewed rebaseline, not a wider threshold.
Do not populate Linux baselines from macOS rusage or synthetic numbers.

## Automation notes

All third-party Actions have full SHA pins and version comments; downloaded tools
have committed SHA-256 pins in `scripts/ci/tools.json`. Updating a tool means
verifying its official release digest and rerunning local gates. No PR job receives
publishing credentials. Optional self-hosted Windows runs only when
`SELF_HOSTED_WINDOWS == 'true'`, on `turnloop-windows`; owners must provision an
**ephemeral isolated** runner suitable for public PR code. Otherwise only that
one skipped job is expected by ci-gate.

The latest official actionlint v1.7.12 does not understand GitHub's new
`concurrency.queue`. `lint-workflows.py` first validates the exact required
queue/cancellation expressions, then filters only that unsupported-key message
and runs every remaining actionlint, zizmor and ShellCheck check. Tests reject
queue weakening. Raw actionlint still reports that known parser limitation.
Remove the compatibility filter when an official binary supports it. The one
zizmor annotation documents a guarded workflow_run release trigger: only verified
main pushes, with no PR artifacts or caches entering the privileged workflow.

See [RELEASING.md](RELEASING.md) for owner setup, publication and Perry consumption.

## Async adapters on turnloop

Protocol crates expose their async layer through a `turnloop` feature. Shared
transport ownership, partial input/output and deadline handling live in the
publishable `turnloop-io` crate; see its README for the adapter contract. The
`adapter` CI role requires independent positive native test counts and declared
WASI suites. Protocol integration targets' `required-features` are activated by
`run-tests.py`; an absent or zero-test async suite remains a failure.

The required native interop job runs Node HTTP/HTTPS/HTTP2/WebSocket and curl
against the async layer, and h2spec runs all 147 strict tests against the async
server. WASI protocol jobs execute async HTTP, TLS and WebSocket over wasi:sockets,
alongside the shared driver's socket/allocation tests. The web/Node contracts
exercise executor fetch with deadlines and aborts using real fixture traffic.
The feature-coverage gate validates the required WASI/web jobs, their targets,
feature forwarding and runtime commands as well as the explicit native modes.

Adapter allocation gates retain existing core-owned HTTP head and rustls record
costs while requiring zero extra transport allocations. The TLS gate verifies
100 bidirectional real-socket records at rustls 0.23.45's existing 400 allocations;
WebSocket frames and 1,000 warmed shared TCP driver exchanges require absolute zero.
None of the pre-existing allocation thresholds or dependency policies change.
