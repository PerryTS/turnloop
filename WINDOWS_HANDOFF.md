# Windows handoff

The Windows parts of turnloop were written and cross-checked on macOS (`cargo check` / `clippy --target x86_64-pc-windows-msvc`), but **nothing has run on Windows yet**. This file is the work order for the Windows machine.

## Setup

1. Install Git, [rustup](https://rustup.rs) and the Visual Studio Build Tools ("Desktop development with C++": MSVC linker + Windows SDK).
2. Clone the repository and check out the Windows lane:
   ```powershell
   git clone https://github.com/PerryTS/turnloop.git
   cd turnloop
   git checkout -b windows/iocp-backend   # everything (spikes and backend) is on main
   rustup show   # installs the pinned nightly-2026-08-20 from rust-toolchain.toml
   ```
3. Record the environment in the results file:
   - Windows edition and build (`winver`, or `[System.Environment]::OSVersion`)
   - bare metal or VM (which hypervisor)
   - CPU
   - `rustc --version`

## Phase 1: run the IOCP spikes (nothing is known to pass yet)

Everything lives in `spikes/iocp/`. See `spikes/iocp/README.md` for what each test binary proves, and `docs/lanes/windows.md` for the full UNRUN list.

```powershell
cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets -- -D warnings
cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets
cargo test --manifest-path spikes/iocp/Cargo.toml --test port
cargo test --manifest-path spikes/iocp/Cargo.toml --test timer -- --nocapture --test-threads=1
cargo test --manifest-path spikes/iocp/Cargo.toml --test tcp
cargo test --manifest-path spikes/iocp/Cargo.toml --test pipe
cargo test --manifest-path spikes/iocp/Cargo.toml --test stdio
cargo test --manifest-path spikes/iocp/Cargo.toml --test integration
cargo test --manifest-path spikes/iocp/Cargo.toml --test process
cargo test --manifest-path spikes/iocp/Cargo.toml --test console
cargo test --manifest-path spikes/iocp/Cargo.toml --test draft -- --nocapture --test-threads=1
cargo test --manifest-path spikes/iocp/Cargo.toml --test handles
```

Rules:
- **Fix real Windows bugs in the spike code, never by weakening a test.** Each fix is its own commit, with a message saying what Windows actually did.
- **Timer precision (`--test timer`):** record the measured lateness distribution for BOTH routes: (a) the high-resolution waitable timer with an APC and alertable `GetQueuedCompletionStatusEx`, and (b) the `NtAssociateWaitCompletionPacket` route. Record whether the machine is a VM. That decides DESIGN.md §15 open question 3.
- **Console test:** it spawns a child with `CREATE_NEW_CONSOLE`. Run it from a normal interactive session, not over a non-interactive SSH session without a console.
- **Write the results** to `spikes/iocp/WINDOWS_RESULTS.md`: one row per test with PASS / FAIL / fixed-in-commit, the timer precision numbers, the environment, and anything that contradicts DESIGN.md §7.3.

## Phase 2: port the backend onto the core trait (ready now)

The core Backend trait is merged on `main` as **revision 2**, tag `trait-v2`, in `crates/turnloop/src/backend/mod.rs`. Revision 2 adds pipes/local IPC, stdio, handle passing, processes, signals, TTY and external waits on top of revision 1's host-clock and scheduling hooks; see `docs/BACKEND_REVISION_2.md` and `docs/lanes/core.md`. The Windows mechanisms for all of these are prototyped in `spikes/iocp` (named pipes, overlapped stdio or reader threads, Job Objects + RegisterWaitForSingleObject, SetConsoleCtrlHandler, console input).

2. Port `spikes/iocp/backend_draft/` to `crates/turnloop/src/backend/iocp/` against the trait. The adaptation notes are in `docs/lanes/windows.md`. Keep `spikes/iocp` as the mechanism reference.
3. Run the contract suite on Windows: `cargo test -p turnloop-contract -- --test-threads=1`, including the no-spin contract (DESIGN.md §10 rule 4a), plus the Windows-specific variants listed in `spikes/iocp/CONTRACT_TEST_PLAN.md`.
4. Record the results in `WINDOWS_RESULTS.md`.
5. CI: `.github/workflows/ci.yml` also runs on GitHub's `windows-2025` runner. To run the same jobs on this machine, register it as a self-hosted runner with label `turnloop-windows` and set the repository variable `SELF_HOSTED_WINDOWS=true`.

## Rules (same as LANES.md)

- **Branches:** push to `windows/iocp-backend` only and open a **draft PR** against `main`; never push to `main`. CI runs on the PR, including GitHub's `windows-2025` runner.
- **Commit messages:** plain, with no attribution, co-author or tool-signature lines.
- **Dependencies:** only `windows-sys` (plus test-only dev-dependencies). No tokio, no compio.
- **Honesty:** report any failing, skipped or unrun test as exactly that, with the command.
