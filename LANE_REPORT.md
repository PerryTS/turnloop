# http-integrate lane report

Updated 2026-09-14. Integration implemented and verified within available platforms.
**Release is blocked by the rustls security fix's mandatory soak**, detailed below.
No commits or uploads by this agent; the integrator checkpoints the working tree.

## Implemented

- Twelve workspace members, ten publishable crates. Added HTTP/TLS/WebSocket and
  the independently publishable `turnloop-zstd-decoder`, with descriptions, READMEs,
  repository/docs/license metadata, shared dependencies and explicit versions on
  local dependency edges. Updated the publish order in `docs/INTEGRATION_REPORT.md`.
- Shared rustls 0.23.44/ring 0.17.14/std/tls12 across TLS and SQL/Redis/SMTP/Mongo
  test transports. Defaults/aws-lc disabled. ring supports native platforms and
  wasm32 with LLVM; browser entropy and PKI web features are explicit, WASI uses
  host entropy. No FIPS/PQ promise. Removed rustls-pemfile for maintained PKI PEM.
- Soaked tungstenite 0.30.0 shares SHA-1 0.11 and getrandom 0.4 with the workspace.
  Remaining transitive random generations belong to BSON/ring/upstream tests.
  Mozilla trust-anchor data's CDLA-Permissive-2.0 license was reviewed; its required
  redistribution text ships in TLS. No advisory ignore or source exception.
- Ruzstd option (b): upstream 0.8.3 and soaked 0.9.0 contain unavoidable private
  per-frame allocations. The native reproduction observes **6000 allocations in
  1000 frames**. The published MIT fork retains probabilities/default-table slices
  and passes the identical zero-allocation and exact-output checks. No root patch
  and no consuming-workspace patch required. Exact upstream submission draft:
  `docs/upstream/ruzstd.md`; source/license/fixture attribution ships with the fork.
- Preserved upstream source and tests, restored 101 ordinary/207 dictionary frames
  and 47 fuzz artifacts against their Git blob hashes; recorded the fixture hashes.
  Adapted safety comments/lints and widened dictionary arithmetic to i64 for wasm32.
  Kept C-reference tests on native hosts and pure decoder checks on all targets.
  Native `pure-rust-zstd` also exercises WASM's implementation and allocation gates.
- Unified HTTP fixture lifecycle in `scripts/test-servers.py`, including start,
  authenticated stop, failure cleanup and both closed-port assertions. Node fixture
  implementation is under `scripts/fixtures`; ports use TURNLOOP_TEST_HTTP_PORT and
  TURNLOOP_TEST_HTTP2_PORT. Removed the six superseded HTTP lane scripts. HTTP's
  packaged interop test helper is self-contained rather than a sibling file path.
- Pinned Node 26.5.1 on native/protocol CI. Metadata selects HTTP/TLS/WebSocket
  interop; separate required WASI 0.2/0.3 protocol jobs execute codecs/allocations.
  Extended policy/no_tokio and cargo-deny bans with all HTTP lane restrictions,
  including the all-target union graph. Strict ci-gate fan-in includes new jobs.
- `scripts/ci/h2spec.py` verifies the pinned source archive before extraction,
  verifies Go modules, builds and runs strict h2spec. Checks JUnit identities,
  totals, failures/errors/skips and rejects stale/partial results. The 16-KiB
  response makes the formerly skipped negative-window test run: **147/147 pass**.
- Allocation gates use standalone harnesses that unconditionally run every original
  test function and first prove a known allocation is counted. Native counters are
  thread-local; single-threaded wasm uses atomics. This avoids pinned p3 libtest's
  CLI-import/custom-allocator stack trap without removing assertions or raising
  thresholds. These allocation targets always run all tests, regardless of filters.

## Verification commands

Commands ran from this checkout unless a manifest path is specified. `--locked`
was used after resolution. Native stable is 1.97.1. Logs are in `.tools/` (ignored).
For WASM commands, the respective `CC_wasm32_*`/`AR_wasm32_*` were set to
`/opt/homebrew/opt/llvm/bin/clang` and `llvm-ar`; CI uses clang/llvm-ar on PATH.
No Linux/Windows cross-compilation result is counted as runtime execution.

| Command / invocation group | Result |
|---|---|
| `cargo generate-lockfile` (initial integration and tungstenite unification) | PASS, unchanged seven-day resolver; rustls 0.23.45 explicitly excluded |
| `cargo fmt --all` / `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| Same with `--all-features` | PASS, including final standalone harnesses |
| `cargo clippy --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS; runtime UNRUN |
| Same with `--target wasm32-wasip2` | PASS |
| Same with `--target wasm32-unknown-unknown` | PASS, final harnesses included; browser runtime UNRUN |
| Same with `--target x86_64-pc-windows-msvc` | FAIL before Rust checking completes: ring/zstd C builds lack Windows SDK headers; full test-target check and runtime UNRUN |
| `cargo clippy -p turnloop-zstd-decoder --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +stable check --workspace --all-targets --all-features --locked` | PASS, including final harnesses |
| `cargo test --workspace` | PASS, final 190 passed; 12 real-server tests explicitly ignored, not counted |
| `cargo test --workspace --all-features` | PASS, 197 passed; same 12 ignored |
| `python3 scripts/ci/run-tests.py native` | PASS, 437 passes across default/all-feature suites and independent repeated 25-test contracts; no-spin gates unchanged |
| `scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop` | PASS, 15 tests: seven HTTP, five TLS, three WebSocket; actual Node/curl traffic, both HTTP/2 directions with 100 streams |
| `scripts/test-servers.py run cargo test --workspace -- --include-ignored` | Invocation FAIL at PostgreSQL initdb/shmget; all full-command test bodies UNRUN (sandbox) |
| `scripts/test-servers.py --services mysql run true` | Initializer FAIL, exit 2/fatal signal; MySQL bodies UNRUN (sandbox) |
| `scripts/test-servers.py --services redis,mongodb,smtp,http run cargo test --workspace --exclude turnloop-postgres --exclude turnloop-mysql -- --include-ignored --test-threads=1` | PASS, 173 tests, zero ignored; private servers stopped |
| `python3 scripts/ci/h2spec.py` | PASS, strict 147 tests, 147 passed, 0 skipped, 0 failed; `.tools/h2spec.xml`/log |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | PASS, 20 executed tests: 16 codecs, three HTTP allocation tests, decoder reuse |
| Same with `--target wasm32-wasip3` | PASS with nightly-2026-09-07/Wasmtime 46, all 20 tests; initial libtest traps described below |
| `CARGO_TARGET_WASM32_WASIP2_RUNNER="$PWD/scripts/ci/wasmtime-runner.sh" cargo test -p turnloop-zstd-decoder --all-features --target wasm32-wasip2 --lib dictionary::frequency -- --test-threads=1` | PASS, all three frequency tests execute |
| `cargo test --manifest-path .tools/ruzstd-repro/Cargo.toml --test reuse` | Expected FAIL, unpatched upstream 0.8.3 makes 6000 allocations; proves need for fork |
| `bash scripts/ci/no-tokio.sh` | PASS, eight target triples plus union × default/all features; normal/build/dev edges |
| `python3 scripts/ci/soak.py` | PASS, all 240 locked registry versions and checksums |
| `PATH="$PWD/.tools/bin:$PATH" cargo deny --locked check` | FAIL solely on RUSTSEC-2026-0285 after license integration; bans/licenses/sources PASS |
| `RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps` | PASS |
| `python3 scripts/ci/install-tools.py cargo-deny actionlint zizmor shellcheck` | PASS, checksum-pinned tools |
| `bash scripts/ci/install-wasmtime.sh` | PASS, checksum-pinned Wasmtime 46.0.0 |
| `PATH="$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` | PASS, actionlint compatibility wrapper, zizmor and ShellCheck |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS, 18 tests; actual HTTP lifecycle plus malicious/partial h2spec-report rejection |
| `cargo publish --dry-run --locked --allow-dirty --workspace` | PASS, every one of ten crates packaged and verified; no --no-verify, every upload aborted |
| Same with `--all-features --target wasm32-wasip2` | PASS, ten packaged libraries including HTTP's registry decoder dependency |
| `cargo test --locked --manifest-path target/package/turnloop-zstd-decoder-0.1.0/Cargo.toml --test reuse` | PASS from the independently unpacked published package |
| `python3 scripts/ci/release.py order` | PASS, ten publishable packages, helpers excluded |
| `rustup component add rust-src --toolchain nightly-2026-09-07`; `wasm-tools print <p3 allocations component>` | PASS diagnostic tools; isolated generated allocator thunk's invalid-stack access |
| `git diff --check`; fixture SHA-1/metadata/package inspections | PASS: exact 664-file upstream inventory; ten normalized archives without path/patch dependencies, all below 10 MiB |

The pinned h2spec source commit is `70ac2294010887f48b18e2d64f5cccd48421fad1`,
archive SHA-256 `791b995048c7e2a2895ed2c019eb9abe46015c3a7a6107cb9ea3f5e5f311da39`.
All 115 files were initially checked against their commit Git blobs before pinning.
Decoder registry source 0.8.3 SHA-256 is
`a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8`;
0.9.0 inspection also verified the registry checksum. No unverified release binary
was executed. Native package and WASI package verification were repeated after
manifest/harness changes. The decoder archive includes all fixtures (~9.6 MiB).

## Corrected intermediate failures

- Newly active upstream Rust source failed strict Clippy on unsafe operations,
  missing safety explanations and newer lint suggestions. `cargo clippy --fix
  --allow-dirty --allow-no-vcs -p turnloop-zstd-decoder --lib -- -D warnings`
  plus reviewed manual fixes retained source behavior and all tests.
- WASM all-feature dictionary code used an isize constant larger than i32; changed
  arithmetic to i64 and executed the unchanged three expected frequency cases.
- First h2spec report checker keyed cases only by classname; corrected identity
  includes package. The first ci-gate inventory regex omitted digit-containing
  `h2spec`; it now recognizes digits and still enforces complete fan-in.
- First package dry run rejected reserved upstream `Cargo.toml.orig`; renamed it
  `UPSTREAM-Cargo.toml`. Preserved upstream README separately under a name distinct
  even on case-insensitive macOS. Rustdoc links/bare URLs were corrected.
- A new ordinary-corpus count incorrectly included dictionary fixtures. The
  verified snapshot has 101 ordinary and 207 dictionary frames; each suite now
  independently asserts its own count, alongside all original byte/checksum checks.
  Initial non-SQL run failed this new incorrect count; its final full rerun passed.
- WASI 0.3 libtest CLI arguments trapped at generated `__rust_alloc`, address
  0xfffffff8. Replacing task-local counters alone did not fix it; disassembly showed
  the shim using a nonexistent stack during the import's allocator callback.
  Standalone harnesses avoid that startup import, execute every existing test and
  calibrate counting. Both WASI versions and native tests now pass unchanged limits.
- A temporary final inventory probe initially guessed 665 fixture files. Checking
  against the exact upstream Git tree confirms 664; every hash matches, no files
  were removed or expectations changed in the Rust suites.
- First cargo-deny run also rejected CDLA-Permissive-2.0; the reviewed Mozilla data
  license and redistribution text are now explicitly included. The security failure
  was not suppressed and remains a release blocker.

## Blockers / deviations / DESIGN proposals

No DESIGN.md changes, runtime exceptions, test exclusions hiding failures, or soak
relaxations. Fork option (b) was necessary. The Rust standard library's internal
rustc-dep-of-std feature is omitted from this independently published fork.

RUSTSEC-2026-0285 affects rustls 0.23.44. The fixed 0.23.45 was published
**2026-09-14 15:11:17.808465 UTC** and becomes eligible only on
**2026-09-21 15:11:17.808465 UTC**. The shared dependency remains on the soaked
version; cargo-deny intentionally rejects it. No safe, already-soaked fixed release
is available. The lane cannot honestly claim a green release gate today.

UNRUN: Linux/Windows runtime, full Windows Clippy without SDK, Docker fixture path,
browser runtime tests, and SQL bodies blocked by sandbox initialization. Existing
production Windows/WASM backend contracts and Linux instruction baseline remain
other-lane release prerequisites; protocol successes do not replace them. No live
GitHub workflow, registry publication, OIDC or release bootstrap was performed.

## Open questions / next steps

1. After 2026-09-21 15:11:17.808465 UTC, resolve rustls 0.23.45 with pinned nightly;
   rerun TLS/interop, workspace/targets, soak, cargo-deny and package gates. Do not
   publish while the required security audit fails.
2. Integrator reruns the full server command outside the sandbox and hosted matrix;
   supplies Windows SDK/runtime, Docker and backend/baseline prerequisites.
3. Submit the prepared ruzstd patch; replace the fork only after an upstream release
   passes the unchanged soak and identical allocation/behavior/package tests.
4. Track the pinned WASI 0.3 custom-allocator/libtest startup issue upstream; preserve
   the working standalone allocation gates and their calibration assertions.
5. Existing protocol-lane gaps remain: executor/Perry binding adapters, WebSocket
   permessage-deflate and detailed Node option/error parity. No full Node-parity claim.
