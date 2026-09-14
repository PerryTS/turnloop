# Windows runtime results

Issue #4, branch `windows/iocp-backend`, 2026-09-14.

## Environment

- Windows 11 Pro, version 10.0.26200, build 26200.
- GMKtec NucBox M6 Ultra; AMD Ryzen 5 7640HS with Radeon 760M.
- Physical-machine SMBIOS identification; Windows reports a hypervisor present
  (Hyper-V/VBS). This is not evidence of a guest VM; precise hypervisor/VBS
  configuration has not been independently established.
- Native x86_64 MSVC build and execution, from the interactive desktop session.
- `rustc +nightly-2026-08-20 --version`: 1.100.0-nightly
  (f7d782a3b 2026-08-19). MSVC linker and Windows SDK are installed and linked tests.
- `rustc +stable --version`: 1.96.1 (31fca3adb 2026-06-26).
- Node v24.21.0 initially installed; HTTP/2 interop is not yet run.
- Commands explicitly select the pinned nightly: implicit `cargo`/`rustc`
  attempts to install every cross target from `rust-toolchain.toml`, but the
  existing wasip1-threads component has a rustup installation conflict.

## Phase 1 baseline

Each row ran natively with
`cargo +nightly-2026-08-20 test --manifest-path spikes/iocp/Cargo.toml --test NAME -- --nocapture --test-threads=1`.

| Binary | Initial result | Tests |
| --- | --- | --- |
| port | PASS | 1/1 |
| timer | FAIL | 2/3; APC arm returns Win32 error 87 |
| tcp | PASS | 1/1; 1,024 echoed bytes, cancellation drained |
| pipe | PASS | 2/2, both connection orderings |
| stdio | PASS | 1/1, bytes and EOF |
| integration | PASS | 2/2, GUI wait and full-queue shutdown |
| process | PASS | 3/3, including live child and grandchild termination |
| console | PASS | 1/1, real events in isolated child console |
| draft | FAIL | 1/3; both direct turns fail at APC timer arm |
| handles | PASS | 1/1, 128 port lifetimes |

`cargo +nightly-2026-08-20 test --manifest-path spikes/iocp/Cargo.toml --all-targets -- --nocapture --test-threads=1`
also ran, but stopped at the draft failures. The separate binary runs above
ensure later binaries were actually executed.

PASS:

- `cargo +nightly-2026-08-20 clippy --manifest-path spikes/iocp/Cargo.toml --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks`
- `cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets`

## Timer decision (DESIGN.md section 15 question 3)

Choose a high-resolution waitable timer associated with IOCP through dynamically
resolved `NtAssociateWaitCompletionPacket`, and use nonalertable port waits.

| Route | Requested delay | Samples | p50 lateness | p95 lateness | Maximum lateness |
| --- | --- | --- | --- | --- | --- |
| High-resolution timer + APC | 250 us | 0 | UNMEASURED | UNMEASURED | UNMEASURED |
| High-resolution timer + NT packet | 250 us | 100 | 275.2 us | 285.8 us | 380.9 us |

The APC probe fails in `SetWaitableTimer` with `ERROR_INVALID_PARAMETER` (87).
An independent native API check confirmed the combination: ordinary timers accept
APC callbacks; high-resolution timers accept a null callback but reject a non-null
callback, for both auto-reset and manual-reset creation. Downgrading to an ordinary
timer would not validate the required high-resolution APC route. The original
precision assertion remains unchanged and its result remains FAIL.

The NT probe additionally passed 100 cancel/rearm cycles. Its measured precision
passes the existing bounds without changing the system timer period. Nonalertable
waits also preserve the rule that turns do not dispatch arbitrary host APCs.
Microsoft documents the packet APIs but does not specify a minimum OS version;
missing exports must produce an explicit capability error. This single host does
not establish the minimum-version or VM precision matrix.

## Phase 2

After replacing the draft's APC deadline with the measured NT packet route,
`cargo +nightly-2026-08-20 test --manifest-path spikes/iocp/Cargo.toml --test draft -- --nocapture --test-threads=1`
passes all three tests, including zero allocations across 256 measured transfers
and cancellation before Closed. Formatting and unsafe-aware Clippy pass again.

The production adapter is integrated at `crates/turnloop/src/backend/iocp/`.
The Windows pending-contract marker is removed: shared contracts execute natively.

PASS on this host:

- `cargo +nightly-2026-08-20 test -p turnloop-contract --all-features -- --test-threads=1`:
  47 tests: 30 shared/backend, five allocation, six executor, two isolated-console
  and four Windows lifetime/imported-handle scenarios.
- `cargo +nightly-2026-08-20 test -p turnloop-contract --test windows_console -- --nocapture --test-threads=1`:
  two tests, each spawning an isolated console child; real Ctrl-C/Break fan-out,
  console input/resize, VT modes and restoration on close/drop.
- `cargo +nightly-2026-08-20 test -p turnloop-contract --test windows_lifetimes --test windows -- --nocapture --test-threads=1`:
  34 tests, including GUI timer reset, 128 synchronous cancellation/drop races,
  and 32 TCP loop lifetimes returning to the native handle-count baseline.
- `cargo +nightly-2026-08-20 clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks`.

Intermediate failures fixed without relaxing assertions:

- Shared refused-connect watchdog: Windows retries refused loopback SYNs. Disable
  SYN retransmission for loopback destinations only; retain administrator settings
  for network destinations. RtlNtStatusToDosError returns Win32 code 1225, which
  additionally needs explicit ConnectionRefused mapping (the standard I/O mapping
  recognizes Winsock's different code).
- IPC allocation gate: one lazily allocated routed-callback context on first
  handle transfer. Reserve callback contexts with the operation slab at setup;
  all five enabled allocation gates pass, including IPC and file-worker I/O.
- Bounded 12 ms wait: GQCSEx's competing integer timeout could end before 10 ms.
  Once an NT deadline is armed, use an infinite GQCSEx timeout and let the exact
  packet provide the deadline. The original bounded/no-spin assertions pass.
- Initial unsafe-aware lint runs flagged comment placement around multiline
  assertions. Comments now immediately precede the unsafe expressions.
- Imported handles need native file-mode classification before using a synchronous
  worker. Overlapped pipes now route to the receiving loop, including handles
  already associated with another port. Other overlapped files return Unsupported.

The initial PR CI run's Windows native job failed because Microsoft's bundled
curl lacks HTTP/2 (`--http2-prior-knowledge`), before production integration was
pushed. CI now installs checksum-pinned official curl 8.22.0_1 with HTTP/2.
Interop commands explicitly set child PATH because Rust otherwise searches
System32 before the inherited PATH. The unchanged HTTP/2 assertions now pass.
The next full-workspace run exposed CRLF-converted decoder reference fixtures;
`.gitattributes` now preserves their original bytes. All 68 decoder tests pass
after restoring the fixtures to their exact committed bytes.
Latest production CI and full-workspace runtime results are recorded below when
available; this record does not claim they have passed yet.

Unrun wider gates: ETW syscall traces, CPU-cycle A/B attribution, overnight soak,
Windows 10 minimum-version coverage, and VM/power-state precision matrices.
Console resize dispatch currently requires an active console input read.
