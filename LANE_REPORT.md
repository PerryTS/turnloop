# proto-fix1 lane report

In progress, based on `fcea55ee9f876dbc8ffaa3c9341068e3aa3a75d3`. No commits;
Git metadata is read-only. Read DESIGN.md and CONTRIBUTING.md completely, the
integration report and relevant MongoDB, SQL, KV, HTTP, core, CI and WASM reports.
No applicable AGENTS.md.

## Diagnosis

**Cross-thread allocator contamination (candidate a).** Retrieved CI run
34886441405/job 104118674676: the failing workspace command uses
`--all-features -- --test-threads=1`. One test thread still runs concurrently
with libtest's main/receiver thread.

The pinned Rust source calls `run_test`, then `rx.recv()` (library/test/src/lib.rs
lines 443–445). The first blocking receive initializes an `Arc<Context::Inner>`
(std/sync/mpmc/context.rs:72) and grows `Waker::selectors`
(std/sync/mpmc/waker.rs:49). Both can occur after MongoDB's two warm-up rounds
have enabled its process-wide counter. This explains the first-mode-only failure.

A controlled local reproduction runs the unchanged MongoDB workload on a worker
and lets the receiving thread enter its first `recv` only after counting starts.
Allocation backtraces identify **48-byte context + 96-byte selector storage**, both
with `measuring=false`. macOS additionally allocates a 64-byte pthread mutex in
`SyncWaker::register`; Linux uses an inline futex mutex (std/sys/sync/mutex/mod.rs),
so the two shared allocation sites match the reported Linux count. **The Linux
attribution is source-based; Linux runtime remains UNRUN.** No measured-thread
allocation was observed. Probe artifacts/logs: `.tools/proto-fix1/channel-repro-final.rs`
and `channel-cargo-repro*.log`. The controlled old-gate failure is intentional.
An unforced instrumented libtest run passed 300/300 fresh processes on macOS;
absence of a natural local reproduction is not presented as a fix.

`cargo tree --locked -e features -p turnloop-mongodb` and its `--all-features`
variant are byte-identical. Full workspace metadata comparison (matched by package
ID) changes features only for turnloop, contract, bench, HTTP and zstd-decoder;
MongoDB has no path to these packages. BSON/rand/flate2 and all MongoDB dependencies
have identical features. No all-feature helper pool, lazy BSON generator or
capacity-growth path is implicated. The measured path uses retained buffers and
borrowed raw BSON; setup/authentication/topology/entropy are outside that path.

## Implemented / allocation audit

- MongoDB: const-initialized per-thread Cells and a synchronous scoped counter,
  including alloc, alloc_zeroed and realloc, with panic cleanup. Every measured
  command asserts its thread's counter is active. The exact two warm-ups, four
  modes, 1,000 measured commands / 2,000 rows per mode and **== 0** remain.
- Added exact positive calibration for all three allocation entry points, an
  overlapping two-thread regression (worker must count exactly two, owner zero),
  and a regression proving cleanup and later counting after a measured panic.
- PostgreSQL/MySQL `tests/support/allocation.rs`, Redis/SMTP
  `tests/support/alloc.rs`: already per-thread, destructor-safe, synchronous
  counters; audited and left unchanged. Their workload/result assertions remain.
- HTTP and zstd-decoder: already per-thread on native; extend the same TLS to
  browser/WASI p2 and threaded wasm. Retain positive allocation calibration and
  every existing workload/threshold. Use `try_with` during allocator callbacks.
- Core contracts: native already per-thread; extend that selection to WASI p2.
  **Only the counter preamble changed; every workload body, including all UDP
  cancellation tests owned by core5, is byte-for-byte unchanged.**
- Web contracts: convert global atomics to per-thread Cells and add a real-allocation
  calibration that executes in both browser and Node test binaries.
- WASI p3's existing single-agent static counters in HTTP/decoder/core remain
  deliberately isolated behind exact `wasi/p3` cfg. They must work before p3's
  task-local area exists during canonical ABI allocation callbacks. There is no
  concurrent OS thread in these components; broad `wasm32`/`wasi` static selection
  is removed. Runtime verification is pending below.
- No product/backend, dependency, lockfile, soak/no-tokio policy, instruction budget,
  no-spin limit or existing allocation assertion changed.

## Verification

Commands and full output are recorded in `.tools/proto-fix1/commands.jsonl` and
named logs; each repetition also records its actual executable, exit and result.
Final tracked command ledger/counts will replace this progress section.

- Feature trees and workspace-feature audit: PASS.
- Controlled old-gate contamination probe: expected FAIL with the stacks above.
- Natural original instrumented libtest probe: PASS 300/300, serial harness.
- Initial fixed MongoDB suite: PASS 3 tests; panic cleanup test added afterward.
- Initial all-feature Clippy: FAIL (safety comment moved above a formatted assert);
  corrected by binding the documented unsafe slice before the assertion.
- Final native Clippy, >=300 MongoDB runs per mode, full workspace default and
  three all-feature runs, cross-target checks, soak/no-tokio: running/pending.

## Deviations / proposed DESIGN changes

No DESIGN changes. WASI p3's single-agent exception is required by the existing
ABI initialization constraint; it cannot suffer the native multi-thread hazard.
No test or gate is weakened.

## Open questions / next steps

Finish the repetition and platform verification ledger, including actual workload
counts. Linux/Windows runtime is UNRUN (no host); SQL real-server tests are UNRUN
(sandbox). The integrator must run native Linux/Windows and commit the tree.
