# CI lane report

Status: implementation in progress. Read DESIGN.md draft 0.2 and LANES.md fully.
Only the CI clone is modified; other lanes are inspected read-only. No commits,
pushes, remotes, repository creation or crates.io mutations are performed.
`.git` is read-only; the integrator must commit this working tree.

## Findings and integration dependencies

- This clone has no Cargo workspace yet. All package selection will use Cargo
  metadata, including the windlass → turnloop rename and future protocol crates.
- WASI p3 requires nightly-2026-09-07 and Wasmtime 46.0.0 per the WASM report.
- Core currently supplies five loom models, but has not marked Miri-compatible
  tests or supplied iai-callgrind targets/committed Linux baselines. Those jobs
  must fail explicitly until the integration supplies them, never pass with zero tests.
- SQL and Mongo reports/test harnesses are absent at initial inspection; HTTP
  and KV are in progress. A documented TURNLOOP_TEST_* convention is needed.
- Cargo's min-publish-age does not reject a too-young already-locked dependency;
  an independent registry timestamp check is needed in addition to locked resolution.
- GitHub concurrency defaults replace pending runs even with cancellation false;
  main needs queue: max. GitHub limits that queue to 100 pending runs.

## Verification

- PASS: read-only source, manifest, lane report and tool help inspection.
- PASS: `cargo +nightly-2026-08-20 publish --help` confirms workspace publication
  support; `python3 --version` is 3.14.6. ShellCheck/actionlint are on PATH, but
  the requested checksum-pinned local binaries will also be installed.
- UNRUN: all workflow executions, service containers, Linux/Windows runtime
  tests, release environment protection and OIDC (no GitHub run or target hosts).

## Next steps

Implement workflows, scripts and release documentation; validate scripts and
workflow linters locally; record exact commands and outcomes before handoff.
