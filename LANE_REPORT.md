# adapters-db — wave 3 part 2

Status: implementation in progress from c506775. No commits; the integrator owns
the read-only Git metadata. DESIGN.md, CONTRIBUTING.md, the integration report,+adapters-net and relevant SQL/KV/MongoDB/core/Windows/WASM/CI reports read.

## Implemented

- Extending the shared turnloop-io driver with whole-operation cancellation
  guards, output-only flushing and validated TLS transition access.

## Verification

No implementation verification has run yet. All quality gates and real-server
tests are UNRUN until recorded below. Commands and raw logs will be retained in
`.tools/adapters-db/`, with a committed command ledger.

## Deviations / proposed DESIGN changes

None. Zero-tokio, seven-day dependency soak and existing gates remain unchanged.

## Open questions and next steps

- Implement clients, SQL/CMAP pools, Redis routing/reconnect/Sentinel and MongoDB
  monitoring/retries/cursors using shared transport glue.
- Add meaningful cancellation, real-server, allocation and no-spin tests, examples,
  READMEs and required protocol/protocol-wasi CI metadata.
- Run native and cross-target gates. PostgreSQL/MySQL server execution is UNRUN
  (sandbox: shmget / initializer crash); Linux/Windows runtime requires integrator.
- Inherited limits to verify: absent Windows Platform provider; WASI resolver
  Unsupported; wasm miniz_oxide reset allocation.
