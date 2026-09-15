# udp-fix

Base: `c506775`. Implementation and verification complete. Default-parallel
workspace has one unrelated process/signal failure; documented serial execution passes.
Read DESIGN.md, CONTRIBUTING.md, integration and relevant core lane reports.
No applicable AGENTS.md. The integrator owns commits (`.git` is read-only).

## Implemented / cause

- Confirmed the proposed release/rebind race in a controlled run: four threads
  repeatedly bound exclusive IPv4/IPv6 ephemeral UDP ports; a temporary 10 ms
  scheduling gap after `dup2` exposed `EADDRINUSE`, macOS errno **48**, on iteration
  18 at `127.0.0.1:56207`, fd 3. The diagnostic-only baseline without the gap
  passed 2,000 runs. This proves the mechanism, not the unrecorded original errno.
- Both the exact fd/port reuse test and the default-bind policy test now retry
  the **whole address-family iteration**, at most 16 attempts, only after the
  released-port bind returns exactly `EADDRINUSE`. Every other error and exhausted
  retries fail with errno. Successful IPv4 and IPv6 iterations must still execute
  every original assertion, including cached readiness, cancelled operations,
  discarded old datagrams, actual waiting, new bytes/source and no duplicates.
- The module's one raw `libc::bind` captures errno immediately and reports fd and
  endpoint before propagating the error. Policy tests prove last-attempt success,
  exact exhaustion and immediate rejection of other/missing OS codes.
- `scripts/stress-udp-reuse.py` runs positive-count UDP suites with four active
  ephemeral-port churn threads, bounded retained sockets and per-run logs.
  The same 10 ms controlled gap with the fix passed **2,000/2,000** suites and
  recovered **78 actual bind conflicts** (50 IPv4, 28 IPv6), all errno 48.
  Final-source stress without the gap also passed **2,000/2,000**, recovering
  two real errno-48 conflicts. No successful family iteration was skipped.

## Verification

Logs and saved reproducer binaries/sources: `.tools/udp-fix/`. Controlled builds
inserted `std::thread::sleep(Duration::from_millis(10));` immediately after
`drop(replacement)`, saved the test binary, then restored the source. No pause is
in the final source. The stress command uses 2,000 iterations by default.
An initial final-stress run reused the saved-gap build because restoring source
also restored its older mtime. Binary SHA-256 comparison caught this. The source
mtime was refreshed, Cargo visibly recompiled, the new binary hash differs, and
the 20 core runs and final stress were rerun. That cached run is not final-source
verification; its logs remain in `cached-delayed-stress/`.

| Command | Result |
| --- | --- |
| `cargo test --locked -p turnloop --lib --no-run --message-format=json` (diagnostics-only, old/fixed 10 ms gap builds) | PASS |
| `python3 scripts/stress-udp-reuse.py --binary .tools/udp-fix/baseline-tests --logs .tools/udp-fix/baseline-stress` | PASS, 2,000 suites, 1,919,705 competing binds |
| `python3 scripts/stress-udp-reuse.py --binary .tools/udp-fix/delayed-tests --logs .tools/udp-fix/delayed-stress` | Expected FAIL at iteration 18, errno 48; 85,513 competing binds |
| `python3 scripts/stress-udp-reuse.py --binary .tools/udp-fix/fixed-delayed-tests --logs .tools/udp-fix/fixed-delayed-stress --require-contention` | PASS, 2,000 complete suites, 78 real conflicts recovered, 5,354,966 competing binds |
| `python3 scripts/stress-udp-reuse.py --binary /usr/bin/true --iterations 1 --logs .tools/udp-fix/empty-control` | Expected FAIL: exit-zero process with no executed tests rejected |
| `cargo fmt --all`; `cargo fmt --check`; `git diff --check` | PASS |
| `cargo test --locked -p turnloop backend::unix::udp_tests:: -- --nocapture` | PASS, all four tests actually ran |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS native |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS native |
| `cargo clippy --locked -p turnloop --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop --all-targets --all-features --target aarch64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +nightly-2026-09-07 clippy --locked -p turnloop --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS, repository's p3 toolchain |
| `cargo clippy --locked -p turnloop --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test -p turnloop` ×20 via `python3 .tools/udp-fix/repeat-core.py` | PASS, 12 tests each; original exact-reuse subject required in every run; default parallel threads, `RUST_TEST_THREADS` unset |
| `bash scripts/ci/no-tokio.sh` | PASS, eight targets plus union, default/all features |
| `python3 scripts/ci/soak.py` | PASS, 251 versions; inherited rustls security exception unchanged |
| `python3 -m py_compile scripts/stress-udp-reuse.py` | PASS |
| `cargo test --locked -p turnloop --lib --no-run` after restoring source mtime | PASS, actual final recompilation and distinct binary hash |
| `cargo test --workspace` | FAIL, unrelated `registered_processes_and_signals_do_not_spin` at `native_surface.rs:440` (`zero <= 1`); all 12 core tests and 10 core allocation tests passed |
| `cargo test --workspace -- --test-threads=1` | PASS, 237 tests including the failing parallel subject and allocation gates; 12 ignored service tests are UNRUN |
| `python3 scripts/stress-udp-reuse.py --logs .tools/udp-fix/final-stress` after fresh rebuild | PASS, 2,000 complete suites, two real conflicts recovered, 1,868,347 competing binds |

The workspace failure is in an unchanged contract binary; changed Rust is entirely
inside `#[cfg(all(test, not(loom)))] mod udp_tests`. Parallel signal stress can
interrupt another test's kevent wait; kqueue counts EINTR as an empty wait.
This is consistent with the failure, not an errno-instrumented diagnosis.
CONTRIBUTING.md:45–46 already requires serial contracts for signal/process and
allocator isolation. The serial run is supplemental; it does not erase the
requested default-parallel failure. The no-spin test/gate remains unchanged.

## Same-pattern audit / deviations

- Fixed `default_udp_bind_does_not_enable_address_sharing`, the other UDP
  close-then-exact-rebind test. The explicit-sharing test keeps both sockets live.
- `turnloop-contract::refused_connect_once` releases a TCP listener before
  connecting, so an unrelated listener could steal its port. This does not rebind
  the port; retries would hide an incorrect successful connection. Left unchanged
  for the existing portable refusal-fixture follow-up, including WASI.
- TCP fixture gaps, listed without changing their lifecycle policies:
  `scripts/test-servers.py:145` (`sql_port`, used by PostgreSQL/MySQL/SMTP), `:317`
  (`redis_port`, explicit candidate and bus-port probes), `:525` (MongoDB socket
  reservations), and `scripts/ci/browser_driver.py:55` (WebDriver). They release a
  reservation before a child binds. A fix needs verified child bind errors and
  bounded restart/cleanup, rather than retrying arbitrary startup failures.
- Related tests in `scripts/ci/test_servers.py:220`, `:238`, `:287`, `:302` assume
  an unused/just-closed TCP port remains closed while probing refusal/cleanup.
  They do not rebind; retrying would obscure their cleanup/refusal subjects.
  Other audited Rust TCP/UDP tests, native/web HTTP fixtures and the proxy keep
  their listeners alive or let the serving process bind port zero directly.
- No production changes, dependencies, sharing options, wait paths, allocation
  thresholds, CI/soak policy or DESIGN.md edits. No new runtime operation requires
  extending an allocation gate; existing allocation tests remain mandatory.

## Open questions / next steps

- No UDP implementation questions remain. Keep the unrelated default-parallel
  process/signal no-spin failure visible; investigate its isolation separately.
  No-spin assertions and the required CI commands were not modified.
- Linux/Windows runtime: UNRUN (no hosts); cross-checks are compilation only.
  Integrator should run native tests on those hosts and commit the working tree.
