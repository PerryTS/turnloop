# http-merge lane report

Updated 2026-09-14. Current main is merged in the working tree; Git's merge index
still needs the integrator to stage and commit the resolved files.
No commits or uploads by this agent; the integrator checkpoints the working tree.

## Current merge resolution and verification

- Resolved all markers in `scripts/test-servers.py`, `scripts/ci/run-tests.py`
  and `scripts/ci/test_servers.py`, retaining both parents' capabilities and tests.
- Retained HTTP/TLS/WebSocket interop, strict h2spec, WASI 0.2/0.3 protocol runs,
  private HTTP shutdown, main's positive native per-member counts and explicit
  pending Windows contracts, Redis 8.4.0/TLS installation, all service log tails,
  crash-safe Redis cleanup, original-error chaining and deterministic MySQL auth.
- HTTP startup now uses the shared log-tail helper and preserves the startup
  error if child cleanup fails. New regressions execute a crashing child and
  verify bounded stdout/stderr tails, reaping, default HTTP selection and the
  failed test command's exit status.
- `Cargo.lock` contains one rustls, 0.23.45, shared by all consumers. Main's
  RUSTSEC-2026-0285 exception is retained unchanged, expiring at the registry's
  exact seven-day timestamp on 2026-09-21. The resolver's seven-day configuration
  and runtime bans remain unchanged. The prior security blocker below is historical.
- Main's measured Linux instruction baseline is present and retained unchanged;
  regression execution still requires Linux/Valgrind.

Merge verification is complete within this macOS sandbox. The requested native,
Linux/WASI 0.2/browser cross-checks, tests, interop, dependency gates and both
package dry runs pass. SQL bodies, Linux/Windows runtime and Docker remain UNRUN.
Additional full Windows Clippy is blocked by SDK headers; extra whole-workspace
WASI 0.3 Clippy exposes the inherited BSON/getrandom target incompatibility.
HTTP/decoder WASI 0.3 runtime tests pass. The required platform gates remain intact.

All commands below ran from this clone. Logs are `.tools/http-merge/<label>.log`
and exact exit codes/timings are in `.tools/http-merge/commands.jsonl`. For WASM,
`CC_wasm32_{wasip2,wasip3,unknown_unknown}` and matching `AR_*` select
`/opt/homebrew/opt/llvm/bin/clang` and `llvm-ar`. `.tools/bin` precedes PATH.
No publish-age override was set. Rust source and allocation thresholds were not
changed by the merge resolution; all unsafe-aware lint flags remain enabled.

| Command | Result | Evidence / limitation (log label) |
|---|---|---|
| `cargo metadata --locked --all-features --format-version 1` | PASS | Twelve members; one shared rustls 0.23.45 with ring/std/tls12. (`metadata`) |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS | 48 tests before the third added HTTP regression. (`scripts`) |
| `cargo build --locked --workspace` | PASS |  (`build`) |
| `python3 scripts/ci/lint-workflows.py` | PASS | Strict queue validation, supported actionlint diagnostics, zizmor and ShellCheck. (`workflow-lint`) |
| `cargo fmt --all --check` | PASS |  (`fmt`) |
| `python3 scripts/ci/soak.py` | PASS | 240 registry versions; exactly one active rustls 0.23.45 exception, no other flags. (`soak`) |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |  (`clippy-native`) |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |  (`clippy-native-all`) |
| `cargo deny --locked check` | PASS | Advisories, bans, licenses and sources pass; transitive-version duplicates remain warnings. (`deny`) |
| `bash scripts/ci/no-tokio.sh` | PASS | Eight triples plus all-target union, default/all features, normal/build/dev edges. (`no-tokio`) |
| `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | Compile only; Linux runtime UNRUN. (`clippy-linux`) |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |  (`clippy-wasip2`) |
| `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |  (`clippy-web`) |
| `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | Missing Windows SDK C headers (ring/zstd); full test-target check blocked, runtime UNRUN. (`clippy-windows`) |
| `cargo +stable check --workspace --locked` | PASS | Installed stable rustc 1.97.1. (`stable`) |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS | Installed stable rustc 1.97.1, all targets/features. (`stable-all`) |
| `env 'RUSTDOCFLAGS=-D warnings' cargo doc --locked --workspace --all-features --no-deps` | PASS |  (`rustdoc`) |
| `cargo test --locked --workspace` | PASS | 192 passed; 12 external-service tests ignored and not counted. (`tests`) |
| `.tools/bin/actionlint -color` | FAIL | Only the two inherited concurrency.queue parser errors in actionlint 1.7.12; strict existing wrapper passes. (`actionlint-raw`) |
| `.tools/bin/zizmor --offline --min-severity low .github/workflows` | PASS | No findings beyond existing annotations/suppressions. (`zizmor`) |
| `python3 scripts/ci/run-tests.py native` | PASS | 639 passes across workspace/default (192), all features (199) and independent member repetitions; unchanged no-spin/allocation gates pass. (`native-runner`) |
| `scripts/test-servers.py run cargo test --workspace -- --include-ignored` | FAIL | Initializer failure at PostgreSQL shmget; full-command test bodies UNRUN (sandbox). Original error and bounded log tail retained. (`servers-full`) |
| `scripts/test-servers.py --services mysql run true` | FAIL | Initializer exits 2 after fatal signal; MySQL bodies UNRUN (sandbox). (`servers-mysql`) |
| `python3 scripts/ci/instructions.py` | FAIL | Expected macOS host-precondition failure; actual Linux/Valgrind regression UNRUN. Committed baseline retained unchanged. (`instructions`) |
| `python3 scripts/ci/release.py order` | PASS | Ten publishable crates in documented dependency order; helpers excluded. (`release-order`) |
| `scripts/test-servers.py --services redis,mongodb,smtp,http run cargo test --locked --workspace --exclude turnloop-postgres --exclude turnloop-mysql -- --include-ignored --test-threads=1` | PASS | 175 passed, zero ignored; Redis 8.4.0 single/TLS/cluster/Sentinel, MongoDB, SMTP and HTTP; cleanup succeeds. (`servers-non-sql`) |
| `scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop` | PASS | 15/15 actual HTTP/TLS/WebSocket Node/curl tests. (`interop`) |
| `python3 scripts/ci/h2spec.py` | PASS | 147 distinct tests passed, zero skips/failures; source checksum and Go modules verified. (`h2spec`) |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | PASS | 20 tests executed: 16 codecs, three HTTP allocation checks, decoder allocation regression. (`protocol-wasip2`) |
| `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` | PASS | 49 tests; all 16 fixture tests from both parents retained, three added. (`scripts-final`) |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | PASS | Same 20 tests executed with nightly-2026-09-07 and Wasmtime 46. (`protocol-wasip3`) |
| `cargo clippy --locked --workspace --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | Compile only; Linux runtime UNRUN. (`clippy-linux-default`) |
| `cargo clippy --locked --workspace --all-targets --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |  (`clippy-wasip2-default`) |
| `cargo clippy --locked --workspace --all-targets --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |  (`clippy-web-default`) |
| `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL | Additional check: inherited BSON -> ahash/rand -> getrandom 0.3.4 rejects WASI 0.3. Unresolved; protocol subset passes. (`clippy-wasip3`) |
| `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-bench --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS | Core, contract and bench portable test targets compile; runtime UNRUN. (`clippy-windows-portable`) |
| `cargo publish --dry-run --locked --allow-dirty --workspace` | PASS | All ten archives packaged and verified; every upload aborted by dry run. (`publish-native`) |
| `python3 scripts/ci/lint-workflows.py` | PASS | Final workflow; no checks or filters weakened. (`workflow-lint-final`) |
| `python3 -m compileall -q scripts` | PASS |  (`python-compile`) |
| `cargo publish --dry-run --locked --allow-dirty --workspace --all-features --target wasm32-wasip2` | PASS | All ten packaged libraries verified with all features on WASI 0.2; every upload aborted. (`publish-wasip2`) |
| `cargo test --locked --manifest-path target/package/turnloop-zstd-decoder-0.1.0/Cargo.toml --test reuse` | PASS | One independently packaged decoder allocation/byte regression executed. (`packaged-reuse`) |
| `cargo +nightly-2026-09-07 tree --locked --target wasm32-wasip3 -i getrandom@0.3.4` | PASS | Confirms the failing getrandom 0.3.4 edges come from BSON via ahash and rand. (`wasip3-entropy-tree`) |
| `python3 .tools/http-merge-review.py` | FAIL | Diagnostic helper initially assumed archives lived directly under target/package; actual batch output is tmp-crate/. Corrected path; no product or test changes. (`semantic-review`) |
| `python3 .tools/http-merge-review.py` | PASS | All ten normalized archives, registry paths, unchanged policy/lock/baseline/MySQL tests, fixture cleanup and marker/whitespace scans pass. Every TLS archive locks rustls 0.23.45. (`semantic-review-final`) |
| `git diff --check` | PASS |  (`diff-check`) |

Additional read-only verification **PASS**: complete relevant design/contribution/
integration and HTTP/CI/SQL/KV lane-report review; `git status`/parent diffs;
Python AST union of both parents' fixture tests; byte equality of inherited soak,
policy, lock, MySQL auth test and instruction baseline; metadata/provider graph;
`rustc +stable --version`, `node --version`, Redis server/CLI versions and
`.tools/bin/wasmtime --version`. The final archive check confirms ten packages
below 10 MiB, no normalized path/patch requirements and eight TLS-containing
archives with rustls 0.23.45. Decoder archive size is 10,031,025 bytes.

`rg -n --hidden '^(<<<<<<<|=======|>>>>>>>)' -g '!.git' -g '!target/**'
-g '!.tools/**' .` — **PASS**, no output, expected exit 1. No private server state,
Redis instance/env records, HTTP state, SMTP supervisor socket or Docker ownership
record remains. `.git` remains unmodified and the integrator must stage the three
resolved conflict files before committing the merge.

**UNRUN:** the full SQL server bodies and deterministic MySQL auth sequence in this
sandbox; native Linux/Windows execution; Docker SQL probes/Mongo containers and
Linux Redis installer/cache execution; production WASI/browser/Node backend
contracts; Linux instruction runtime comparison; live GitHub CI/OIDC/publication.
Local installed Redis 8.4.0/TLS ran successfully, and installer corruption/version/
TLS tests executed. Full workspace WASI 0.3 Clippy is **FAIL**, not an environment
skip; its BSON/getrandom compatibility issue is recorded under next steps.

## Implemented

- Twelve workspace members, ten publishable crates. Added HTTP/TLS/WebSocket and
  the independently publishable `turnloop-zstd-decoder`, with descriptions, READMEs,
  repository/docs/license metadata, shared dependencies and explicit versions on
  local dependency edges. Updated the publish order in `docs/INTEGRATION_REPORT.md`.
- Shared rustls 0.23.45/ring 0.17.14/std/tls12 across TLS and SQL/Redis/SMTP/Mongo
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

## Historical HTTP integration verification (before this merge)

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
  was not suppressed; main's reviewed rustls update now resolves that former blocker.

## Blockers / deviations / DESIGN proposals

No DESIGN.md changes, forbidden-runtime exceptions or test/gate relaxations.
Main's reviewed security exception is retained. Fork option (b) was necessary.
The Rust standard library's internal rustc-dep-of-std feature is omitted from
this independently published fork.

Main replaced vulnerable rustls 0.23.44 with 0.23.45 and supplied the reviewed,
dated RUSTSEC-2026-0285 security exception. This supersedes the earlier audit
failure: all other dependency ages and every checksum remain mandatory. No new
exception, advisory ignore or resolver override was added by this merge lane.

UNRUN: Linux/Windows runtime, full Windows Clippy without SDK, Docker fixture path,
browser runtime tests, and SQL bodies blocked by sandbox initialization. Existing
production Windows/WASM backend contracts and Linux instruction regression remain
other-platform release prerequisites; the baseline is now committed. No live
GitHub workflow, registry publication, OIDC or release bootstrap was performed.

## Open questions / next steps

1. Have the integrator stage the resolved files and commit the merge. Remove the
   rustls security exception at 2026-09-21T15:11:17Z, when the index timestamp
   completes its soak.
2. Integrator reruns the full server command outside the sandbox and hosted matrix;
   supplies Windows SDK/runtime, Docker and production backend prerequisites;
   runs the instruction comparison against the committed Linux baseline.
   Resolve BSON's getrandom 0.3.4 WASI 0.3 compilation failure without weakening
   the required workspace gate; the HTTP/decoder p3 protocol suite already passes.
3. Submit the prepared ruzstd patch; replace the fork only after an upstream release
   passes the unchanged soak and identical allocation/behavior/package tests.
4. Track the pinned WASI 0.3 custom-allocator/libtest startup issue upstream; preserve
   the working standalone allocation gates and their calibration assertions.
5. Existing protocol-lane gaps remain: executor/Perry binding adapters, WebSocket
   permessage-deflate and detailed Node option/error parity. No full Node-parity claim.
