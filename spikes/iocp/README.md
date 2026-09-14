# IOCP mechanism spike

Standalone workspace, windows-sys 0.61.2 only. No tokio or compio dependency.
Requires Windows 10 1803+ for the high-resolution timer probes. NT packet probes
feature-detect ntdll exports and fail explicitly if unavailable. All Windows
runtime tests are **UNRUN** on the development macOS host.

Each mechanism has its own Cargo integration-test binary:

| Binary | Subject |
| --- | --- |
| `port` | Per-loop IOCP, bounded GQCSEx, reserved-key cross-thread wake/isolation |
| `timer` | High-resolution APC and NT-packet timers, precision samples, cancel/rearm |
| `tcp` | AcceptEx, ConnectEx, zero-byte receive, nonblocking read, send, skip-success, cancellation and close |
| `pipe` | Overlapped named-pipe server/client, both connect orderings and transfer |
| `stdio` | Synchronous pipe reader thread posting actual bytes and EOF |
| `integration` | Sole-consumer helper, bounded queue, auto-reset event and MsgWait consumer |
| `process` | Child stdio, job, exit wait, early exit and live descendant termination |
| `console` | Isolated real Ctrl-C/Ctrl-Break, mapping and callback lifetime |
| `draft` | Preallocated backend, provided I/O, allocation counter, notifier, close order |
| `handles` | Port churn with handle-count baseline and completion counts |

```sh
cargo fmt --manifest-path spikes/iocp/Cargo.toml -- --check
cargo check --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc
cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc -- -D warnings
cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets --target x86_64-pc-windows-msvc
```

On Windows, run the tests in [CONTRACT_TEST_PLAN.md](CONTRACT_TEST_PLAN.md). The
test helper executables are spawned by their tests. Non-Windows check builds only
the portable harness surfaces and does not validate any Windows mechanism.

See [EVALUATION.md](EVALUATION.md) for source evidence and timer support status,
[backend_draft/README.md](backend_draft/README.md) for exact core adaptation work,
and [../../LANE_REPORT.md](../../LANE_REPORT.md) for verification results/blockers.
Every subtle Win32 lifetime/notification rule is cited beside the implementing code.
