# core3 lane report

Work in progress. Scope: idle instruction regression, child registration ESRCH,
and signal fan-out/unsubscribe races. Starting revision: 9e9d8aa.

Read DESIGN.md and CONTRIBUTING.md completely, integration report, core/CI lane
reports and core2 command history. No applicable AGENTS.md. The inherited ci-fix4
report is preserved in `.tools/core3/inherited-ci-fix4-report.md` and Git history.

## Findings and implementation

- Core2 added repeated full-capacity scans in Services::has_work/poll and
  Files::has_work/start on every idle turn. Removing these with readiness queues.
- Existing child registration calls try_wait once on ESRCH and incorrectly fails
  if wait status is not yet visible. Deterministic coverage will force this gap.
- Investigating kqueue signal notification before ordinary handler delivery and
  restoration of the default disposition after the last fan-out recipient stops.
- Five pre-change macOS ri_instructions rounds captured at release cgu=1. The
  existing harness already excludes initialization and warms up 100 turns.

## Verification

See [complete command ledger](docs/core3-commands.md). Required native/cross-target
Clippy, stable, workspace tests, 50 contract repetitions, 10 all-feature workspace
repetitions, loom, Miri, no-tokio and soak are pending. Linux Callgrind and native
Linux/Windows runtimes are UNRUN (no host). No instruction baseline or gate changed.

## Deviations / questions / next steps

No dependency changes, soak overrides or DESIGN changes. Implement event-driven
service/file scheduling, deterministic race regressions and stress tests, then
measure before/after and run all required verification. Integrator commits the
working tree because .git is read-only.
