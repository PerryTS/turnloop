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
MongoDB cleanup is an explicit example invoked only by `stop`, never a test that
could shut down a concurrently running suite. SMTP's installed-sink test uses the
runner; in-process TLS/auth SMTP peers remain normal tests.

In the managed macOS sandbox PostgreSQL initialization fails at `shmget` and
MySQL initialization crashes. Those real-server tests are **UNRUN (sandbox)**;
run the full command outside the sandbox.

HTTP fixture servers use the same lifecycle and authenticated private-instance
shutdown. Node **26.5.1** is pinned with setup-node on native and protocol CI.
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
and the decoder declares its allocation regression. Both WASI 0.2 and 0.3 have a
required protocol job, independent of the pending production backend contracts.
For ring on macOS, set `CC_wasm32_wasip2` and `CC_wasm32_unknown_unknown` to
`/opt/homebrew/opt/llvm/bin/clang`, and the corresponding `AR_*` values to
`/opt/homebrew/opt/llvm/bin/llvm-ar`; Linux CI uses clang/llvm-ar. Protocol library
and all-test-target Clippy covers browser wasm, while browser runtime contracts
remain pending with the backend lane.

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

CI's PostgreSQL 16/MySQL 9 service containers are provisioned by
`scripts/test-servers.py --ci-services`. That mode launches the same six-node
Redis topology and five MongoDB instances using private named Linux containers,
with the same configurations, auth, certificates and environment variables as the
native runner. It runs verified SQL TLS probes, then the metadata-selected Rust
suites. Container identities/ownership labels are checked during cleanup. This
Docker path is **UNRUN locally** because the development sandbox has no Docker.

## Pending platform and measurement gates

The `wasi`, `web`, Windows shared-contract portion of `test-native`, and
`instructions` jobs intentionally remain required and failing until their missing
inputs land. Their commands and the strict `ci-gate` fan-in are retained. No
`continue-on-error`, empty-test success or expected skip is permitted for them.

- **Wave 2 Windows:** adapt `spikes/iocp` to the production Backend, instantiate
  `turnloop-contract` on IOCP, run all contracts (including no-spin) on Windows.
- **Wave 2 WASI:** integrate both providers, provide nonzero shared contracts on
  `wasm32-wasip2` and `wasm32-wasip3`, run the existing `run-tests.py wasi` commands.
  Resolve the p3 bounded waitable-set API before claiming D7 compliance.
- **Wave 2 web:** provide real browser and Node test targets, add `web-tests` and
  `node-tests` metadata, run Chrome, Firefox and Node via `run-tests.py`.
- **Linux instruction baseline:** run the candidate command below on Ubuntu
  24.04 x86_64, review three rounds and exact controls, commit the measured baseline.
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
