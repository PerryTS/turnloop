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

Production backend integration and shared contracts are in progress. No production
Windows runtime coverage or green Windows CI is claimed by this initial record.
