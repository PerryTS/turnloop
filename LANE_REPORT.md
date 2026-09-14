# core4 lane report

Starting revision: b78d533. Scope: Linux fallback wait accounting, benchmark-only
BTree timer isolation, and required per-mode native CI. Read DESIGN.md,
CONTRIBUTING.md, docs/INTEGRATION_REPORT.md and core/CI lane reports completely;
no applicable AGENTS.md. The previous root report is preserved under
`.tools/core4/inherited-core3-report.md` and in Git history. No commits: .git is read-only.

## Findings and implementation (in progress)

- The reported line 516 is actually `info.zero_event_waits`, not `info.os_waits`
  (line 515). Both epoll wait branches already return `waits: 1`. timerfd-only
  expiry returns a private TIMER event, making the raw `n == 0` counter false.
  This is an accounting defect; the examined timeout path arms a relative
  nanosecond one-shot and calls epoll_wait(-1) once.
- Planned fix counts a timerfd-only expiry like an epoll_pwait2 timeout; notifier
  and I/O readiness still count as events. Existing deadline/allocation gates
  stay unchanged. Additional wait accounting and mode-selection tests forthcoming.
- BTree comparison will move wholly to turnloop-bench. Production remains the heap.
- CI will explicitly select Linux default, timerfd, SIGCHLD and combined fallback
  modes, plus executor and all-feature arms; macOS/Windows get applicable modes.
  A manifest/matrix coverage gate will reject unexercised new public core features.

## Verification

Runtime verification has not started. Linux/Windows runtime is UNRUN (no host).
Exact final commands/results and Linux integrator commands will be added here.
Read-only source/manifest/workflow/status inspection: PASS. Initial directory scan
was overbroad; a zsh shell-loop variable temporarily shadowed PATH in that process
and a glob for a nonexistent services test failed; subsequent reads used discovered
paths. These were inspection failures, not verification passes.

## Deviations / proposed DESIGN clarification

Clarify wait instrumentation: native events exclude the backend's private timeout
mechanism; a timeout-only timerfd event equals a zero-event timed OS wait. Wake
notifications remain native events. No behavioral/timing/allocation limit changes.
No dependency, lockfile, soak or baseline change is planned.

## Open questions / next steps

Implement and validate locally, then run the exact per-mode tests on Linux x86_64
and arm64 via the integrator. Linux runtime cannot be claimed from cross-Clippy.
