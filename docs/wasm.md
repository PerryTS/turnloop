# WASI and web backends

The wasm adapters implement the checked-in Backend contract, including its
revision-1 clock/deadline/timeout hooks and revision-2 empty-wait counters.
`Driver` remains the only completion/token arbiter. Backends never dispatch user
callbacks from inside `turn`. The WASI 0.3 provider is **experimental**, with the
remaining promotion blockers below. See the root lane report for actual results;
compilation or an ignored/filtered test is not a runtime pass.

## Target selection and tools

| Target | Provider | Rust pin | Runtime/tools |
| --- | --- | --- | --- |
| wasm32-wasip2 | `backend::wasi_p2::WasiP2` | nightly-2026-08-20; stable checked | Wasmtime 46.0.0 |
| wasm32-wasip3 | `backend::wasi_p3::WasiP3`, feature `wasi-p3-experimental` | nightly-2026-09-07, only this target | Wasmtime 46.0.0 |
| wasm32-unknown-unknown | `backend::web::Web` | nightly-2026-08-20; stable checked | wasm-bindgen 0.2.108, wasm-pack 0.15.0, Node 26.5.1, Chrome and Firefox |

The p3 pin comes from the wave-1 wasm report. A diagnostic build with
nightly-2026-09-13 did not fix the canonical stack-context issue, so it was not
adopted. The repository's default toolchain and seven-day dependency soak remain
unchanged. `scripts/ci/soak.py` also checks already-locked versions. The exact
wasip2 1.0.3 (WASI 0.2.9) and wasip3 0.8.0 (WASI 0.3.0) bindings are intentionally
pinned: hand-lowered canonical layouts must be re-audited when bindings change.

Wasmtime and wasm-pack releases are SHA-256 pinned in `scripts/ci/tools.json`.
Use `bash scripts/ci/install-wasmtime.sh` and
`python3 scripts/ci/install-tools.py wasm-pack`. `install-web-tools.py` copies the
registry-checksummed wasm-bindgen-cli 0.2.108 source into a normal isolated Cargo
project under `.tools`, resolves under the pinned seven-day policy, then builds
with `--locked`. This avoids a separate unsoaked `cargo install` resolution.
CI places the resulting binaries on PATH. Local setup:

```sh
python3 scripts/ci/install-web-tools.py
export PATH="$PWD/.tools/bin:$PWD/.tools/wasm-bindgen-source/target/release:$PATH"
export WASM_PACK_CACHE="$PWD/.tools/wasm-pack-cache"
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3
python3 scripts/ci/run-tests.py web
python3 scripts/ci/run-tests.py node
```

The WASI runner grants loopback/network access and sets a 120-second execution
limit. WASI contract and allocation binaries are checked independently for
positive test counts; allocations use release builds with the same zero threshold.
P3's debug custom allocator currently traps before the test harness starts (see
below). Browser runs are attempted separately, so a Chrome failure cannot hide
Firefox. Each run owns a fresh ephemeral-port HTTP/WebSocket fixture and asserts
fetch, AbortController disconnect, connected socket and echoed byte counters.
Fixtures are terminated even on failure. No spike is substituted for production
contract execution. CI retains failures in the required `wasi`/`web` fan-in jobs.

## WASI 0.2

Each turn executes cached runnable I/O, then at most one `wasi:io/poll.poll` with
an optional monotonic-clock deadline pollable. Idle sockets retain the requested
positive deadline after WouldBlock. Deadline pollables are dropped at turn end;
socket/stream subscriptions are owned persistently and dropped before their
parents. There is no periodic timer tick and no retry-until-ready wait loop.

Operation queues are intrusive, preallocated per direction. Partial writes and
inline writev segments retain offsets; flush acknowledges the total write, not
just the current segment. Cancel unlinks an operation and queues its terminal
acknowledgement. Close releases a transport only after that acknowledgement and
after core has appended `Closed` to the host's output.

Generated p2 `poll`, `read`, and UDP receive bindings allocate returned lists.
The adapter uses reusable input handles, owners, result indices and aligned
canonical return storage. A scoped `cabi_realloc` override lowers these specific
synchronous imports into preallocated storage; returned pointers are copied/read
within the scope and never converted to owning Vecs. All unrelated canonical
allocations use the ordinary Rust allocator. Large TCP reads progress in bounded
scratch-sized chunks. UDP preserves one datagram, with normal buffer truncation.

The allocation gate checks provided/pooled TCP transfers, accepts, timer batches,
backlogged cancel/close, 100 UDP datagrams (6,400 bytes) and 20 actual deadline
expiries. Every measured subject asserts its own bytes/events before zero counts.

## WASI 0.3: experimental constraints

The provider owns one persistent canonical waitable set. Each operation owns a
fixed return area and keeps stream buffers alive until return or synchronous
cancellation acknowledgement. TCP uses raw stream/future vtables rather than
building executor futures per operation. Accept/read/write directions are FIFO;
cancelling an armed head makes its queued successor runnable. EOF and shutdown
results are cached because a canonical future can be consumed only once.

Raw async socket imports and wait-set operations preserve the caller's shadow
stack context through the pinned compiler's `env.__wasm_get_stack_pointer` and
`env.__wasm_set_stack_pointer` imports. In emitted p3 components those map to
`canon context.get/set 0`. Without restoration across the wait-set step,
release-mode UDP return-list lowering left context 0 at zero and the next Rust
stack access trapped. The release UDP and full I/O regressions now pass with
restoration; debug-only tests had failed to expose this. This is part of the
experimental ABI surface, not a portable assumption about future toolchains.

There are still promotion blockers:

1. `waitable-set.poll` alone does not schedule host socket subtasks when the owner
   repeatedly calls `turn(Now)`. One cooperative `thread-yield` before one poll
   allows I/O to progress under repeating-timer/post backlog. No guest wait loop
   or fresh `block_on` is used, and the no-spin contract passes. However the host
   yield has no demonstrated wall-time budget. A host primitive that advances
   ready subtasks with an explicit work/time bound, or a documented bound for
   this yield, is required before claiming strict D7 boundedness.
2. UDP receive returns an owned canonical `list<u8>`. It currently allocates once
   per successful nonempty datagram: the strict gate measures **100 allocations
   for 100 receives**, then fails against zero. Correct reuse needs an allocator
   for asynchronous returns that preserves per-task context and owns returned
   storage until completion/cancel; p2's synchronous scratch scope cannot simply
   be copied. Results can arrive on a later turn and multiple requests can be in
   flight. Error variants can also own strings. No unsafe scratch ownership
   shortcut or relaxed allocation threshold is used.
3. On the pinned p3 compiler, a debug custom GlobalAlloc makes the harness's
   `wasi:cli/environment.get-arguments` canonical realloc access a zero shadow
   stack before tests start. Release allocation tests do start and exercise their
   subjects. Debug allocator-entry context/stack initialization needs an upstream
   fix or an audited canonical allocator trampoline. Backend context restoration
   does not fix this pre-backend startup path.
4. Workspace p3 Clippy reaches MongoDB -> BSON -> rand 0.9 -> getrandom 0.3.4,
   which rejects p3. Core/contract p3 Clippy passes. The protocol dependency needs
   a soaked p3-capable implementation; no insecure RNG substitute is enabled.

On this macOS/Wasmtime combination, p2 and p3 also fail the existing shared
`timer_precision` median-lateness ceiling of 500 microseconds (approximately
1.03 ms and 2.35 ms in debug; p3 release still measured 1.08 ms). Nanosecond deadlines are passed without a
backend-added millisecond floor. The tests stay enabled and the gate stays red;
a host/runtime timing investigation is necessary. Separate filtered coverage
runs are reported as such and never used as CI gate passes.

## Web host integration

Install `Loop::set_schedule_turn` with a JS function retained by the host. The
function should call the owner's Rust export which performs `turn(Timeout::Now)`
and dispatches returned completions. Use `loop.now()` for deadlines; it is based
on `performance.now()`. HostCallback mode arms one rechecked host timer for the
current earliest deadline and removes it on cancellation. `queueMicrotask`
coalesces wakes, and an epoch suppresses stale scheduled dispatch after a turn.
No callback runs synchronously during a driver method. The loop never blocks
the browser main thread: even `After(Duration::ZERO)` is rejected with Unsupported.

`fetch(url, buffer, token)` opens a fetch handle and reads one complete successful
HTTP response. `websocket(url, token)` opens a browser WebSocket and completes
Connected. Subsequent reads/writes exchange whole binary messages. A response or
message exceeding the provided/pool buffer completes with ResourceLimit. There
is one outstanding read per web handle; a second is rejected with WouldBlock.
Multishot reads and writev are Unsupported. Host identity plus the complete
64-bit generational OpId guards callbacks; core retains all 64 token bits.
Cancelling fetch invalidates the operation before AbortController.abort, so late
promise rejection cannot affect a reused slot. Drop invalidates host callbacks
and clears timers/resources.

The zero-allocation web gate counts **Rust guest allocations** around synchronous
submission/turn paths: 100 real 64-byte WebSocket exchanges plus posts and timer
cancel/close, with actual byte/completion assertions. JavaScript fetch bodies,
Uint8Arrays, Promise callbacks and browser networking/GC allocate on the host.
They are not counted as zero, nor does this gate promise a bounded browser heap.
The message queue is bounded in message count by operation capacity. Fetch uses
the browser response-body API and buffers a response before the size check.
This host allocation boundary requires explicit design acknowledgement for a
stronger process-wide no-allocation promise.

## Worker Poster (`web-worker` feature)

`loop.worker_poster(power_of_two_capacity)` returns a JS descriptor containing
`buffer` (SharedArrayBuffer), `capacity` and `producerSource` (SharedPoster class
source for bootstrapping a Worker). Only one ring attaches to a loop. The producer
class can also be imported from `backend/web/host.js` via a bundled application;
applications with strict CSP should bundle the class rather than evaluate source.
Transfer the descriptor's buffer by structured clone, not by transferring Rust
linear memory. The owner drains into its ordinary Poster as `(Token, Payload::U64)`.
`post(tokenBigInt, valueBigInt)` returns false on contention, full or closed; the
producer retains the values and may retry on its own scheduling turn.

The ring uses Atomics for a producer lock, published head/tail, wake sequence and
parked handshake. Producer notification is coalesced. The consumer uses
`Atomics.waitAsync`, drains at most capacity records per call, keeps records when
the core queue is full, and resumes on the next owner turn. It never uses a timed
polling tick or blocks the main thread. Drop marks the ring closed and wakes its
waiter; delayed resolution cannot invoke the dropped Rust callback. Consumers
must not run two loop owners over the same descriptor.

Browsers must serve the document over HTTPS (or localhost), with:

```text
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

`crossOriginIsolated`, SharedArrayBuffer and Atomics.waitAsync must be available;
otherwise creation returns Unsupported. Subresources must satisfy COEP using
same origin, CORS or suitable CORP headers. The wasm-bindgen-test server sends
isolation headers. Node's Worker test uses worker_threads without DOM APIs. The
same test runs in isolated browsers, fills the ring to prove rejection, then
checks 2,000 unique full-width messages from two real workers and no idle wakes.

## Explicit contract exclusions

These are platform exclusions, not successful tests. Required WASI timer/UDP
and allocation failures are **not excluded**.

| Contract family | WASI p2/p3 | Web/Node |
| --- | --- | --- |
| TCP connect/listen/accept, UDP, writev, shutdown | Exercised; reuse-port Unsupported, nodelay remains a hint because bindings lack a setter | Native socket cases excluded: platform lacks raw sockets. Actual Unsupported results tested; fetch/WebSocket byte paths replace transport workloads |
| Blocking pool / native cross-thread wake and 8-peer posting | Excluded: these single-agent targets cannot spawn OS threads; Running-notify/post behavior exercised | Native threads/pool excluded and Unsupported tested; two actual JS Workers exercise SAB posting |
| Native detach/attach transfer | Excluded: WASI resource transfer Unsupported (accept attaches owned transport internally) | Excluded: browser transport transfer Unsupported, tested |
| Integration fd/event, POSIX signal EINTR, kqueue/epoll/IOCP specifics | Excluded: runtime-owned integration, no native fd | Excluded: HostCallback and zero OS waits |
| Native sub-ms timer ceiling | Required unchanged, currently FAIL on this host | Native ceiling excluded: browser timers are clamped. Host-clock tests require no early firing, bounded lateness, 60 actual expiries and one scheduled turn each |
| Real Postgres/MySQL/Redis/SMTP/MongoDB server integration | Outside backend contract scope; existing native protocol fixtures remain separate | Raw-socket protocol integration excluded by platform capability; sans-IO protocol crates still cross-check |

For browser reruns outside the sandbox, use the setup/PATH above and:

```sh
python3 scripts/ci/run-tests.py web --browser chrome
python3 scripts/ci/run-tests.py web --browser firefox
```
