# wasm3 lane report

Continuation of wasm2 on `lane/wasm2`, 2026-09-14, macOS arm64. The five requested changes and browser CI tooling are implemented. WASI p2/p3, Node and native gates pass. Real browser execution is **UNRUN on this host**; the command correctly fails. The additional dependency advisory audit **FAILS** on existing rustls 0.23.44; its fix has not completed the mandatory seven-day soak. This is not an all-green merge claim.

Read the previous root report, DESIGN.md, CONTRIBUTING.md, integration report and wasm/core/CI/Mongo lane reports completely. No applicable AGENTS.md was found. No Git mutations, commits, pushes or external messages by this lane. Integrator checkpoints happened externally during the work. Newer main CI changes were not reproduced.

## Implemented and independently verified

### 1. p3 UDP: canonical list allocation removed

The 100 allocations/6,400 bytes came from host canonical lowering of each received `list<u8>`, not address conversion or the provided/pool output buffer. Release p3 now redirects the hand-lowered async imports' `cabi_realloc` into retained 64 KiB slots. Each UDP socket reserves a slot during setup. Capacity stays at the high-water mark until the last arena owner drops. All live loops on the guest agent share storage: progress on one loop can return another loop's subtask. Busy storage survives turns and is released only on result consumption or acknowledged cancellation. Error text also uses its correct storage owner. Oversized exceptional strings and unrelated imports use the heap. Context restoration occurs before allocation-scope cleanup.

**PASS:** the original 100-datagram zero-allocation gate is unchanged. Added 320 concurrent IPv6 receives and 320 cancellations over two loops, provided and pooled buffers, lengths 0–8,192, output capacity one, pending-loop drop and surviving-loop reuse. Completion identities, addresses and payload bytes are checked before zero allocations. Release unit tests additionally initialize two complete 65,536-byte canonical lists, verify lifetime and reuse, consume an allocated error string, and exercise exceptional growth to 65,537.

### 2. Complete p3 workspace, including MongoDB

Checked the registry API and exact manifests: newest compatible rand 0.9.5 (2026-07-11) still requires rand_core 0.9/getrandom 0.3.4. The last getrandom 0.3 release is 0.3.4 (2025-10-14), without p3 support. Soak-eligible getrandom 0.4.3 (2026-06-17) supports p3 but cannot satisfy BSON's 0.3 requirement.

The p3-only target rustflag selects getrandom's documented custom backend. `turnloop-wasi-random` supplies the unique `__getrandom_v03_custom` symbol used by both locked generations. It initializes caller bytes using scalar `wasi:random/random@0.3.0.get-random-u64` imports, including tails and uninitialized destinations, without list allocation or predictable fallback. MongoDB, MySQL and Postgres link the same small crate, avoiding duplicate definitions when combined. There is no event-loop dependency in the protocol crates. Downstream applications must supply the target rustflag themselves; dependency Cargo config is not inherited. See [the mechanism](docs/wasm.md#p3-entropy-and-the-complete-workspace).

**PASS:** whole-workspace p3 debug/release Clippy and actual release build/link with all targets/features, including MongoDB. Runtime tests generate 754 bytes through both getrandom generations, reject zero/constant long outputs, and generate distinct BSON ObjectIds. Another 200 real entropy fills pass at zero allocations. No registry versions changed; the soak policy remains enabled.

### 3. Timer precision attributed to Wasmtime

The new bare example calls the same WASI clock primitive with no turnloop types or calls. P2 uses monotonic-clock subscription/poll; p3 removed pollables, so its bare program uses the same async-lowered clock and one waitable set. Each program executes twenty 250 µs waits and rejects early returns.

Runtime: **wasmtime 46.0.0 (423be7a4e 2026-06-22)**, release, macOS arm64.

| Subject | Median lateness |
| --- | ---: |
| Bare p2 | 919,416 ns |
| Bare p3 | 907,125 ns |
| turnloop p2 | 1,062,667 ns |
| turnloop p3 comparison run | 973,666 ns |
| turnloop p3 final full runner | 1,082,792 ns |

All comparison raw samples are recorded in [docs/wasm.md](docs/wasm.md#wasi-timer-measurements). Final p3 raw samples (ns):
`[1072333, 2345833, 1076292, 1069500, 1076291, 1069334, 1080292, 1082792, 1093917, 1087375, 1104959, 1073958, 1086417, 1073708, 1134542, 1110500, 1099000, 1069167, 2309833, 1031291]`.

The bare programs reproduce the ~1 ms floor. Per the integrator's explicit decision, DESIGN §7.4/§7.6 now say **host-dependent (Wasmtime ≈1 ms)** and the release WASI median gate is ≤2 ms. Debug excludes precision sampling, while semantic timers and no-spin still execute. The native <500 µs ceiling is intact. **PASS:** both WASI profiles execute sixty no-spin expiries, requiring turns per expiry ≤2 and zero-event waits ≤1. No wait floor or busy-spin was added. CI runs the bare release program and prints raw driver samples.

### 4. p3 allocation harness: explicit release requirement

Used the integrator-authorized release-only option. With the pinned compiler, the debug Counting GlobalAlloc entry accesses shadow-stack context slot 0 during pre-main `get-arguments` canonical realloc; the slot is zero and traps before any test body runs. Backend restoration cannot fix that earlier entry. Release inlining avoids the startup path. The custom canonical allocator is also release-only; debug uses std-owned canonical lists. **No debug zero-allocation claim.** The reason is documented in the harness and wasm docs.

**PASS:** the mandatory WASI CI runner independently requires positive counts for release core unit tests, debug contracts, release contracts/precision and release allocation binaries. Both debug and release workspace Clippy are in CI. P3's final run executes **5 + 18 + 18 + 6** tests, respectively; every allocation threshold remains zero. P2 executes **3 + 17 + 17 + 5**.

### 5. Strict p3 boundedness remains experimental

[docs/upstream/wasi-p3-wait.md](docs/upstream/wasi-p3-wait.md) states the exact remaining gaps: host work/delay inside the cooperative yield used for `Now` progress, deadline scheduling priority amid unrelated component work, synchronous cancellation cleanup latency, and compiler/context/concurrent-export portability. Counting one wait-set operation does not bound the yield or arbitrary host work.

Closing this requires a specified bounded host-progress/deadline operation (or normative bounds on yield/poll), plus a supported wit-bindgen persistent stepping API with cancellation, retained caller buffers and allocator/context guarantees. The document identifies the relevant upstream design/discussion; no issue or patch was submitted. Measured no-spin passes are not proof of strict §7 boundedness. **`wasi-p3-experimental` remains mandatory.**

### Browser CI

Linux CI installs checksum-pinned Chrome for Testing/chromedriver **153.0.8010.36**, Firefox **155.0.1** and geckodriver **0.37.1**. The installer verifies archives before extraction, rejects unsafe paths, restores executable modes, checks versions on Linux and supplies explicit binary paths. All four official Linux archives were downloaded, checksummed and extracted on this Mac. Mozilla SHA256SUMS and the Gecko release asset digest also matched; Chrome hashes were computed from the official HTTPS archives. Pins are in `scripts/ci/browsers.json`.

Each runner owns its driver process group, checks `/status`, preserves Chrome verbose/Gecko trace logs, prints them on failure and cleans up browser children. Ordinary stderr is not a startup failure. Each browser is attempted independently. Zero passed browser tests cause failure even if compilation or Node passes.

**PASS:** all 18 Python CI tests, including actual temporary HTTP driver startup, stderr diagnostics, zero-count rejection, exit-23 startup failure, log emission and process cleanup, plus archive tampering/traversal rejection. Workflow lint passes through the existing wrapper (including its previously documented exact actionlint `queue` compatibility exception; not introduced here).

**PASS:** Node **7/7**, with 3 fetches, 1 slow/abort request, 4 WebSockets and 6,657 echoed bytes; includes the guest allocation subject and 2,000 posts from real workers. **UNRUN:** browser test bodies on this host. This attempt started both drivers, but Chrome exited during session creation and Firefox aborted with SIGABRT; no browser test body ran. Logs are in `.tools/browser-logs/` and were printed by the failed command. The integrator also reproduced ChromeDriver SIGKILL outside the sandbox with matching versions, establishing a Mac host restriction; this is not reported as merely a sandbox issue. Linux GitHub browser execution is still required.

## Remaining FAIL / UNRUN and precise reasons

| Gate | Status | Reason / required follow-up |
| --- | --- | --- |
| `cargo deny --locked check advisories bans licenses sources` | **FAIL** | Existing rustls 0.23.44 has [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285), TLS 1.3 encryption-level validation. Bans/licenses/sources pass. Fixed rustls 0.23.45 was published `2026-09-14T15:11:17.808465Z` ([registry](https://crates.io/api/v1/crates/rustls/0.23.45)); it becomes soak-eligible `2026-09-21T15:11:17.808465Z`. This is an inherited protocol-test TLS dependency. No advisory ignore, dependency substitution or soak exception was added. Integrator must update after eligibility and rerun audits/TLS fixtures. |
| Chrome and Firefox test bodies | **UNRUN**; runner exits 1 | Mac host launch/session failures above. Required Linux job must execute both suites with positive counts. Checksums, compilation and fake drivers are not browser passes. |
| Linux / Windows runtime tests | **UNRUN** | Neither OS nor Docker is available. Linux full-workspace cross-Clippy passes; Windows workspace libraries and core/contract all-target cross-Clippy pass. Native target CI remains required. |
| Windows whole-workspace protocol-test cross-Clippy | **UNRUN** | This Mac lacks the Windows SDK/C headers for test-only ring; libraries and core/contracts were checked instead. Full `--workspace --all-targets` must run on Windows. |
| p3 debug allocation binary | **UNRUN**, unsupported measurement profile | Known pre-main canonical allocator trap; explicit authorized release restriction above. Release gate runs and passes; debug semantics/no-spin run and pass. |
| Stable p3 build | **UNRUN**, unsupported toolchain | p3 requires its existing nightly-2026-09-07 pin. Stable native/p2/web pass. Default nightly-2026-08-20 remains unchanged. |
| Real-server protocol fixtures | **UNRUN in this continuation** | PostgreSQL `shmget` and MySQL initialization failures are established sandbox limits. Other protocol fixture suites are outside these backend changes and were not rerun; workspace-ignored tests are not counted as runtime passes. Integrator retains separate server runner/CI suites. |
| Strict p3 boundedness / other component runtimes | **UNRUN / unproven** | Requires upstream scheduling/context guarantees detailed above, not just another timing run. Keep experimental flag. |

## Resolved development failures

- First retained-storage allocation build failed because a reservation containing `Rc` was put into the `Send` detached type. It now belongs to the live UDP resource; WASI detach remains Unsupported. Final cross-Clippy/build passes.
- The untouched p3 precision test failed at median 986,750 ns against 500 µs. Bare-runtime evidence justified the explicitly authorized platform-bound change.
- The first version of the **new** expanded UDP fixture tried 16/60 KiB wire datagrams and received an I/O error. A direct IPv6 socket probe accepted 8 KiB but rejected 16/60 KiB with EMSGSIZE (errno 40) on this Mac. The fixture now uses portable ≤8 KiB wire payloads; separate canonical tests cover full 64 KiB lists. No existing test, zero threshold or no-spin gate was weakened.

## Deviations, open questions and next steps

DESIGN changes are limited to the requested §7.4/§7.6 host-dependent precision decision. The authorized p3 release-only allocation restriction is documented; zero allocations and no-spin limits are unchanged. The shared entropy linker crate is the selected documented custom-backend mechanism; MongoDB was retained. UDP storage is reserved at socket setup and retained at the arena high-water mark, trading bounded setup memory for zero steady-state canonical allocations.

The remaining upstream question is which specified host-progress and wit-bindgen API can provide strict bounds without spinning. No promotion is proposed. The integrator should run the pinned Linux browser jobs and native Linux/Windows CI, rerun restricted server fixtures outside the sandbox, and resolve the rustls advisory after the seven-day date above. Merge newer main CI changes during integration as planned. This working tree is coherent and ready for review, with the advisory failure and platform execution limits explicitly outstanding.

## Verification command ledger

All commands run from the clone root. Unqualified Cargo uses nightly-2026-08-20. `+stable` is installed stable 1.97.1. The WASI runner uses pinned local Wasmtime 46. Test runner commands expand into independently checked binaries and reject zero execution. Native runner executes workspace tests with default and all features (**93 each**), then the contract crate independently (**28 each**). Cross-Clippy includes `-D warnings` and `-D clippy::undocumented_unsafe_blocks`. No-tokio passes on all eight target graphs with default and all features; soak passes for all 208 locked packages.

The ledger below lists every verification command, grouping identical repeats and preserving status history. Local `.tools/wasm3/commands.jsonl` also contains exit codes/times and later per-command logs; early logs were not all retained, so durable subject counts and timer samples are recorded above. `$PWD` and `$PATH` abbreviate only the clone path and inherited tool search path. Browser command **FAIL** means test bodies **UNRUN**, as explained above.

<!-- command-ledger -->

| Run IDs | Status history | Command |
| --- | --- | --- |
| 1 | PASS | `cargo check --workspace --all-targets --all-features` |
| 2 | PASS | `cargo +nightly-2026-09-07 clippy --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 3 | PASS | `cargo run --locked -p turnloop-contract --release --target wasm32-wasip2 --example wasi_timer_baseline --config 'target.wasm32-wasip2.runner="scripts/ci/wasmtime-runner.sh"'` |
| 4 | PASS | `cargo +nightly-2026-09-07 run --locked -p turnloop-contract --release --features wasi-p3-experimental --target wasm32-wasip3 --example wasi_timer_baseline --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"'` |
| 5, 6, 11, 12 | FAIL → PASS → FAIL → PASS | `cargo +nightly-2026-09-07 test --locked -p turnloop-contract --release --target wasm32-wasip3 --features wasi-p3-experimental --test allocations --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' -- --nocapture --test-threads=1` |
| 7 | FAIL | `cargo +nightly-2026-09-07 test --locked -p turnloop-contract --release --target wasm32-wasip3 --features wasi-p3-experimental --test wasi --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' -- --nocapture --test-threads=1` |
| 8, 10, 15, 38 | PASS ×4 | `cargo fmt --all` |
| 9, 35, 48 | PASS ×3 | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` |
| 13 | PASS | `python3 scripts/ci/install-browsers.py --verify-only` |
| 14, 46 | PASS ×2 | `python3 -W error -m unittest discover -s scripts/ci -p 'test_*.py' -v` |
| 16, 39 | PASS ×2 | `cargo +nightly-2026-09-07 test --locked -p turnloop --release --target wasm32-wasip3 --features wasi-p3-experimental --lib --config 'target.wasm32-wasip3.runner="scripts/ci/wasmtime-runner.sh"' -- --nocapture --test-threads=1` |
| 17 | PASS | `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 18, 49 | PASS ×2 | `env "PATH=$PWD/.tools/bin:$PATH" python3 scripts/ci/lint-workflows.py` |
| 19, 45 | PASS ×2 | `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --release --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 20 | PASS | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` |
| 21 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features` |
| 22, 51 | PASS ×2 | `cargo fmt --all --check` |
| 23 | PASS | `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 24 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 25 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 26 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 27 | PASS | `.tools/bin/wasmtime --version` |
| 28 | PASS | `cargo +nightly-2026-09-07 clippy --locked --workspace --all-targets --all-features --target wasm32-wasip3 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 29 | FAIL | `env "PATH=$PWD/.tools/bin:$PWD/.tools/wasm-bindgen-source/target/release:$PATH" "WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache" "CHROMEDRIVER=$PWD/.tools/wasm-pack-cache/chromedriver-153.0.8010.36/chromedriver" "GECKODRIVER=$PWD/.tools/wasm-pack-cache/geckodriver-654dd0c3a88b5d6f/geckodriver" python3 scripts/ci/run-tests.py web` |
| 30 | PASS | `cargo clippy --locked --workspace --lib --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 31 | PASS | `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings -D clippy::undocumented_unsafe_blocks` |
| 32 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-wasip2` |
| 33 | PASS | `cargo +stable check --locked --workspace --all-targets --all-features --target wasm32-unknown-unknown` |
| 34 | PASS | `python3 scripts/ci/run-tests.py native` |
| 36 | PASS | `bash scripts/ci/no-tokio.sh` |
| 37 | PASS | `python3 scripts/ci/soak.py` |
| 40 | PASS | `cargo +nightly-2026-09-07 build --locked --workspace --all-targets --all-features --release --target wasm32-wasip3` |
| 41 | PASS | `python3 scripts/ci/install-tools.py cargo-deny` |
| 42 | PASS | `env 'RUSTDOCFLAGS=-D warnings' cargo doc --locked --workspace --all-features --no-deps` |
| 43 | PASS | `env "PATH=$PWD/.tools/bin:$PWD/.tools/wasm-bindgen-source/target/release:$PATH" "WASM_PACK_CACHE=$PWD/.tools/wasm-pack-cache" python3 scripts/ci/run-tests.py node` |
| 44 | FAIL | `env "PATH=$PWD/.tools/bin:$PATH" cargo deny --locked check advisories bans licenses sources` |
| 47, 52 | PASS ×2 | `git diff --check` |
| 50 | PASS | `cargo clippy --locked --workspace --all-targets --all-features --release --target wasm32-wasip2 -- -D warnings -D clippy::undocumented_unsafe_blocks` |
