# Windows lane report

Branch: `lane/windows`. Host: macOS arm64. Windows runtime validation is unavailable.

## Implemented

- Read DESIGN.md draft 0.2 and LANES.md completely; no AGENTS.md found.
- Standalone `spikes/iocp` crate, windows-sys only, separate workspace.
- One IOCP per loop, reserved synthetic keys, stack batch storage, one GQCSEx wait,
  per-entry NTSTATUS, bounded timeout/APC outcomes, cross-thread wake/isolation test.

## Verification ledger

- PASS `rustc --version`: nightly 1.100.0 (2026-08-19), pinned nightly-2026-08-20.
- PASS `cargo +stable --version`: cargo 1.97.1 (environment differs from stated 1.98).
- PASS `git -C ../core tag -l 'trait-v*'`: no tags at initial inspection.
- UNRUN `cargo test --manifest-path spikes/iocp/Cargo.toml --target x86_64-pc-windows-msvc`:
  no Windows machine or executable runner. Cross-checking does not run tests.

## Deviations / proposed specification changes

- None yet. Timer and transfer constraints are under investigation.

## Integrator questions

- Awaiting `trait-v0`; no core files modified.

## Next steps

- Implement both timers, TCP, pipes, stdio, GUI helper, processes and console probes.
- Review compio-driver source; adapt to core trait when tagged; write contract plan.
- PASS `cargo fmt --manifest-path spikes/iocp/Cargo.toml`.
- PASS `cargo check --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc`.
- FAIL first `cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc -- -D warnings`: `not_unsafe_ptr_arg_deref` on raw ownership constructor. Fixed by requiring an unsafe caller contract.
- PASS repeat of that clippy command after the fix.
- PASS `cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc`.

## Environment blocker: commits

- FAIL `git add spikes/iocp LANE_REPORT.md && git commit -m 'Add standalone IOCP wait and wake probe'`:
  `fatal: Unable to create '.../windows/.git/index.lock': Operation not permitted`.
  Session filesystem permissions explicitly make `.git` read-only; escalation is disabled.
  Working files remain reviewable in this clone. Subsequent commit steps are UNRUN for
  the same reason; no permission boundary will be bypassed.

## Spike progress: timers, TCP, pipes and stdio

- Both timer paths implemented, 100-expiration precision tests each, APC cancellation,
  NT packet rearm and dynamic support detection. Precision measurements UNRUN.
- TCP AcceptEx/ConnectEx with context updates, IFS-provider check, skip-success,
  zero-byte idle receive then nonblocking recv, 128 echo iterations, cancellation
  packet and drain-before-close ordering. Op storage/events allocated per fixture
  and reused, including error-path cancellation waits.
- Named-pipe server/client: pending-connect and client-first ERROR_PIPE_CONNECTED
  branches, 128 transfers each, cancel/drain/close. Synchronous stdio reader thread
  posts byte counts and EOF; test checks actual payload.
- PASS repeated fmt, Windows all-target clippy, stable Windows all-target check
  commands listed above after timers, and after TCP/pipes/stdio.
- FAIL intermediate clippy after TCP alone: `Pipe` and `Operation::data` unused;
  resolved by completing the planned named-pipe implementation, without lint exemptions.
- PASS repeated `git -C ../core tag -l 'trait-v*'`: still no tags.
- UNRUN commit timer/TCP/pipe steps: read-only `.git` blocker above.
- UNRUN all new Windows test binaries: no Windows runner.

Specification findings: APCs target the thread that arms the timer; GUI helper mode
must use NT packets or arrange arming on the helper. Nt* packet APIs are documented
Microsoft devnotes with no SDK header/import library and no listed minimum OS;
detect exports. CancelIoEx is not a completion barrier. IOCP association is permanent
until handle close, so generic detach/attach cannot simply reassociate a handle.

## Spike progress: GUI, child process and console

- Opt-in GUI helper is sole GQCSEx consumer, bounded queue/backpressure, partial-drain
  event rearm and shutdown. Tests exercise 1024 forwarded packets and a timer.
- CreateProcessW suspended creation, explicit inherited stdio handle list, parent
  overlapped/child synchronous named-pipe ends, Job Object assignment before resume,
  one-shot exit callback and joining unregister. Tests verify child stdin/stdout/
  stderr, exit code 23 and registration after the child has already exited.
- Console dispatcher maps Ctrl-C/Ctrl-Break/Ctrl-Close, protects port lifetime against
  active handler threads. Child with CREATE_NEW_CONSOLE generates real C/BREAK events;
  CTRL_CLOSE mapping is checked but OS close event is not generated (terminates process).
- PASS fmt and Windows all-target clippy after GUI helper, process and console.
- PASS stable Windows all-target check after process and console; PASS stable host
  all-target check (Windows code is cfg-excluded on macOS; this is not runtime coverage).
- FAIL initial process clippy compilation: windows-sys WAITORTIMERCALLBACK uses `bool`
  rather than `u8`; corrected callback signature, subsequent clippy PASS.
- UNRUN GUI/process/console runtime tests and commits for blockers above.
- PASS repeated core tag check after console: no trait-v0 yet.

Additional spec changes proposed: document child stdio endpoints as synchronous and
parent endpoints as overlapped; ordinary children do not issue overlapped stdio I/O.
CTRL_CLOSE can only be best-effort HUP: OS terminates after handler returns/timeout,
so D1's asynchronous delivery cannot promise host cleanup for console close.

## Step 2: compio evaluation

- Implemented `spikes/iocp/EVALUATION.md`, reviewing published 0.12.5 and exact source
  commit. Recommendation: own implementation, borrow techniques. Public submission
  allocates ThinCell/Box per operation; IOCP poll allocates a Vec each call. Token
  pull delivery/cancel terminal events and GUI/timer behavior require adaptation.
- PASS source retrieval from official crate distribution, source SHA recorded in
  evaluation, and inspection of IOCP, keys, socket ops, wait backends and thin-cell.
- PASS fmt --check and Windows all-target clippy before the planned evaluation commit.
- UNRUN evaluation commit: `.git` read-only.
- PASS core tag recheck after evaluation: no trait-v0. Proceed with DESIGN §6-based
  draft, as authorized. No fetch/merge attempted while no tag exists.
