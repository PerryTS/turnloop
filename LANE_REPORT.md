# ci-fix lane report

Updated 2026-09-14. All four requested CI fixes are implemented. The integrator
owns commits, pushes and hosted CI. External checkpoint `8d2a10f` appeared during
work; final source/tests/docs may remain uncommitted. No Git mutation was attempted.
WASI/web jobs and their runner branches are unchanged.

## Implemented

1. **Security exception and rustls update.** `scripts/ci/soak.py` reads exact
   crate/version exceptions from `scripts/ci/policy.toml`. Each requires advisory,
   reason and expiry. It verifies registry checksums, validates expiry against
   publish date + seven days, prints active entries, and rejects malformed,
   duplicate, expired, unused or mismatched entries. Other young versions still
   fail. Environment overrides remain forbidden when running the gate.
   rustls is locked to **0.23.45** for **RUSTSEC-2026-0285**, published at
   **2026-09-14T15:11:17Z**, eligible **2026-09-21T15:11:17Z**. The policy stores
   `expires = 2026-09-21`; it becomes unused at the exact eligibility timestamp.
   The only lockfile change is rustls's version/checksum. `.cargo/config.toml`,
   the seven-day window, runtime bans and cargo-deny advisory policy are unchanged.

2. **Portable Windows coverage.** The failing zero count came from the independent
   backend contract invocation, not the portable workspace suites. No unnecessary
   `cfg(unix)` exclusion was found. Native CI now independently requires positive
   counts from core and every protocol member in default/all-feature modes, in
   addition to running the workspace. Metadata discovery includes future protocol
   members; an automation fixture covers a newly added HTTP member.
   Added portable notifier transition/retry/coalescing/closed-state tests and a
   zero-allocation gate for 1,000 cycles of handle/op storage, timer expiry/cancel,
   posts, completion draining and retained/released buffer leases. Bytes, tokens,
   generations, completions and actual operation counts are asserted.
   Existing SCRAM, wire, RESP, Mongo SDAM/selection and allocation suites run via
   normal workspace discovery. Windows IOCP contracts remain visibly **PENDING**
   in the job summary with a link to `WINDOWS_HANDOFF.md`. The contract package's
   `windows-contracts-pending` marker must be removed when IOCP lands. It exempts
   only the separate Windows contract requirement; no zero-count check was removed.

3. **Deterministic MySQL auth.** The real-server test deliberately authenticates
   both RSA/TLS accounts first, then sends `FLUSH PRIVILEGES` through a private TLS
   `auth_admin` account. It requires a successful server acknowledgement before
   asserting full RSA auth and full TLS auth without RSA, then fast hits for both
   accounts. The shared fixture SQL grants RELOAD plus database access to the
   admin and is used by both local and CI setup. The earlier bootstrap shell
   scripts no longer exist; `scripts/test-servers.py` owns both paths.
   CI was using floating `mysql:9`; it now uses **mysql:9.6.0**, matching the local
   Homebrew server. Official registry manifest exists for Linux amd64/arm64.

4. **Instruction-baseline bootstrap.** Missing baselines trigger three fresh
   Gungraun/Callgrind rounds, artifact **`instruction-baselines`**, and an intentional
   failure explaining exactly how to download/review/copy/commit the JSON files.
   `workflow_dispatch` boolean `record_baselines: true` runs the same recording
   path without the missing-baseline failure. Normal regression checking retains
   cgu=1, positive integer counts, exact controls, complete benchmark sets, and
   the 3% ceiling. Bench metadata explicitly lists all four expected summary keys
   and the control. Candidates use each benchmark's minimum measured count and
   must accept all three rounds under the unchanged limits. The artifact includes
   ready-to-review repository-relative JSON files, all raw rounds, and README
   commands. Recording never updates the committed baseline in place.
   Upload runs even after the intentional failure; its pinned action is official
   upload-artifact v7.0.1, published 2026-04-10 (already soaked).

`CONTRIBUTING.md` documents the security update, expiry cleanup, Windows pending
marker, MySQL reset, recording input, and exact artifact commit procedure.

## Verification ledger

Commands ran from this clone with pinned nightly-2026-08-20 and stable 1.97.1.
Logs are under ignored `.tools/`. Cross-compilation never counts as runtime proof.
All unsafe blocks added are test allocator forwarding with `// SAFETY:` comments;
no new I/O unwraps or production operation allocations were introduced.

| Command | Result |
|---|---|
| `env CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE='0 days' cargo +nightly-2026-08-20 update -p rustls --precise 0.23.45` | **PASS**; pinned Cargo accepts this env form, scoped to this command only |
| `python3 scripts/ci/install-tools.py cargo-deny actionlint zizmor shellcheck` | **PASS**; official archives verified against committed SHA-256 pins |
| `PATH="$PWD/.tools/bin:$PATH" cargo deny check` | **PASS** advisories, bans, licenses, sources; existing transitive-major duplicate warnings remain |
| `python3 scripts/ci/soak.py` | **PASS**, 193 locked registry versions, exactly one printed active security exception |
| `bash scripts/ci/no-tokio.sh` | **PASS**, eight targets × default/all features, no runtime exceptions |
| `python3 scripts/ci/test_gates.py` | **PASS**, 13 original adversarial gate tests |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | **PASS**, initially 27 and finally 28 tests; security expiry/unused/checksum cases, real parser bootstrap/dispatch simulations, Windows selection/zero-count failures, original gate/fixture tests |
| `PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` | **PASS**, pinned actionlint, zizmor and ShellCheck. Existing strict concurrency.queue compatibility filter retained; raw actionlint was not rerun |
| `cargo fmt --all` then `cargo fmt --check` | **PASS** |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS**, native |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS**, native |
| Same all-feature Clippy with `--target x86_64-unknown-linux-gnu` | **PASS**; Linux runtime **UNRUN** |
| Same with `--target x86_64-pc-windows-msvc` | **FAIL / incomplete**, ring C build cannot find Windows SDK `assert.h`; full protocol test-target cross-check **UNRUN** beyond that dependency |
| `cargo clippy --locked --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS**, every library |
| `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS**, including the new portable core tests |
| Whole-workspace all-target/all-feature Clippy with `--target wasm32-wasip2` and `--target wasm32-unknown-unknown`, same strict lints | **PASS** both; wasm execution **UNRUN**, jobs untouched |
| `cargo +stable check --locked --workspace --all-targets --all-features` | **PASS**, stable 1.97.1 |
| `cargo test --workspace` | **PASS**, 92 tests; 10 real-server tests ignored and not counted |
| `python3 scripts/ci/run-tests.py native` | **PASS**, 92 tests per workspace feature configuration plus independently checked core/protocol/contract suites; 368 total executions including repeats, 25 Unix contracts per configuration |
| `cargo +nightly-2026-08-20 run --release -p turnloop-bench --locked -- --portable --timers` | **PENDING** at report write; portable timing smoke, never Linux instruction counts |
| `scripts/test-servers.py --services mysql run cargo test -p turnloop-mysql --test server -- --include-ignored --test-threads=1` | Command **FAIL** at mysqld initialization, fatal signal in Aligned_atomic/Shared_spin_lock/delegates_init; all MySQL test bodies **UNRUN (sandbox)** |
| `python3 scripts/ci/instructions.py` | Expected precondition **FAIL** on macOS; actual Linux measurements/regression comparison **UNRUN** |
| `git diff --check` | **PASS** |

Read-only evidence: full required documents/lane reports; cfg/test audit; registry
rustls timestamp and checksum; upstream RustSec advisory patched range; upstream
Gungraun 0.19.4 source confirms `instructions::operations::<function>` summary keys;
official upload-artifact tag SHA/release date; Docker Hub manifest for 9.6.0 and
local `mysqld --version`. These checks passed. The initial RustSec web URL failed;
the official advisory-db source was fetched successfully. Initial patches that
failed context matching were corrected before checks; no verification gate was
weakened to resolve a failure.

MySQL cleanup left no runner state or running server. The initializer's partial
private datadir was moved to `.tools/sql/mysqldata-sandbox-failed-20260914`, so the
next outside-sandbox attempt starts fresh. The crash log remains at
`.tools/sql/mysql-init.log`.

## Deviations and proposed DESIGN.md changes

- No DESIGN.md edit. The user-authorized security exception is narrowly scoped
  to one exact version until its original seven-day soak completes. No change to
  no-spin, allocation limits, Windows/WASM priority or forbidden runtime policy.
- Windows backend contracts are pending as explicitly requested; portable tests
  now satisfy the required Windows native job. This does not claim IOCP coverage.
- No synthetic or macOS-derived Linux baseline was committed. The initial Linux
  instruction job is intentionally still red until the measured artifact is reviewed
  and committed. Recording mode validates measurement rather than regression.

## Open questions and next steps

No implementation questions remain in this lane. Integrator actions:
1. Commit the final working tree and push/re-run CI, especially Windows and the
   Linux MySQL service job. Windows/Linux runtime and Docker execution remain
   **UNRUN locally**. The first hosted artifact/upload path is also **UNRUN**.
2. Run the MySQL command above outside the sandbox, preferably twice against the
   same running fixture; the test itself warms both caches before resetting them.
3. Download `instruction-baselines` from the first Linux run, inspect its three
   rounds and controls, and follow its README / CONTRIBUTING commit commands.
   Then rerun ordinary CI for the real regression comparison. Manual recording
   is available with `record_baselines: true`.
4. Remove the rustls exception at **2026-09-21T15:11:17Z** or if 0.23.45 leaves
   Cargo.lock; stale entries deliberately fail the soak gate.
5. Windows lane removes `windows-contracts-pending` when the shared IOCP contracts
   are instantiated. WASI/web failure fixes remain owned by their other lane.
