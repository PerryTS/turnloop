# proto-fix1 lane report

In progress, based on `fcea55ee9f876dbc8ffaa3c9341068e3aa3a75d3`.

Read DESIGN.md and CONTRIBUTING.md completely, the integration report and relevant
MongoDB, SQL, KV, HTTP, core and WASM lane reports. No applicable AGENTS.md.

## Findings / implemented

- MongoDB's allocation test uses process-wide AtomicBool/AtomicUsize instrumentation.
  Its workload executes synchronously on the test thread.
- PostgreSQL, MySQL, Redis, SMTP and native HTTP/decoder/core gates already use TLS.
  Auditing WASI single-agent and browser counters and tracing libtest overlap.
- MongoDB package default/all-feature feature trees are byte-identical (PASS).
- No product, dependency, soak, backend, no-spin or UDP cancellation body changes.

## Verification

Detailed command output and a machine-readable command ledger are being kept in
`.tools/proto-fix1/`. Final tracked command ledger and counts will be included here.
Required >=300 fresh MongoDB allocation executions in each mode and three complete
all-feature workspace runs: UNRUN (pending implementation).

## Deviations / proposed DESIGN changes

None. Allocation assertions remain unchanged.

## Open questions / next steps

Trace the source of the reported two allocations; implement and prove thread
isolation; complete all checks and repetition counts. Linux/Windows runtime is
UNRUN (no host); SQL real-server tests remain UNRUN (sandbox).
