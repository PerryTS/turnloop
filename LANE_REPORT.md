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
