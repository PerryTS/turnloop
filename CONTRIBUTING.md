# Contributing

`DESIGN.md` draft 0.3 is the specification. The repository is public and its full
required CI matrix runs on **every PR**, including documentation and automation
changes. The single branch-protection check is **`ci-gate`**. Run tests on the
platform where their subject executes; a cross-target check is not a test pass.

## Local checks

Use the pinned nightly (`nightly-2026-08-20`) to resolve dependencies with the
seven-day minimum publish age. Keep `Cargo.lock` committed. Stable Rust is also
required, with no nightly language features in the library. WASI 0.3 alone uses
`nightly-2026-09-07` and Wasmtime 46.0.0, based on the WASM lane's successful spike.

```bash
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

Every unsafe block must explain its safety with `// SAFETY:` and crate roots deny
`unsafe_op_in_unsafe_fn`. No `unwrap()` on I/O paths. Assert actual completions,
bytes or work counters; success with zero tests is an error. Contracts run with
`--test-threads=1` for global signal/process and allocator isolation. Default and
all-feature native suites exercise the timer alternative and forced Linux
`epoll-timerfd` backend; newly introduced incompatible feature combinations need
explicit metadata-driven CI coverage, not silent exclusions.

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

## Workspace discovery and test metadata

Scripts use `cargo metadata` and its `workspace_members`, never a crate list or
filesystem glob of manifests. `spikes/*` are rejected as workspace members and
cannot be published. `publish = false` excludes helpers from releases. All local
path dependencies of publishable crates need explicit registry version requirements.

Optional `[package.metadata.turnloop-ci]` describes test capabilities. The role
may be `core`, `contract`, `protocol` or `bench`. During integration, `*-contract`,
`*-bench` suffixes and direct `protocols/` members are recognized from metadata;
that also supports the windlass → turnloop rename. Other crates default to core.
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

Install the appropriate Rust targets and Miri component first. Both browsers must
be installed; GitHub's Ubuntu image supplies Chrome and Firefox. wasm-pack locates
or downloads the matching WebDriver and wasm-bindgen test runner. WASI gets network
access only inside the disposable test runner; each component has a 120-second
runtime timeout. A standalone spike is not a substitute for the shared contracts.

## Protocol service convention

Each protocol member declares **integration-test target names** under
`package.metadata.turnloop-ci.integration-tests`, e.g. `["servers"]`. These must
be real Cargo test targets, not names of ignored unit-test functions. CI runs each
with `--include-ignored --test-threads=1 --nocapture`; ignored real-server tests
must actually execute. The protocol job fails if any protocol member lacks a
declaration, or a selected suite runs zero tests.

CI sets `TURNLOOP_TEST_REQUIRED=1`. A harness must **fail**, never return success,
if a required connection/certificate variable is absent, a service is unavailable,
or a requested capability is missing. Without that flag local tests may be ignored
explicitly; describe them as UNRUN. Tests must assert rows, authenticated sessions,
round trips, replicated writes, cluster redirections, etc., not merely open sockets.

| Variable | CI value / meaning |
|---|---|
| `TURNLOOP_TEST_POSTGRES_URL` | `postgres://turnloop:turnloop@127.0.0.1:5432/turnloop` (PostgreSQL 16, SCRAM) |
| `TURNLOOP_TEST_MYSQL_URL` | `mysql://turnloop:turnloop@localhost:3306/turnloop` (MySQL 9) |
| `TURNLOOP_TEST_MYSQL_TLS_CA` | Absolute path to ephemeral CA PEM |
| `TURNLOOP_TEST_MYSQL_TLS_SERVER_NAME` | `localhost`; certificate SAN includes localhost and 127.0.0.1 |
| `TURNLOOP_TEST_MYSQL_AUTH_PLUGIN` | `caching_sha2_password`; TLS and certificate validation required |
| `TURNLOOP_TEST_REDIS_URL` | `redis://127.0.0.1:6379` (Redis 8) |
| `TURNLOOP_TEST_REDIS_CLUSTER_URLS` | Comma-separated Redis seed URLs, ports 7000–7002; three masters plus three replicas |
| `TURNLOOP_TEST_MONGODB_URL` | `mongodb://turnloop:turnloop@127.0.0.1:27017/turnloop?authSource=admin` (Mongo 8 standalone, SCRAM) |
| `TURNLOOP_TEST_MONGODB_REPLICA_URL` | `mongodb://127.0.0.1:27018,127.0.0.1:27019,127.0.0.1:27020/turnloop?replicaSet=turnloop-rs` |

The replica set binds loopback without authentication for topology/majority tests;
standalone Mongo provides authenticated coverage. Cluster members announce
loopback endpoints reachable by the host test process. Bootstrap scripts prove
all members become healthy and perform a cluster read/write and majority Mongo
write. MySQL uses a generated certificate and a verified TLS query; insecure
transport is forbidden for its application user. Credentials are disposable
fixture constants, not production secrets. Service containers are isolated per job;
`cleanup-services.sh` removes only the explicitly named extra cluster containers.

HTTP/WebSocket/TLS/SMTP harnesses should create their own loopback peers inside
these integration targets (and assert protocol exchanges). There are no shared
external services for those protocols. Native service tests must be cfg-gated out
of WASM builds, while pure protocol tests remain portable. Current lane harnesses
were incomplete at CI handoff; use this single convention when integrating them.

## Instruction regression gate

The bench crate must depend on **`iai-callgrind = "=0.16.1"`**, with at least one
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

Review all three fresh-process rounds, choose stable counts, identify controls,
and commit the baseline in the bench crate. Candidate recording is **not** a gate
pass and never overwrites a baseline. Standard CI runs
`python3 scripts/ci/instructions.py`, never `--record`.

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
