# wasm2 lane report

Work in progress, 2026-09-14. Read DESIGN.md draft 0.3, CONTRIBUTING.md,
INTEGRATION_REPORT.md, and the wasm/core/CI reports. No AGENTS.md applies.

The checked-in Backend has revision-1 hooks and revision-2 empty-wait counters;
this lane preserves both. Implementing WASI 0.2, experimental WASI 0.3 and web,
their shared contracts and required CI jobs. The seven-day soak and runtime bans
remain unchanged. Git is read-only; the integrator commits.

## Implemented

WASI 0.2 adapter implemented, including raw canonical poll/read/UDP storage.
WASI 0.3 direct async wait-set and socket adapter implemented behind
`wasi-p3-experimental`; runtime validation in progress. Web HostCallback adapter
and shared browser/Node scenarios implemented; worker/CI integration in progress.
Existing spikes remain reference material, not production verification. Verification ledger: `.tools/wasm2/commands.jsonl`;
final commands and results will be copied here.

## Verification

- PASS: read-only inventory, clean initial git status, required document/source reads.
- PASS: p2 strict Clippy; existing allocation gates (zero allocations for read,
  write, timer and accept); shared no-spin, TCP 1/64, UDP, cancel/close, liveness,
  pooled backpressure and capacity/stale IDs.
- FAIL: full p2 shared suite: strict timer precision median 1.07875 ms vs <500 us.
  The gate remains unchanged. Separate coverage run found a writev segment flush
  bug; fixed, and its 512 KiB write/shutdown scenario now PASS.
- PASS: web strict Clippy (core and contract); p3 strict Clippy and bounded wait.
- PASS: pinned Wasmtime 46 installation with official checksum; wasm-bindgen CLI
  0.2.108 built as a separate normal Cargo project under unchanged seven-day soak.
- UNRUN: remaining full quality gates, browser and Worker contracts, pending work.
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
