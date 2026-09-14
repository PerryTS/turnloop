# WASM lane report

Branch `lane/wasm`; report date 2026-09-14. All three spikes, the evaluation and
fallback drafts are present. **Production conformance is incomplete:** p2 poll
allocates, p3 needs a persistent bounded step provider, and browser assertions
have not run because Chrome startup fails. No failing gate has been waived.

## Implemented and verified subjects

1. **WASI 0.2** — `spikes/wasi-p2/`: single `wasi:io/poll` wait at most per turn;
   timers through monotonic subscriptions; TCP bind/listen/connect/accept/read/write;
   cancellation, error completions and subscription-before-stream-before-socket release.
   Wasmtime 44 PASS: 64 concurrent connections, **16,448 actual received bytes echoed
   and verified**, 256 read/write completions, 128 Cancelled→Closed pairs plus listener
   close. Duplicate cancellation/close and a failing accepted operation are checked.
   Timer tests assert completion counts and no early firing, 32 samples each at
   100 µs, 500 µs, 1 ms, 5 ms. Observed lateness ~0.15–2.61 ms across recorded runs.
2. **WASI 0.3** — `spikes/wasi-p3/`: real 0.3 async command, clock futures,
   accept/byte streams and send/receive result futures. Wasmtime 46 PASS: 257-byte
   echo each way, eight short and one long timer, TCP progress concurrently with timers.
   Both mixed p2 packaging on the pin **and pure wasm32-wasip3 on separate
   nightly-2026-09-07** work. The repo pin is unchanged. Pure-target 100 µs timers
   elapsed 0.893–5.516 ms; mixed packaging 1.295–2.559 ms in recorded runs.
3. **Web** — `spikes/web/`: Now-only HostCallback loop, fixed operation capacity,
   u64 tokens, generation/duplicate guards, coalesced scheduling with epoch invalidation,
   timer deadline recheck, fetch + AbortController and WebSocket exchange imports.
   Node wasm contract PASS: ten assertion groups, 257 fetch bytes, actual response
   abort, 257 WebSocket bytes, two-loop routing and 128-operation capacity test.
   Sixteen 1 ms timer samples after deadline recheck elapsed 1.283–2.136 ms.
   Worker Poster PASS: bounded MPSC SharedArrayBuffer ring, two Node producers,
   2,000 unique verified completions, actual parked wake, zero notify calls while
   running, full-ring backpressure and high token bits. Browser isolation requirements
   and the blocking-worker callback starvation problem are documented.
4. **Evaluation** — `spikes/WASM_EVALUATION.md`: feature matrix, corrected TTY and
   timer claims, threading and Perry boundary, fuel method/results, allocation and
   cancellation risks, and proposed mappings to the eventual Backend trait.
5. **Fallback drafts** — `spikes/backend_draft/`: no `trait-v0` exists at the final
   check. Temporary `DraftBackend` interface based on §6; executable p2 adapter;
   fixed-capacity generic web inbox; p3 provider boundary without a pretend implementation.
   Wasmtime draft example PASS: timer completion, cancellation then close, actual wait.
   Pure-Rust web inbox contract PASS: 256 ordered records preserved through rejected
   timeout/output-capacity turns. These are **not** core's shared contract tests.

Read DESIGN.md draft 0.2 and LANES.md completely. No applicable AGENTS.md found.
No other lane's implementation was read or written. Only its trait-tag list was queried.
No pushes, remote additions, publication or native/protocol backend work was performed.

## Verification summary and open gates

| Gate | Final status / evidence |
|---|---|
| Formatting | PASS, all four standalone manifests, final `cargo fmt --check` |
| Target clippy, all targets, `-D warnings` | PASS: p2, p3 packaging, pure p3 on newer nightly, web all features, drafts on p2/web/macOS |
| Stable checks | PASS: installed stable **1.97.1**, p2/web/drafts and p3 using p2 packaging. Environment stated 1.98 but actual version differs |
| Runtime/build dependency audit | PASS: no tokio in any of the four runtime/build dependency trees |
| Source hygiene | PASS: deny unsafe_op_in_unsafe_fn at crate roots; explicit SAFETY comments for every handwritten unsafe block; no `.unwrap()` in lane Rust sources |
| p2 idle and timer-cancel allocations | PASS: 0 allocations after warm-up for 10,000 iterations each |
| p2 poll allocation gate | **FAIL**: `allocation-gate 100` reports 200 allocations, assertion expected 0; retained strict and rerun on final code |
| p3 strict one-wait-per-turn provider | **UNRUN / not implemented**: public wit-bindgen TaskState/wait set stepping unavailable; async spike is not a D7 conformance claim |
| p3 cancel/close/buffer-lifetime and allocation contracts | **UNRUN**: no production p3 provider |
| Chrome HostCallback contract | **UNRUN assertions / FAIL launch commands**: driver warning/startup heuristic, direct `Chrome instance exited`, later legacy navigation HTTP 404 |
| Cross-origin-isolated browser Worker contract | **UNRUN assertions / FAIL WebDriver startup**: session not created, Chrome exited |
| Firefox browser contracts | **UNRUN**: Firefox unavailable; no passing browser claim |
| Full core contract tests | **UNRUN**: no core trait/workspace/tag imported; fallback tests are explicitly separate |
| Linux / Windows execution | **UNRUN**: no hosts; native backends outside this lane's scope |
| WASI OS-thread Poster / detach / shared pool tests | **UNRUN**: component async concurrency does not prove OS threads |
| Bootstrap and Python helpers | PASS: Python syntax compilation and reproducible local tool/driver bootstrap |

Full exact commands, outputs, failed intermediate attempts and reruns are in
[`spikes/verification.jsonl`](spikes/verification.jsonl). The appendix below lists
all distinct logged commands; repeat counts preserve their PASS/FAIL history.
The wrapper `spikes/verify.py` captures both stdout and stderr and propagates failure.
Inner wasm-pack/cargo/WebDriver invocations are printed in those command outputs.

### Fuel method and final numbers

Release cgu=1/LTO; minimum successful fuel threshold T verified against failing
T−1; difference between 100 and 200 iterations; three interleaved fresh-process
rounds with a black-box integer control. All rounds agreed:

| Workload | Fuel/iteration [min,max] | Increment over control |
|---|---:|---:|
| Control | [198.31,198.31] | — |
| Empty Now turn | [14497.31,14497.31] | 14299 |
| Immediately due poll budget | [26934.31,26934.31] | 26736 |
| Timer submit/cancel/Now | [14957.31,14957.31] | 14759 |

These measure **guest fuel**, not host/runtime/syscall instructions. Fixed-capacity
linear scans dominate. Raw final thresholds: `spikes/wasi-p2/results/fuel.json`.
Earlier threshold sets remain in the ledger, superseded after a retirement fix.

## Failures investigated, without relaxing gates

- Generated p2 poll bindings allocate an input handle list and a returned readiness
  list; stream reads allocate a returned byte list. Reserving windlass collections
  cannot fix this. The zero-allocation assertion remains failing.
- Pinned `rustup target add wasm32-wasip3` failed: no prebuilt target artifacts.
  `-Z build-std=std,panic_abort` compiled std but binary link lacked `crt1-command.o`
  and libc. Separate nightly-2026-09-07 supplies the target and passes pure builds/runs.
  Newer Cargo warns that the old unstable config key is unused; builds use `--locked`
  against the lockfile already resolved with the pin and seven-day soak.
- Wasmtime 44's experimental p3 flag cannot link final 0.3 clock imports. Installed
  official Wasmtime 46.0.0 only under ignored `.tools/`; archive SHA-256 matches
  official release metadata: `ab4bdab6ea42a3245cda91cdc6e0430491c4b78ecd643406fc1764ccddbdcd25`.
- Soak rejected wasip3 0.9.0 (four days old); selected 0.8.0. No guest lock override.
- wasm-pack's Cargo installer unexpectedly ignored the project soak; an explicit
  cargo-install config did too. Recent **tool-only** dependencies were downloaded.
  Corrected by copying the official 0.2.108 CLI source into `.tools/`, generating
  a fresh lockfile as a normal project (resolver explicitly says “as of 7 days ago”),
  and building locked. Final Node runs use that build with `--mode no-install`.
  `bootstrap.py` reproduces this route. Project configuration was never disabled.
- Browser startup diagnostics: ChromeDriver serves `/status`, but the older runner
  treats any stderr as fatal (the netmask warning), kills/retries it, and fails.
  Independent W3C sessions fail `session not created: Chrome instance exited`;
  direct headless Chrome exits 134 with no output. A separately started driver
  reaches the legacy runner's navigation step but returns HTTP 404. Browser test
  bodies were not skipped or weakened. The diagnostic driver was stopped via its
  tool session after cross-session process signaling was denied.
- Initial Rust build/clippy failures (FutureReader requires IntoFuture; collapsible
  if; chunks_exact lint) were fixed and gates rerun.
- A draft example's short timer legitimately expired before polling and failed its
  wait-count assertion. Added an explicit immediately-due-budget poll probe, keeping
  both the nonzero wait assertion and timer completion assertion. No timing bound
  was widened or assertion removed. Final example passes.

## Deviations and proposed specification changes

- p2 experiment uses fixed 256-slot linear scans, nongenerational resource indices,
  one operation per socket and small owned buffers. It omits core ref/unref,
  multishot, provided/pooled buffers, UDP/DNS/files, transfer and notifier APIs.
  Completion reservation supports the test bursts, not unrestricted submission.
- p3 async demo uses convenience stream collection/owned futures and allocates.
  A real bounded backend needs dedicated ABI lowering or a supported persistent
  runtime step API. Fresh block_on per turn violates the intended design.
- Web demo implements one WebSocket exchange, not a full persistent multishot API.
  JS host objects/Promises/closures/result arrays allocate. Decide the allocation
  boundary explicitly; do not mark the whole path allocation-free. Browser close
  ends driver ownership, not provable remote transport teardown.
- Replace “ns timer precision” with “ns deadline representation; runtime-dependent
  wake precision”. Measured WASI behavior is millisecond-scale here.
- Replace WASI TTY “size only” with **“terminal detection only”**. Inspected p2/p3
  terminal resources have no size/mode methods; comments reserve future extensions.
- Inline blocking jobs violate D1/D7. Unsupported or a host async interface is needed
  on single-agent targets. An Atomics.wait-blocked Worker cannot also deliver its own
  timer/fetch/WebSocket callbacks; another runnable agent or async scheduling is required.
- Official `wasip3` package is justified as the split WASI bindings crate permitted
  by §13. wasm-bindgen-test/futures are development dependencies only. Tool and fixture
  dependencies do not enter windlass's runtime graph. No tokio.

## Git state and integrator questions

Early and step-1 `git add` attempts failed creating `.git/index.lock`: **Operation
not permitted**. This managed sandbox marks `.git` read-only; no bypass was used.
Checkpoint commits **b69cd67** and **7509aaa** appeared externally during execution
and contain the implementation and helper updates. I did not create or amend them. The final commit attempt also FAILED creating `.git/index.lock` with Operation not
permitted. After the second external checkpoint, the last observed remaining changes are
`LANE_REPORT.md` and `spikes/verification.jsonl`. The attempted command had included
those plus the web README, audit script and bootstrap script, followed conditionally
by `git commit -m 'Finish wasm evaluation and verification report'`; staging failed,
so the commit command was not reached. No per-step commit claim is made.

No `trait-v0` tag exists at the final check, so the authorized fallback drafts were
used. Fetch/merge and `crates/windlass/src/backend/*.rs` integration are UNRUN.
Questions: core's registration/operation ABI and output saturation contract; supported
p3 one-step interface; allocation accounting across canonical ABI/JS host; browser
close semantics; worker scheduling ownership; explicit Unsupported capabilities.

## Next steps

1. Integrator commits remaining report/verification-log updates from this working tree
   if the sandbox still denies local commits, and merges the checkpoint plus updates.
2. Publish the core trait locally and adapt the drafts to its actual operation,
   completion, ownership, capacity, wake and error contracts.
3. Implement/audit allocation-free p2 ABI storage; rerun the still-failing gate.
4. Supply persistent p3 wait-set stepping and cancellation/buffer-lifetime contracts.
5. Run retained Chrome and isolated Worker tests on a working browser host, plus Firefox.
6. Add shared core contracts, full buffer modes, ref/unref, fault injection and long soaks.

## Command appendix

Commands below ran from this clone root. An individual PASS means the command
completed, not that a failed quality gate was waived; see the interpretations above.

| Command | Recorded results; latest |
|---|---|
| `cargo fmt --manifest-path spikes/wasi-p2/Cargo.toml` | PASS ×6; latest **PASS** |
| `cargo clippy --manifest-path spikes/wasi-p2/Cargo.toml --all-targets --target wasm32-wasip2 -- -D warnings` | FAIL ×1, PASS ×5; latest **PASS** |
| `cargo build --manifest-path spikes/wasi-p2/Cargo.toml --release --target wasm32-wasip2` | PASS ×4; latest **PASS** |
| `wasmtime run -S inherit-network=y -W timeout=45s spikes/wasi-p2/target/wasm32-wasip2/release/windlass-wasi-p2-spike.wasm` | PASS ×4; latest **PASS** |
| `cargo +stable check --manifest-path spikes/wasi-p2/Cargo.toml --target wasm32-wasip2 --locked` | PASS ×3; latest **PASS** |
| `wasmtime run spikes/wasi-p2/target/wasm32-wasip2/release/windlass-wasi-p2-spike.wasm allocation-gate 100` | FAIL ×2; latest **FAIL** |
| `cargo fetch --manifest-path spikes/wasi-p3/Cargo.toml` | FAIL ×1, PASS ×1; latest **PASS** |
| `cargo build --manifest-path spikes/wasi-p3/Cargo.toml --target wasm32-wasip3 -Z build-std=std,panic_abort` | FAIL ×2; latest **FAIL** |
| `python3 spikes/wasi-p2/scripts/fuel.py` | PASS ×3; latest **PASS** |
| `git add LANE_REPORT.md .gitignore spikes/wasi-p2 spikes/verify.py spikes/verification.jsonl` | FAIL ×1; latest **FAIL** |
| `wasmtime run spikes/wasi-p2/target/wasm32-wasip2/release/windlass-wasi-p2-spike.wasm timer-cancel 10000` | PASS ×2; latest **PASS** |
| `cargo fmt --manifest-path spikes/wasi-p3/Cargo.toml` | PASS ×2; latest **PASS** |
| `cargo clippy --manifest-path spikes/wasi-p3/Cargo.toml --all-targets --target wasm32-wasip2 -- -D warnings` | FAIL ×1, PASS ×1; latest **PASS** |
| `cargo build --manifest-path spikes/wasi-p3/Cargo.toml --release --target wasm32-wasip2` | FAIL ×1, PASS ×1; latest **PASS** |
| `.tools/wasmtime-v46.0.0-aarch64-macos/wasmtime run -S inherit-network=y -W timeout=45s spikes/wasi-p3/target/wasm32-wasip2/release/windlass_wasi_p3_spike.wasm` | FAIL ×1, PASS ×1; latest **PASS** |
| `cargo +stable check --manifest-path spikes/wasi-p3/Cargo.toml --target wasm32-wasip2 --locked` | PASS ×1; latest **PASS** |
| `wasmtime run -S p3=y,inherit-network=y -W component-model-async=y,timeout=45s spikes/wasi-p3/target/wasm32-wasip2/release/windlass_wasi_p3_spike.wasm` | FAIL ×1; latest **FAIL** |
| `cargo clippy --manifest-path spikes/wasi-p3/Cargo.toml --all-targets --target wasm32-wasip3 -Z build-std=std,panic_abort -- -D warnings` | PASS ×1; latest **PASS** |
| `cargo fetch --manifest-path spikes/web/Cargo.toml` | PASS ×1; latest **PASS** |
| `cargo fmt --manifest-path spikes/web/Cargo.toml` | PASS ×3; latest **PASS** |
| `cargo clippy --manifest-path spikes/web/Cargo.toml --all-targets --target wasm32-unknown-unknown --all-features -- -D warnings` | FAIL ×1, PASS ×2; latest **PASS** |
| `node spikes/web/tests/worker-node.mjs` | PASS ×1; latest **PASS** |
| `cargo +stable check --manifest-path spikes/web/Cargo.toml --target wasm32-unknown-unknown --locked` | PASS ×2; latest **PASS** |
| `wasmtime run spikes/wasi-p2/target/wasm32-wasip2/release/windlass-wasi-p2-spike.wasm idle 10000` | PASS ×2; latest **PASS** |
| `python3 spikes/web/tests/run.py node` | PASS ×3; latest **PASS** |
| `python3 spikes/web/tests/run.py chrome` | FAIL ×2; latest **FAIL** |
| `cargo install wasm-bindgen-cli --version 0.2.108 --root .tools/wasm-bindgen-soaked -Z min-publish-age --config 'registry.global-min-publish-age="7 days"'` | PASS ×1; latest **PASS** |
| `cargo generate-lockfile --manifest-path .tools/wasm-bindgen-source/Cargo.toml` | PASS ×1; latest **PASS** |
| `cargo build --release --manifest-path .tools/wasm-bindgen-source/Cargo.toml --locked` | PASS ×1; latest **PASS** |
| `python3 spikes/web/tests/isolated.py` | FAIL ×1; latest **FAIL** |
| `rustup toolchain install nightly-2026-09-07 --profile minimal --component clippy,rustfmt --target wasm32-wasip3` | PASS ×1; latest **PASS** |
| `cargo fmt --manifest-path spikes/backend_draft/Cargo.toml` | PASS ×2; latest **PASS** |
| `cargo clippy --manifest-path spikes/backend_draft/Cargo.toml --all-targets --target wasm32-wasip2 -- -D warnings` | PASS ×2; latest **PASS** |
| `cargo clippy --manifest-path spikes/backend_draft/Cargo.toml --all-targets --target wasm32-unknown-unknown -- -D warnings` | PASS ×2; latest **PASS** |
| `cargo clippy --manifest-path spikes/backend_draft/Cargo.toml --all-targets -- -D warnings` | PASS ×2; latest **PASS** |
| `cargo test --manifest-path spikes/backend_draft/Cargo.toml` | PASS ×2; latest **PASS** |
| `cargo build --manifest-path spikes/backend_draft/Cargo.toml --example wasi_p2 --release --target wasm32-wasip2` | PASS ×2; latest **PASS** |
| `wasmtime run -W timeout=5s spikes/backend_draft/target/wasm32-wasip2/release/examples/wasi_p2.wasm` | FAIL ×1, PASS ×1; latest **PASS** |
| `cargo +stable check --manifest-path spikes/backend_draft/Cargo.toml --target wasm32-wasip2 --locked` | PASS ×2; latest **PASS** |
| `cargo +stable check --manifest-path spikes/backend_draft/Cargo.toml --target wasm32-unknown-unknown --locked` | PASS ×1; latest **PASS** |
| `cargo +nightly-2026-09-07 build --manifest-path spikes/wasi-p3/Cargo.toml --release --target wasm32-wasip3 --locked` | PASS ×1; latest **PASS** |
| `cargo +nightly-2026-09-07 clippy --manifest-path spikes/wasi-p3/Cargo.toml --all-targets --target wasm32-wasip3 -- -D warnings` | PASS ×1; latest **PASS** |
| `.tools/wasmtime-v46.0.0-aarch64-macos/wasmtime run -S inherit-network=y -W timeout=45s spikes/wasi-p3/target/wasm32-wasip3/release/windlass_wasi_p3_spike.wasm` | PASS ×1; latest **PASS** |
| `python3 spikes/audit.py` | PASS ×1; latest **PASS** |
| `cargo fmt --manifest-path spikes/wasi-p2/Cargo.toml --check` | PASS ×1; latest **PASS** |
| `cargo fmt --manifest-path spikes/wasi-p3/Cargo.toml --check` | PASS ×1; latest **PASS** |
| `cargo fmt --manifest-path spikes/web/Cargo.toml --check` | PASS ×1; latest **PASS** |
| `cargo fmt --manifest-path spikes/backend_draft/Cargo.toml --check` | PASS ×1; latest **PASS** |
| `git -C ../core tag -l 'trait-v*'` | PASS ×2; latest **PASS** |
| `rustc --version` | PASS ×1; latest **PASS** |
| `rustc +stable --version` | PASS ×1; latest **PASS** |
| `cargo +stable --version` | PASS ×1; latest **PASS** |
| `wasmtime --version` | PASS ×1; latest **PASS** |
| `wasm-pack --version` | PASS ×1; latest **PASS** |
| `node --version` | PASS ×1; latest **PASS** |
| `rustc --print target-list` | PASS ×1; latest **PASS** |
| `rustup target list --installed` | PASS ×1; latest **PASS** |
| `rustup target list --toolchain stable --installed` | PASS ×1; latest **PASS** |
| `rustup target add wasm32-wasip3` | FAIL ×1; latest **FAIL** |
| `rustup component add rust-src` | PASS ×1; latest **PASS** |
| `cargo fetch --manifest-path spikes/wasi-p2/Cargo.toml` | PASS ×1; latest **PASS** |
| `cargo check --manifest-path spikes/wasi-p2/Cargo.toml --target wasm32-wasip2` | PASS ×1; latest **PASS** |
| `wasmtime run --help` | PASS ×1; latest **PASS** |
| `wasmtime run -S help` | PASS ×1; latest **PASS** |
| `wasmtime run -W help` | PASS ×1; latest **PASS** |
| `shasum -a 256 .tools/wasmtime-v46.0.0-aarch64-macos.tar.xz` | PASS ×1; latest **PASS** |
| `.tools/wasmtime-v46.0.0-aarch64-macos/wasmtime --version` | PASS ×1; latest **PASS** |
| `git add LANE_REPORT.md` | FAIL ×1; latest **FAIL** |
| `'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' --headless=new '--user-data-dir=$PWD/.tools/chrome-direct-profile' --no-first-run --disable-crash-reporter --disable-breakpad --dump-dom about:blank` | FAIL ×1; latest **FAIL** |
| `curl --noproxy '*' -sS --max-time 3 http://127.0.0.1:9515/status` | PASS ×1; latest **PASS** |
| `curl --noproxy '*' -sS --max-time 3 'http://[::1]:9515/status'` | PASS ×1; latest **PASS** |
| `git diff --check` | PASS ×1; latest **PASS** |
| `git status --short --branch` | PASS ×1; latest **PASS** |
| `git check-ignore .tools/wasmtime-v46.0.0-aarch64-macos/wasmtime spikes/web/target` | PASS ×1; latest **PASS** |
| `python3 spikes/web/tests/bootstrap.py` | PASS ×1; latest **PASS** |

| `git add LANE_REPORT.md spikes/verification.jsonl spikes/web/README.md spikes/audit.py spikes/web/tests/bootstrap.py` | FAIL: `.git/index.lock`: Operation not permitted; final conditional commit not reached |
