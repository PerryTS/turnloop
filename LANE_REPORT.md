# core2 lane report

Updated 2026-09-14. Implementation in progress; no completion claim yet.
Wave-1 history remains in docs/INTEGRATION_REPORT.md and docs/lanes/.

Read completely: DESIGN.md, CONTRIBUTING.md, integration report, core/Windows/WASM
lane reports. No applicable AGENTS.md. Git metadata remains read-only.

## Scope and implementation

Remaining native surface: local IPC/stdio/SCM_RIGHTS, processes, process-wide
signals, TTY, external waits, optional local executor, shared contracts and rustdoc.
The starting tree already uses Backend revision 2 for empty-wait instrumentation.
Preserve all no-spin, exactly-once, allocation and dependency gates.

## Verification

Implementation checks are pending. Linux/Windows runtime tests are UNRUN (no hosts).
WASI/web native-only operations must explicitly return Unsupported; their common
core and executor will be cross-checked. The seven-day dependency soak remains on.
Exact verification invocations and outcomes will be in docs/core2-commands.md.

## Decisions / questions

Portable API additions will use shared types and optional backend methods. Process
groups map to Windows Job Objects; received transports use existing attach semantics.
No DESIGN.md change has been made. Native close must reap owned children.

## Next steps

Implement and verify each surface, extend allocation/contract gates, document API
and platform limitations, then run the full requested verification matrix.
