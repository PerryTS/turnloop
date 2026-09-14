# wasm2 lane report

Work in progress, 2026-09-14. All required design/contribution/integration and
wasm/core/CI lane documents read. Checked-in Backend now identifies revision 2
(empty-wait counters); revision-1 method/ownership contracts are preserved.
The integrator commits periodically; no git mutation performed by this lane.

## Implemented

- WASI p2 TCP/listen/UDP, pooled/provided buffers, writev/shutdown, intrusive FIFO
  operation queues, synchronous cancel acknowledgement, one poll per turn and
  monotonic-clock deadlines. Reusable canonical input/return storage removes both
  reported poll allocations and read/UDP list allocations. Empty lists handled.
- Experimental WASI p3 persistent raw wait-set, pinned async request areas,
  TCP streams/futures, UDP subtasks, deadline subtasks, synchronous cancellation,
  cached EOF/shutdown future results. One cooperative host yield is necessary on
  turn(Now) to prevent socket starvation; strict boundedness is NOT claimed.
- Web HostCallback with asynchronous coalescing, real fetch/AbortController and
  binary WebSocket imports, timeout validation, cancellation generation checks,
  unsupported-operation errors. Feature web-worker adds bounded SAB/Atomics MPSC
  numeric Poster with waitAsync, backpressure, closure invalidation and no ticks.
- Shared WASI contracts and browser/Node scenarios; real fixture checks fetch,
  abort and WebSocket byte counts. Allocation gates extended to UDP/deadlines,
  Rust web I/O/timers/posts. Worker test uses two actual producers/2,000 messages.
- CI wasm placeholders replaced: target metadata, positive counts for each
  binary/browser, separately attempted browsers, owned fixture cleanup, pinned
  Wasmtime/nightlies/wasm-pack and soaked wasm-bindgen CLI build. Fan-in retained.

## Current verification

The exact chronological ledger is `.tools/wasm2/commands.jsonl`; its commands
and PASS/FAIL/UNRUN classifications will be included here before handoff.

- PASS native workspace strict Clippy (all targets/features), stable workspace
  check (all targets/features), workspace tests (existing server ignores retained).
- PASS p2 and web full workspace strict cross-target Clippy; p3 core/contract
  strict Clippy. p3 full workspace Clippy FAIL: transitive getrandom 0.3.4 rejects
  p3 (MongoDB -> BSON -> rand 0.9); no insecure RNG workaround added.
- PASS p2 three allocation gates, including 6,400 UDP bytes and 20 actual deadline
  expiries at zero allocations. p2 shared scenarios other than precision PASS.
- PASS p3 debug shared scenarios other than precision (15), including repeated
  EOF/shutdown and empty datagrams. New queued-read cancellation test pending.
- FAIL full WASI precision: unchanged <500us median threshold; p2 median lateness
  1.03475ms, p3 2.346875ms. CI still runs the failing test, without skip/relaxation.
- FAIL p3 debug custom-allocator harness startup: canonical get-arguments realloc
  executes with context stack pointer zero. Release original allocation subjects
  PASS, but added UDP allocation subject traps, as does release shared UDP without
  custom allocator. Inspecting emitted component shows stack pointer mapped to
  canon context.get/set 0; list lowering precedes the zero-stack fault. Root cause
  attribution to toolchain/runtime versus direct provider remains under analysis.
- PASS Node six contracts and fixture proof: 2 HTTP requests, 1 abort, 3 WebSockets,
  6,657 echoed bytes; zero Rust steady-state allocations and 2,000 Worker posts.
- UNRUN Chrome (sandbox): ChromeDriver cannot launch, zero tests executed.
- UNRUN Firefox (sandbox): geckodriver starts, headless Firefox session fails HTTP
  500/SIGKILL, zero tests executed. Exact per-browser reruns will be listed.
- PASS seven-day soak: 208 locked registry packages. PASS no-tokio every target
  and feature configuration, including WASI/web. Policy unchanged.
- PASS checksum-pinned tool installers, 16 CI adversarial tests, workflow lint
  wrapper, direct zizmor (zero findings). Raw actionlint FAIL: pre-existing
  concurrency.queue unsupported by pinned 1.7.12; compatibility wrapper unchanged.
- UNRUN Linux/Windows runtimes (no hosts). No server integration claims.

## Deviations / open questions

P3 remains experimental: host cooperative yield lacks a bounded scheduling
contract; variable-length canonical returns need a correct reusable allocator
and stack/context support; UDP release regression is a blocker. Testing a newer
nightly solely for p3 to determine whether it resolves that fault. No production
p3 compliance claim or green CI claim. Web allocations are bounded Rust storage;
JavaScript/browser fetch, messages, Promises and GC are outside that gate.

DESIGN.md remains authoritative and unchanged. Proposed clarifications: name the
web host allocation boundary; formalize binary WebSocket message semantics and
fetch response-size failure; specify the host-yield budget and runtime timer
precision obligations before promoting p3.

## Next steps

Finish p3 diagnosis, queue-cancellation regression, backend audit/docs, final
format/affected checks and exact verification ledger. Preserve all failing gates;
integrator must rerun both browsers and WASI timing outside sandbox/on CI hosts.
