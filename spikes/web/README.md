# Web HostCallback spike

`WebLoop` (wasm32-unknown-unknown + wasm-bindgen) owns a fixed-capacity operation
registry and a reserved completion queue. All browser/Node operations go through
`host.js`: setTimeout/clearTimeout, fetch/AbortController, and a one-exchange
WebSocket transport. JS callbacks post an operation ID, result kind and payload;
Rust checks the generation and recovers the host's **lossless u64 token**. Duplicate
and stale callbacks cannot complete an operation twice. The only supported turn
is `turn(0)` / Now. Other timeouts fail before mutating the queue.

`schedule_turn` runs in a microtask, coalesced until the host calls turn. An epoch
invalidates obsolete queued callbacks when the host turns synchronously. The driver
does not dispatch the user's completion handlers. Polling/phase order belongs to
Perry. Timer callbacks check a monotonic deadline and rearm if the host fires early.
Cancellation is terminal before abort/late rejection can post another result.
Close appends Cancelled then Closed; JS records remain until a following turn.

## Reproduction

From the clone root:

```sh
cargo fmt --manifest-path spikes/web/Cargo.toml
cargo clippy --manifest-path spikes/web/Cargo.toml --all-targets --target wasm32-unknown-unknown --all-features -- -D warnings
cargo +stable check --manifest-path spikes/web/Cargo.toml --target wasm32-unknown-unknown --locked
python3 spikes/web/tests/bootstrap.py
python3 spikes/web/tests/run.py node
python3 spikes/web/tests/run.py chrome
node spikes/web/tests/worker-node.mjs
python3 spikes/web/tests/isolated.py
```

The test wrapper starts/stops a Node fixture at 127.0.0.1:18765, uses wasm-pack
`test --node` or `test --headless --chrome --features browser`, and verifies server
counters for real fetches, AbortController cancellation and WebSocket bytes.
The fixture implements just the binary WebSocket echo frames needed by the test;
it is test support, not a production protocol crate. `contract.js` runs in wasm
on Node/Chrome via wasm-bindgen-test. The browser feature selects run_in_browser.

PASS on Node 26.5.1: lossless tokens, coalescing including a stale scheduling race,
Now-only rejection, delayed/duplicate callbacks, cancellation/close, slot reuse,
per-loop routing, 257 fetch bytes, 257 WebSocket echo bytes, 16 timer samples,
128-entry capacity/backpressure. The fixture independently asserts 2 fetches,
1 aborted response, 1 WebSocket and 257 bytes echoed.

**Chrome assertions UNRUN**. `wasm-pack test --headless --chrome` was attempted:
ChromeDriver 153.0.8010.36 starts, emits `FromSockAddr failed on netmask`, and the
0.2.108 test runner treats *any stderr* as fatal, kills/retries it, then reports
`driver failed to bind port during startup`. A direct local WebDriver session
bypasses that runner startup heuristic but also fails: `session not created:
Chrome instance exited`. A direct headless Chrome invocation exits 134 with no
output. With a separately started driver, the legacy test-runner session path also fails
with HTTP 404 during navigation. The browser and isolated Worker contracts are retained for a
host where Chrome launches. Their command failures are in `../verification.jsonl`.
No Chrome timer precision has been measured. Firefox is UNRUN/not installed.

## Tool installation and soak

All tool caches stay in ignored `.tools/`. wasm-pack's default Cargo installer
ignored the project's publish-age config; even explicit `cargo install -Z
min-publish-age --config ...` did not filter recent transitive tool dependencies.
To avoid relying on that installation, copied the official downloaded
wasm-bindgen-cli 0.2.108 crate source to `.tools/wasm-bindgen-source`, generated a
fresh Cargo.lock there with the root seven-day soak, then built it with `--locked`.
The resolver explicitly reported `as of 7 days ago`, selecting older bitflags, cc,
ureq, jiff, etc. The wrapper uses that build's PATH and `--mode no-install` so
wasm-pack cannot replace it. The guest crate lockfile always honored the soak.
The tool's many dependencies are build tooling, not windlass runtime dependencies.

## Worker Poster

`worker-poster.js` is a bounded MPSC numeric ring in SharedArrayBuffer. Producer
acquisition is compareExchange; a failed try-post leaves ownership with the caller.
The consumer publishes PARKED, rechecks the queue, then waits on a sequence value.
Producers store payload before publishing head, increment the sequence, and call
Atomics.notify **only when PARKED**. A mismatch before Atomics.wait closes the
lost-wake race. No periodic timeout is used in the wait. Slots carry u64 token and
u64 payload as four 32-bit words. This is a host ring prototype, not shared wasm
memory or a Rust Send+Sync Poster implementation. Contended/full callers need
backpressure and asynchronous retry, not a browser-main-thread spin.

Node worker_threads PASS: two producers, 2,000 unique verified completions; an
actually parked consumer was awakened; fast-path notify count zero. Full-ring
backpressure and lossless high token bits are asserted. `isolated.html` tests a
browser Worker with 1,000 completions; its assertions are UNRUN due to Chrome startup.

The browser test server serves `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` on a trustworthy loopback origin.
Production needs HTTPS and compatible CORS/CORP for embedded resources. Verify
`crossOriginIsolated` before constructing/sharing the buffer. See
[MDN crossOriginIsolated](https://developer.mozilla.org/en-US/docs/Web/API/Window/crossOriginIsolated)
and [Atomics.wait](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Atomics/wait).
Without isolation, use one loop per worker and postMessage. Atomics.wait is forbidden
on the browser main thread. In a worker it blocks that worker's JS callbacks too:
put asynchronous I/O callbacks on an agent that remains runnable and posts into
the ring, or use async wait/scheduling instead. Merely moving the whole browser
backend into a blocking worker can deadlock fetch/timer/WebSocket delivery.

## Limits relevant to integration

- The JS facade builds result arrays, host APIs allocate closures/Promises/controllers
  and returned byte arrays, and the worker convenience take() returns arrays/BigInts.
  The complete path does not meet the zero-heap-allocation gate. Reserved Rust queue
  storage alone is not an allocation audit of the browser.
- The one-exchange WebSocket operation is not a persistent multishot websocket API.
- Closed means driver ownership ended. Browser transport close/abort is asynchronous;
  a strict “underlying network resource already released” guarantee is unavailable.
  Generation checks and retention guard against late callbacks, not remote FIN timing.
- No Rust ref/unref, shared wasm-memory allocator, detach/attach, streams or OPFS yet.
  Browser raw TCP/UDP/listen/process/signal/TTY requests need Unsupported at the core
  API boundary. Node APIs extending those capabilities are outside this browser backend.
