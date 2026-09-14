# wasm2 lane report

Work in progress, 2026-09-14. Read DESIGN.md draft 0.3, CONTRIBUTING.md,
INTEGRATION_REPORT.md, and the wasm/core/CI reports. No AGENTS.md applies.

The checked-in Backend has revision-1 hooks and revision-2 empty-wait counters;
this lane preserves both. Implementing WASI 0.2, experimental WASI 0.3 and web,
their shared contracts and required CI jobs. The seven-day soak and runtime bans
remain unchanged. Git is read-only; the integrator commits.

## Implemented

Initial inspection complete. Existing spikes are reference material, not evidence
that production contracts passed. Verification ledger: `.tools/wasm2/commands.jsonl`;
final commands and results will be copied here.

## Verification

- PASS: read-only inventory, clean initial git status, required document/source reads.
- UNRUN: implementation gates, until the new backends and contracts exist.
- UNRUN: Linux/Windows runtime, unavailable hosts.

## Deviations / questions

- p2 requires scoped canonical ABI return storage in addition to reusable input
  pollable lists; generated poll/read bindings allocate.
- p3 needs an audited persistent wait-set provider; investigating the bindings'
  task registration ABI before choosing the experimental implementation.
- Web host JS allocations must be distinguished from Rust steady-state storage.

## Next steps

Implement and test all three adapters; replace pending wasm jobs; run strict
quality gates and workflow lints; record platform skips with exact reasons.
