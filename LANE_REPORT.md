# ci-fix4 lane report

Updated 2026-09-14. Scope implemented on main 59593ee. The requested path, wasm
compiler/provider and Windows curl fixes pass their local checks. **The entire
native CI runner is not green:** it exposed an unchanged core2 signal race after
both workspace configurations passed. Windows/Linux runtime remains UNRUN.

Read completely: DESIGN.md, CONTRIBUTING.md, docs/INTEGRATION_REPORT.md and
CI/HTTP/Windows/WASM lane reports. No applicable AGENTS.md. No commits by this
agent; integrator checkpoints 648efb7 and 23a95e2 appeared externally.

## Implemented

- `scripts/ci/check-paths.py`: independent Python/Git gate, no Cargo dependencies.
  Uses index spellings and working-tree source, rejects case collisions (including
  directories), checks include_str/include_bytes, #[path], default/inline modules,
  conditional paths and their default fallback, and Cargo readme/license-file
  entries including workspace definitions. Rust comments/strings/characters are
  lexed; raw/escaped literals and concat with CARGO_MANIFEST_DIR are supported.
  Unknown computed includes fail closed. Parent normalization cannot hide a
  misspelled intermediate directory. Explicit Cargo target roots and modules
  loaded through #[path] use their correct source directories.
- Eight path regressions include a real temporary Git index with tracked
  `Readme.md` and an include of `README.md`, asserting CLI failure and the exact
  diagnostic, then success after fixing only the reference. Synthetic collisions
  also work on case-insensitive hosts. Guard runs in workflow-lint and the local
  `scripts/verify-core.sh`; CONTRIBUTING lists it in local preflight.
- Chose **option (a), retain ring 0.17.14**. The supplied WASI log's exact stderr
  was `failed to find tool "llvm-ar": No such file or directory`. The existing
  provider already supports these targets; no replacement crypto dependency or
  soak override is needed. `install-wasm-toolchain.py` installs wasi-sdk 34.0's
  clang, llvm-ar and sysroot under `.tools/`, checking committed official hashes
  before extraction. It compiles and archives a real wasm object for p2/p3/web.
  CI exports absolute target-specific CC/AR paths via GITHUB_ENV; local shells
  source `.tools/wasm-env.sh`. Three installer regressions cover checksum refusal,
  actual extraction, path traversal rejection and target environment mapping.
- SDK pins, released 2026-08-25: Linux x86_64 SHA-256
  `b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4`;
  macOS arm64 `9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9`.
  Verified against the [official release and asset digests](https://github.com/WebAssembly/wasi-sdk/releases/tag/wasi-sdk-34).
  Rust keeps its target linker/libc; ring uses clang's freestanding C path.
  The wasi/web jobs receive only the shared compiler prerequisite; their backend
  gates/runner semantics are untouched.
- New `turnloop-tls/tests/portable.rs`, declared through existing `wasi-tests`
  metadata: three real in-memory TLS tests with fragmented ciphertext, generated
  signing keys, bidirectional payloads, ALPN, full/resumed handshakes, four distinct
  certificate rejection cases, explicit insecure mode and exactly-once injected
  timeout. Bounded pumps assert records/bytes and both handshake states. Runs on
  native Windows as well as both WASI versions; browser all-target lint includes it.
- HTTP interop probes the same `curl -V` executable it runs. HTTP/1 requires the
  `http` protocol; HTTP/2 additionally requires the exact `HTTP2` Features token.
  Mandatory Node runs first, before probing curl; neither a missing curl feature
  nor a broken curl probe can suppress that execution. Missing Node fails.
  Both tests count successful legs and print which ran; Node h2 verifies all 100
  streams. Added Windows-style capability parsing fixtures and a real 100-stream
  Node regression with curl HTTP2 disabled. Bounded Node-fetch timeout added.
- Reviewed all HTTP/TLS/WebSocket tests and the shared Node fixture: no /bin/sh,
  Unix-only fixture paths, or hostname-based socket connections to change. Every
  actual network address is IPv4 loopback. `localhost` remains only in protocol
  fields, certificate names and SNI. No Windows cfg or portable-tests gate removed.
- Removed the two supplied CI logs from docs, as requested. Original evidence is
  retained locally under `.tools/ci-fix4/ci-original-*.log`.

No runtime implementation, Cargo.lock, workspace dependency declaration, soak
configuration/security exception, protocol runner, Postgres harness or MongoDB
cleanup was changed. No backend/no-spin or pre-existing allocation gate changed.

## Verification summary

All commands and intermediate results are preserved in the ledger below. Wasm
commands inherit `source .tools/wasm-env.sh`; audit tools use `.tools/bin` on PATH.
Cargo uses nightly-2026-08-20 unless shown; stable here is 1.97.1. Raw output is
under `.tools/ci-fix4/`. Commands within the CI test wrappers appear in those logs;
the outcomes below distinguish their successful subcommands from overall failure.

| Check | Result |
|---|---|
| Path guard and eight synthetic regressions | PASS; current index 1306 files / 170 references |
| Full Python automation tests | PASS: 60 tests, warnings treated as errors |
| Pinned SDK installation and all three compile/archive probes | PASS on macOS arm64; official Linux artifact digest verified, Linux executable UNRUN |
| cargo fmt --check | PASS |
| Native workspace/all-target Clippy, default and all features, warnings and undocumented unsafe denied | PASS |
| Whole-workspace wasm32-wasip2 and wasm32-unknown-unknown all-target Clippy, default/all features | PASS |
| Stable workspace/all-target/all-feature check | PASS |
| Linux x86_64 all-target/all-feature Clippy | PASS (compile only) |
| Windows whole-workspace all-target/all-feature Clippy | FAIL: no Windows C SDK headers; ring/zstd-sys cannot build (assert.h/string.h/stdlib.h). Changed HTTP test execution UNRUN on Windows |
| Windows core/contract/bench all-target/all-feature Clippy | PASS (compile only) |
| WASI 0.3 whole-workspace Clippy | FAIL: inherited BSON dependency getrandom 0.3.4 rejects p3; ring C build now works |
| WASI 0.3 TLS all-target Clippy | PASS, warnings and undocumented unsafe denied |
| cargo test --workspace -- --test-threads=1 | PASS rerun: 217 tests. Initial FAIL in unchanged core2 256-child spawn (OS error 3 / ESRCH) |
| run-tests.py native | FAIL overall in final independent all-feature core contract run: SIGUSR1 (signal 30) during signals_reach_four_loops_on_four_threads. Workspace default 217 and all-feature 232 tests PASS; all independent core/protocol member runs PASS; default contract 42 PASS; all-feature contract terminated after preceding 35 passed tests |
| Fixture-driven HTTP/TLS/WebSocket interop | PASS: 9 HTTP + 5 TLS + 3 WebSocket, zero ignored, real Node/curl peers |
| protocol-wasi --target wasm32-wasip2 | PASS: 23 tests, zero ignored: HTTP codecs 16 + HTTP allocation 3 + TLS 3 + decoder allocation 1 |
| protocol-wasi --target wasm32-wasip3 | PASS: same 23 tests under Wasmtime 46.0.0 |
| no-tokio.sh | PASS: all eight configured targets plus union graph, default/all features |
| soak.py | PASS: 241 locked registry versions; existing exact rustls exception remains the only exception |
| cargo deny --locked check | PASS advisories/bans/licenses/sources; existing duplicate-major warnings |
| Workflow lint (actionlint compatibility wrapper, zizmor, ShellCheck) | PASS |
| git diff --check | PASS |

## Failures, deviations and DESIGN proposals

No DESIGN.md edit proposed for these CI fixes. Retaining ring avoids introducing
a new provider and is backed by real WASI crypto execution, not compilation alone.
Database/SMTP native real-server TLS harnesses still require threads/native socket
fixtures; they are **UNRUN on WASI**, not replaced by or counted as the portable
TLS suite. The selected provider is shared by those crates and remains usable on
WASI; porting their fixture transports was not added to this CI-fix lane.

The initial new TLS memory harness overwrote successive EncodeTlsData chunks;
the tests failed with InappropriateMessage on native/WASI. Fixed by appending
all handshake chunks and clearing only after transport acknowledgement.

An additional whole-record TLS allocation experiment FAILED: **4000 allocations
for 1000 bidirectional 1024-byte exchanges**, after three warm-ups and a positive
allocator calibration. rustls 0.23.45 ReadTraffic owns Option<Vec<u8>> and queues
owned plaintext; record encryption also allocates. This behavior predates this
lane (the HTTP report explicitly left rustls allocation counts uninstrumented).
The probe initially sat in the new portable test during development; it is now
preserved unchanged at `.tools/ci-fix4/tls-record-allocation-probe.rs`, with its
failed zero assertion and raw result retained. It was not adopted as a new required
CI gate or represented as a pass. Scope clarification received no response; the
announced default was to finish the requested CI fixes, not fork/redesign rustls.
The shipped portable suite tests TLS behavior; **no allocation-free TLS claim** is
made. All allocation gates that existed on starting main remain required and pass.
This performance finding needs separate TLS ownership/specification review.

The two native core2 failures also need independent follow-up: intermittent ESRCH
when registering 256 short-lived children, and SIGUSR1 during the all-feature
signal fan-out test. Successful reruns do not resolve either race. No retry was
added to CI and no core test or assertion was changed to hide them.

## Windows coverage and remaining exclusions

- The unchanged native gate executes the entire workspace in default/all-feature
  modes and independently requires nonzero core/protocol tests. HTTP/TLS/WebSocket
  fixture interop runs on windows-2025 too; both ignored HTTP fixture tests execute
  there via --include-ignored. All new portable TLS and Node fallback tests run.
- Only the curl h2 leg is unavailable when Features lacks HTTP2; Node h2 remains
  mandatory. HTTP/1 curl runs whenever the http protocol exists. A missing curl
  executable is reported as an unavailable curl leg, never a zero-client pass.
- `turnloop-contract`'s 22 native backend contracts, 5/6 allocation tests,
  descriptor lifetime test, 14 native surface tests and 6 feature-gated executor
  tests still require the production IOCP provider/Windows fixtures. The existing
  windows-contracts-pending marker and WINDOWS_HANDOFF.md are unchanged. Unix
  kqueue/epoll internals and the waitid-based core child-race test are Unix-specific;
  driver examples/doctest I/O bodies also await a Windows provider.
- Ten external-service tests remain ignored in Windows native CI: PostgreSQL 3,
  MySQL 3, Redis 2, MongoDB 1 and installed Postfix smtp-sink 1. CI provisions these
  fixtures only in its Linux protocol job (Docker service containers, native Redis,
  Unix Postfix sink). Their portable codecs/auth/TLS transition/allocation tests
  and SMTP in-process TLS socket tests do run on Windows. No additional exclusions.
- Platform-specific instruction/syscall measurements, Miri and loom remain in
  their existing dedicated jobs; they are not Windows runtime test passes.

## UNRUN / exactly what CI and integrator must confirm

1. Commit the working tree (including new scripts/test and the two log deletions).
   Run `python3 scripts/ci/check-paths.py` after staging all reference targets;
   confirm Linux case-sensitive checkout and synthetic guard failures.
2. `lint-wasm` and both `protocol-wasi` jobs: download/hash-check the pinned Linux
   SDK, export GITHUB_ENV tool paths, run requested Clippy and all 23 protocol tests
   per WASI target. Local macOS Wasmtime execution passes; hosted Linux UNRUN.
3. Windows hosted and optional provisioned Windows runner: run
   `python3 scripts/ci/run-tests.py native`, then
   `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop`.
   Confirm printed Node h2 (100 streams), correct optional curl leg, all portable
   member counts, and native C-dependency lint. Runtime UNRUN (no Windows host).
4. Linux/macOS integrator: investigate both unchanged core2 process/signal races;
   rerun complete default/all-feature native gate without retry/threshold waivers.
   Linux runtime/no-spin/allocation execution UNRUN here (no Linux or Docker).
5. WASM lane: resolve inherited p3 getrandom compatibility and production WASI/web
   contracts; their required jobs/fan-in remain intact. Browser runtime UNRUN here
   (backend lane and known sandbox Chrome startup limits).
6. ci-fix3/integrator: full SQL/Mongo/server-runner jobs remain theirs. SQL tests
   UNRUN (sandbox shmget / mysqld initialization limits); full Docker protocol job
   UNRUN (no Docker). No server-harness or cleanup edits in this lane.
7. TLS owner: address upstream record allocations separately before claiming the
   whole TLS path obeys a zero-allocation-per-record budget.

No publication, commits, changes to other clones, additional dependencies or
soak/security-policy exceptions. Reviewable fixes are complete; the outstanding
items above prevent a claim that the full repository CI is green.

## Complete verification command ledger

- PASS (exit 0): `python3 scripts/ci/install-wasm-toolchain.py`; log `.tools/ci-fix4/1789407110615594000.log`.
- PASS (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/ci-fix4/1789407229916278000.log`.
- PASS (exit 0): `python3 -m unittest discover -s scripts/ci -p test_paths.py -v`; log `.tools/ci-fix4/1789407313868572000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --target wasm32-wasip2 -- -D warnings`; log `.tools/ci-fix4/1789407314292192000.log`.
- PASS (exit 0): `cargo fmt --all`; log `.tools/ci-fix4/1789407363794312000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --target wasm32-unknown-unknown -- -D warnings`; log `.tools/ci-fix4/1789407384689947000.log`.
- PASS (exit 0): `cargo test -p turnloop-http --test interop -- --nocapture --test-threads=1`; log `.tools/ci-fix4/1789407385915828000.log`.
- PASS (exit 0): `bash scripts/ci/install-wasmtime.sh`; log `.tools/ci-fix4/1789407416647395000.log`.
- PASS (exit 0): `cargo fmt --all`; log `.tools/ci-fix4/1789407540379382000.log`.
- FAIL (exit 101): `cargo test -p turnloop-tls --test portable`; log `.tools/ci-fix4/1789407541071487000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407570924511000.log`.
- FAIL (exit 1): `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2`; log `.tools/ci-fix4/1789407569740037000.log`.
- PASS (exit 0): `python3 -m unittest discover -s scripts/ci -p test_paths.py -v`; log `.tools/ci-fix4/1789407618057085000.log`.
- FAIL (exit 101): `cargo test -p turnloop-tls --test portable`; log `.tools/ci-fix4/1789407616899305000.log`.
- PASS (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/ci-fix4/1789407618618738000.log`.
- PASS (exit 0): `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck cargo-deny`; log `.tools/ci-fix4/1789407690491868000.log`.
- PASS (exit 0): `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v`; log `.tools/ci-fix4/1789407690475209000.log`.
- PASS (exit 0): `cargo +stable check --locked --workspace --all-targets --all-features`; log `.tools/ci-fix4/1789407690478495000.log`.
- PASS (exit 0): `python3 scripts/ci/lint-workflows.py`; log `.tools/ci-fix4/1789407747119442000.log`.
- PASS (exit 0): `bash scripts/ci/no-tokio.sh`; log `.tools/ci-fix4/1789407747108375000.log`.
- PASS (exit 0): `cargo deny --locked check`; log `.tools/ci-fix4/1789407749402117000.log`.
- FAIL (exit 101): `cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407747126135000.log`.
- PASS (exit 0): `python3 scripts/ci/soak.py`; log `.tools/ci-fix4/1789407747113782000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407785434759000.log`.
- PASS (exit 0): `cargo fmt --all`; log `.tools/ci-fix4/1789407816194670000.log`.
- PASS (exit 0): `cargo clippy -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --target x86_64-pc-windows-msvc --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407816681819000.log`.
- PASS (exit 0): `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop`; log `.tools/ci-fix4/1789407817342181000.log`.
- PASS (exit 0): `python3 -m unittest discover -s scripts/ci -p test_paths.py -v`; log `.tools/ci-fix4/1789407856210493000.log`.
- PASS (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/ci-fix4/1789407856627549000.log`.
- FAIL (exit 101): `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407858916648000.log`.
- PASS (exit 0): `cargo fmt --check`; log `.tools/ci-fix4/1789407876501949000.log`.
- PASS (exit 0): `git diff --check`; log `.tools/ci-fix4/1789407877089503000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407891982014000.log`.
- FAIL (exit 101): `cargo test --workspace -- --test-threads=1`; log `.tools/ci-fix4/1789407890795465000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407893177605000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789407927205151000.log`.
- PASS (exit 0): `cargo fmt --all`; log `.tools/ci-fix4/1789407947053317000.log`.
- PASS (exit 0): `cargo test -p turnloop-tls --test portable`; log `.tools/ci-fix4/1789407947569113000.log`.
- PASS (exit 0): `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v`; log `.tools/ci-fix4/1789407970049827000.log`.
- PASS (exit 0): `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2`; log `.tools/ci-fix4/1789407968791800000.log`.
- PASS (exit 0): `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3`; log `.tools/ci-fix4/1789407978545291000.log`.
- PASS (exit 0): `python3 -m unittest discover -s scripts/ci -p test_paths.py -v`; log `.tools/ci-fix4/1789408010252153000.log`.
- PASS (exit 0): `cargo test --workspace -- --test-threads=1`; log `.tools/ci-fix4/1789408018254092000.log`.
- PASS (exit 0): `cargo fmt --all`; log `.tools/ci-fix4/1789408067998438000.log`.
- PASS (exit 0): `python3 -m unittest discover -s scripts/ci -p test_paths.py -v`; log `.tools/ci-fix4/1789408143310554000.log`.
- PASS (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/ci-fix4/1789408143925710000.log`.
- FAIL (exit 1): `python3 scripts/ci/run-tests.py native`; log `.tools/ci-fix4/1789408068446759000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789408198789189000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --target wasm32-wasip2 -- -D warnings`; log `.tools/ci-fix4/1789408199413988000.log`.
- PASS (exit 0): `cargo clippy --workspace --all-targets --target wasm32-unknown-unknown -- -D warnings`; log `.tools/ci-fix4/1789408214449688000.log`.
- PASS (exit 0): `cargo +nightly-2026-09-07 clippy -p turnloop-tls --all-targets --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks`; log `.tools/ci-fix4/1789408234418019000.log`.
- PASS (exit 0): `cargo +stable check --locked --workspace --all-targets --all-features`; log `.tools/ci-fix4/1789408240177860000.log`.

- PASS (exit 0): `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v`; log `.tools/ci-fix4/1789408347389951000.log`.

- PASS (exit 0): `python3 scripts/ci/check-paths.py`; log `.tools/ci-fix4/1789408356537298000.log`.

- PASS (exit 0): `cargo fmt --check`; log `.tools/ci-fix4/1789408358752237000.log`.

- PASS (exit 0): `git diff --check`; log `.tools/ci-fix4/1789408359229651000.log`.

- PASS: read-only Python/subprocess assertions using `git diff 59593ee --` confirm Cargo.lock, root dependencies/config, soak policy, protocol runner and server harness are unchanged; added HTTP/TLS lines contain no `.unwrap()`; both supplied documentation logs are absent.

- PASS (exit 0): `git diff --check`; log `.tools/ci-fix4/1789408519199562000.log`.
