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

## Restart: authoritative draft 0.3

Read /Users/amlug/projects/perry/windlass/DESIGN.md draft 0.3 completely and the
local LANES.md. The user supersedes its private-repository note: public full PR
matrix, zero tokio without any exception. Removed all earlier h2 exception code.

Implemented so far: main/PR matrix, WASI 0.2/0.3, browser/Node entry points,
metadata-driven test selection, positive execution counts, loom/Miri gates,
seven-day locked-version audit, all-target dependency bans, three-round Callgrind
baseline gate, authenticated exact-SHA CI release checks, protected OIDC publish
workflow, dependency-ordered Cargo workspace publication, per-crate tags/releases,
release-plz PR generation, SQL TLS/SCRAM, Redis cluster and Mongo replica fixtures.

New local findings:
- PASS: five action SHA pins re-verified against their official GitHub commit APIs.
- PASS: `shellcheck scripts/ci/*.sh` using checksum-pinned ShellCheck 0.11.0.
- FAIL: raw `.tools/bin/actionlint -color`: v1.7.12 (latest official release)
  rejects `concurrency.queue`, a GitHub feature released May 2026. Both workflows
  retain the mandatory queue behavior. `lint-workflows.py` validates exact queue
  and cancel expressions before filtering only this unsupported-key diagnostic;
  every other diagnostic remains fatal. This is an explicit tooling compatibility
  limitation, not a claim that the raw linter passed.
- FAIL: initial `zizmor --offline --min-severity low .github/workflows` flags
  workflow_run generically. Added a specific annotated justification: release
  consumes only successful same-repository main pushes, checks exact SHA/job via
  API and consumes no untrusted PR artifacts/caches. Recheck pending.
- PASS: official upstream iai-callgrind v0.16.1 summary v6 schema and release-plz
  v0.3.167 implementation inspected. Cargo workspace publication is chosen over
  release-plz's per-package publisher so all unpublished siblings can be staged
  together during preflight, without disabling the dependency soak.
