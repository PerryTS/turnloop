# ci-fix4 lane report

Updated 2026-09-14. Work in progress, based on main 59593ee. Read DESIGN.md,
CONTRIBUTING.md, docs/INTEGRATION_REPORT.md and the CI/HTTP/Windows/WASM lane reports
completely. No applicable AGENTS.md. Git metadata is read-only; integrator commits.

## Scope and evidence

- Add an independent Git case/path guard with adversarial synthetic tests, CI and
  local verification wiring.
- CI run 34874044440 WASI stderr: cc-rs cannot find `llvm-ar`. Preserve ring and
  install checksum-pinned wasi-sdk 34 (clang, llvm-ar and sysroot); execute TLS
  under Wasmtime as well as cross-Clippy. No dependency/provider/soak change.
- Windows curl lacks HTTP2. Detect `curl -V` capabilities, require Node HTTP/2,
  assert executed client legs and report them. Review other HTTP/WebSocket tests.
- ci-fix3 owns protocol runner/Postgres/Mongo cleanup; those files are untouched.
  Production WASI/web jobs and backends belong to the wasm lane.

## Verification

Read-only source/log/status inspections PASS. Implementation checks pending.
Every build/test/check invocation will be recorded below with PASS/FAIL/UNRUN.

## Deviations / DESIGN proposals

None. Runtime code, no-spin rules, allocation thresholds, runtime bans, seven-day
soak and the existing exact rustls security exception remain unchanged.

## Open questions / next steps

Implement guards/toolchain/client detection; run native and affected cross-target
checks, Wasmtime protocols and automation tests. Windows/Linux runtime UNRUN (no
hosts); SQL runtime UNRUN (sandbox). Record exact CI follow-ups and Windows exclusions.

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

## Current findings (pending final verification)

- Pin is wasi-sdk 34.0, released 2026-08-25. Linux x86_64 SHA-256
  `b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4`;
  macOS arm64 `9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9`.
  Official release/API digests verified. The local installer passed real wasm
  object/archive probes for p2/p3/web. CI backend jobs receive only this shared
  prerequisite; no backend gate or runner semantics changed.
- Both requested whole-workspace wasm cross-Clippy commands pass. Native HTTP
  interop passes 7 tests (2 fixture tests ignored in that invocation), including
  real mandatory Node 100-stream execution with curl HTTP2 disabled.
- The new TLS memory harness originally overwrote successive handshake output
  chunks; native and WASI tests caught it. Fixed by retaining all chunks until
  TransmitTlsData and only acknowledging after transfer. No assertion relaxed.
- The additional TLS allocation experiment found **4000 allocations / 1000
  bidirectional record exchanges** in pre-existing rustls 0.23.45, despite warm-up.
  Upstream ReadTraffic owns `Option<Vec<u8>>`; incoming plaintext is queued as
  owned chunks and encryption also allocates. This is not introduced by the CI
  fixes. User scope clarification is pending; the strict new experiment currently
  fails while handshake/ALPN/resumption/certificate/deadline checks pass.
- Integrator checkpoint 648efb7 appeared externally. This agent did not commit.

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
