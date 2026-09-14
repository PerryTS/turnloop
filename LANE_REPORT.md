# WASM lane report

Branch: `lane/wasm`. Scope: WASI 0.2, WASI 0.3, web spikes, evaluation, then core backend adapters if `trait-v0` is available.

## Implemented

- Read DESIGN.md draft 0.2 and LANES.md completely; confirmed lane ownership.
- Created standalone spike directories. No core Backend tag was available at first check.

## Verification ledger

Commands run from the clone root unless specified. Tool output retained where measurements matter.

| Command | Result |
|---|---|
| `git status --short --branch`, `git log -5 --oneline` | PASS: clean lane/wasm at b2e4611 initially |
| `git -C ../core tag -l 'trait-v*'` | PASS: no tags initially |
| `cat DESIGN.md LANES.md`, chunked reads; ancestor/local AGENTS.md discovery | PASS: design and lane rules read; no applicable AGENTS.md found |
| `cat rust-toolchain.toml .cargo/config.toml .gitignore` | PASS: pinned nightly and 7-day dependency soak retained |
| `cargo fmt`, target clippy, stable check, runtime tests | UNRUN: implementation not yet present |

## Deviations and proposed spec changes

Pending experimental results. No specification changes made.

## Open questions for integrator

- Backend trait is pending; will recheck after spikes.

## Next steps

1. WASI 0.2 bounded poll loop, timer/TCP/cancellation tests, fuel measurements.
2. WASI 0.3 toolchain/runtime and async wait-set prototype.
3. Browser/Node host callback and worker Poster tests.
4. Capability evaluation and core trait integration or drafts.

## Step 1: WASI 0.2

Implemented standalone bounded `wasi:io/poll` driver, monotonic subscriptions,
TCP listen/connect/accept/read/write, explicit cancellation and delayed resource
release, 64-connection verified echo, timer sampling, and guest-fuel threshold harness.

- PASS: target check, release build, target clippy with all targets and warnings denied,
  stable 1.97.1 target check. Initial clippy failed on `chunks_exact`; corrected to
  `as_chunks`, gate rerun successfully.
- PASS: Wasmtime 44 `run -S inherit-network=y -W timeout=45s ...` — 64 concurrent
  connections, 16,448 bytes verified, 256 I/O completions, 128 cancellation/close
  pairs plus listener close. 32 samples/deadline (100 µs, 500 µs, 1 ms, 5 ms).
  Observed lateness ~0.15–2.60 ms across first runs, not nanosecond wake precision.
- FAIL: `... allocation-gate 100`: 200 allocations, expected zero. Generated poll
  bindings allocate both argument and result lists; stream reads also allocate.
  This gate remains strict and failing; production zero-allocation acceptance is blocked.
- Exact commands, output, intermediate failures and reruns: `spikes/verification.jsonl`.
- FAIL: `git add LANE_REPORT.md && git commit -m 'Record wasm lane scope and initial verification'`:
  `fatal: Unable to create '.../wasm/.git/index.lock': Operation not permitted`.
  The managed sandbox makes `.git` read-only. No bypass attempted; changes remain
  in the working tree. Step commits cannot be created in this environment.
- Environment: actual stable is rustc/cargo 1.97.1, not the stated 1.98.

Spec proposals: distinguish nanosecond timestamp representation from measured
runtime timer resolution; decide how the strict allocation gate applies to the
component ABI and JS host. These are unresolved gates, not waived requirements.

## Step 2: WASI 0.3

- Implemented working timer + TCP echo over genuine WASI 0.3 futures/streams,
  packaged as a wasip2 cdylib with a p3 async command export. Wasmtime 46 PASS:
  257 echoed/verified bytes, eight short and one long concurrent timer completions.
- Installed official Wasmtime 46 archive into ignored `.tools/`, verified SHA-256
  against release metadata. No system installation or repo pin changes.
- Pinned target recognized but prebuilt artifacts absent. Source build of std
  reaches link and fails on missing libc/startup objects. Detailed route and sources
  in `spikes/wasi-p3/README.md`.
- FAIL (resolved): wasip3 0.9.0 rejected by soak; older 0.8.0 selected. Early build
  errors used FutureReader as Future; corrected via IntoFuture. Final target clippy
  and packaging build PASS. Stable check recorded in ledger.
- Unresolved: stock wit-bindgen owns a private wait set and block_on loops until
  completion. No honest strict D7 adapter using only its public stepping surface.
  Proposed persistent step API / dedicated ABI lowering, or explicit async host
  scheduling amendment. Full p3 contract and allocation gates UNRUN.
- Step commit blocked by the already-observed read-only `.git` sandbox.

## Step 3: Web

Implemented wasm-bindgen HostCallback loop, bounded registry, u64 tokens,
generation checks, Now-only turns, coalesced scheduling with stale-task invalidation,
timer deadline rechecking, fetch AbortController, WebSocket exchange, and MPSC
SharedArrayBuffer/Atomics.notify worker prototype. Rust target clippy and stable
check PASS. Node wasm contract and 2,000-completion/two-producer worker test PASS.

Browser gate command FAIL before assertions; browser tests UNRUN. ChromeDriver's
netmask warning triggers an overbroad test-runner startup failure; independent
WebDriver sessions also fail with `Chrome instance exited`. Direct Chrome exits
134. Browser/isolated Worker tests remain ready for a host with working Chrome.

Tooling deviation discovered and remedied: wasm-pack's cargo-install dependency
resolution did not honor the project soak (nor did an explicit install config).
Rebuilt the official CLI crate source as a regular standalone project in `.tools/`
with a fresh soaked lockfile; subsequent tests use it and `--mode no-install`.
Guest dependency resolution always preserved the soak. No repo config was weakened.

JS host allocations, asynchronous transport teardown, worker callback starvation
when blocking, and unsupported browser capabilities are documented in the web
README. These require integrator decisions; they are not passed production gates.
Step commit remains blocked by `.git` being read-only.
