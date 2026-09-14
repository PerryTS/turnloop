# WASI and web backends

The wasm adapters implement the checked-in Backend contract, including its
revision-2 native capability methods, clock/deadline/timeout hooks and empty-wait counters.
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
python3 scripts/ci/install-wasm-toolchain.py
source .tools/wasm-env.sh
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
positive test counts with all features (including the executor); allocations use release builds with the same zero threshold.
The semantic binaries receive an explicit 36-byte stdin fixture, and must read it
through turnloop, write to both output streams and close all three handles.
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

Strict host-progress/cancellation boundedness and ABI portability remain
unproven. See [the precise upstream requirements](upstream/wasi-p3-wait.md).
The backend remains opt-in; passing no-spin does not remove that feature gate.

### Canonical UDP storage and debug allocation harness

The allocation source was host canonical lowering of each received `list<u8>`,
not address conversion or the provided/pool output buffer. In release, scoped
`cabi_realloc` redirects raw async socket/wait-set return lists into retained
64 KiB slots. There is one reservation per live UDP socket (one receive head per
socket), retained to the high-water mark until the last backend drops. All loops
on the agent share the arena because host progress can return another loop's
subtask. Busy slots are never reset at turn boundaries. Returned bytes are
copied to the requested output and the slot is released only on consumption or
acknowledged cancellation; error text is also consumed without generated String
destruction. Unrelated imports and oversized exceptional error strings use the
ordinary allocator. The measured steady path remains strictly zero allocations.

Regression subjects cover the original 100 datagrams/6,400 bytes, 320 concurrent
IPv6 datagrams with provided/pooled outputs (0 through 8,192 bytes), 320 real
cancellations, output capacity one, two loops, pending drop and surviving-loop
reuse. A direct canonical test initializes two full 65,536-byte lists, verifies
both contents survive scope/owner changes, and asserts reuse only after release.
The wire maximum is 8 KiB because a direct host socket probe accepts 8 KiB but
rejects 16/60 KiB with EMSGSIZE on this Mac; canonical capacity is tested directly.

**p3 allocation gates are release-only.** The pinned compiler lowers the custom
GlobalAlloc harness's unoptimized allocator entry using context-slot-0 stack
storage. The `get-arguments` canonical realloc enters with that slot zero before
Rust's harness starts; unoptimized stack access traps. Release inlining avoids
that startup path. Backend stack restoration cannot repair a pre-main entry.
The custom canonical allocator is likewise release-only; debug uses std's default
owned lists. There is no debug zero-allocation claim. The required WASI CI job
runs debug semantic/no-spin contracts, release semantic/precision contracts,
release canonical-storage unit tests, and release allocation gates, requiring
positive counts for every binary. The zero threshold is unchanged.

### p3 entropy and the complete workspace

BSON 3.1.0 requires rand 0.9; its newest compatible release 0.9.5 (2026-07-11)
still uses rand_core 0.9/getrandom 0.3.4. getrandom 0.3.4 (2025-10-14) is the last
0.3 release and has no p3 backend. Soak-eligible getrandom 0.4.3 (2026-06-17) has
p3 support, but cannot satisfy that dependency's 0.3 requirement. Registry API
and the exact locked manifests were checked on 2026-09-14; no dependency was
updated and no soak exception was used.

`.cargo/config.toml` sets `--cfg getrandom_backend="custom"` **only for
wasm32-wasip3**. The explicit `turnloop-wasi-random` linkage crate supplies the
[documented custom-backend symbol](https://github.com/rust-random/getrandom/blob/v0.3.4/README.md#custom-backend),
`__getrandom_v03_custom`, which both locked getrandom 0.3.4 and 0.4.3 declare.
The scalar `wasi:random/random@0.3.0.get-random-u64` import initializes every
requested byte, including unaligned tails, with no list allocation. The shim is
linked by the three protocol consumers and shared once in a final binary;
MongoDB remains in the full p3 workspace. It introduces no event-loop dependency
into the sans-IO crates. Other targets are unaffected.

Downstream p3 applications must set that target rustflag themselves (Cargo does
not inherit dependency config), link the shim, and avoid a second custom symbol.
Missing WASI random support is a link/host capability failure, never a predictable
fallback. Tests generate 754 bytes through both getrandom generations, reject
zero/constant long samples, generate two distinct BSON ObjectIds, and measure
200 additional real entropy fills at zero allocations.

## WASI timer measurements

Wasmtime **46.0.0** on macOS arm64, release, twenty 250 µs waits per process.
`crates/turnloop-contract/examples/wasi_timer_baseline.rs` deliberately uses no
turnloop types or calls: p2 subscribes/polls the monotonic-clock pollable; p3 uses
the same async-lowered monotonic-clock wait with a single raw waitable set (p3
removed pollables). Every wait checks it did not return early.

| Measurement | Median lateness |
| --- | ---: |
| Bare p2 | 919,416 ns |
| Bare p3 | 907,125 ns |
| turnloop p2, same runtime | 1,062,667 ns |
| turnloop p3, same runtime | 973,666 ns |

Raw bare p2 lateness, nanoseconds:
`[2389375, 911375, 924542, 920625, 2046875, 936792, 925125, 903625, 904000, 2030958, 906833, 920250, 919416, 909917, 2046125, 915625, 902208, 894500, 894583, 908292]`.

Raw bare p3 lateness, nanoseconds:
`[262291, 1293542, 2020166, 894375, 892167, 897875, 909250, 2049500, 906917, 907125, 918042, 906875, 906125, 2039958, 920167, 956666, 903917, 891667, 2027125, 895166]`.

Raw turnloop p2 lateness, nanoseconds:
`[2075125, 1068625, 1051083, 1047375, 1062625, 1073708, 1047333, 2286833, 1030083, 2289292, 1048167, 1076417, 2353792, 1025292, 2239583, 1039500, 1062667, 1069916, 1038333, 1040375]`.

Raw turnloop p3 lateness, nanoseconds:
`[910750, 906208, 919292, 2067416, 973666, 990000, 918333, 2066166, 925791, 918583, 930125, 921000, 2062833, 959958, 924458, 1121584, 2050917, 979083, 1042708, 1023542]`.

The bare programs reproduce the ~1 ms host lateness; the observed additional
turnloop p3 median is ~67 µs, not a millisecond floor. Per the integrator's decision,
DESIGN §7.4/§7.6 now describe host-dependent wake precision (Wasmtime ≈1 ms).
The WASI release gate uses median ≤2 ms, giving measured host scheduling headroom;
native's <500 µs gate is unchanged. Debug excludes precision sampling but still
runs timer semantics and **turns per expiry ≤2, zero-event waits ≤1**. CI prints
both baseline and driver raw samples to make host changes visible. Deadline
representation remains nanoseconds; no backend floors or spins were introduced.

Reproduce each baseline with the target's pin and runner, for example:

```sh
CARGO_TARGET_WASM32_WASIP3_RUNNER=scripts/ci/wasmtime-runner.sh cargo +nightly-2026-09-07 run --locked -p turnloop-contract --release --all-features --target wasm32-wasip3 --example wasi_timer_baseline
python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3
```

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

## Revision 2: stdio, external waits and executor

WASI p2 and p3 implement `open_stdio` with owned `wasi:cli` stream endpoints.
Reads, writes, writev, cancellation and shutdown use the same direction queues,
provided/pooled buffers and bounded waits as sockets. P2 drops subscriptions
before stream resources; p3 uses CLI-specific result futures and waits for stream
shutdown acknowledgement. Closing owned endpoints leaves the host standard
streams usable. Native AF_UNIX, handle passing, processes, signals and TTY
mode/window operations explicitly return `Unsupported`; the WASI terminal API
has detection but no portable mode/size query. Web stdio is also Unsupported.

`WaitCondition` and `external_wait` work on the single owning WASI/web agent.
A lazily initialized registry reserves 16,384 slots during condition construction,
retains storage at its used high-water mark, and publishes into each waiting
loop's reserved completion queue. No helper thread or per-wait allocation is
created. Initial inequality, same-value notify, value changes, exact deadlines,
cancel/stop, output backpressure and loop-drop cleanup have executable contracts.
The earliest wait deadline participates in `next_deadline()` and web host timer
scheduling. Expiry publication during a turn does not leave a spurious wake for
the next turn. WASI cannot accept OS-thread producers; a host must return control
to the guest agent to mutate/notify conditions.

With `web-worker`, `loop.worker_wait_condition(&condition, capacity)` returns
another bounded SharedArrayBuffer descriptor. Its `producerSource` class exposes
`store(valueBigInt)` and `notify()`, each returning false on full/contended/closed.
Accepted records apply in FIFO order when the owner drains the queue; admission
is asynchronous and is not an acknowledgement that the owner applied the value.
This is a host message bridge, not shared Rust linear memory. It uses the same
Atomics publication/parking protocol as Worker Poster, with an independent ring
per condition descriptor. Descriptor count is bounded by `max_operations`, and
storage/closures live until the owning driver drops. A descriptor can notify a
condition registered on several loops on the same agent. Cross-origin isolation
requirements are identical to Worker Poster. Drop closes the ring before releasing
the callback; delayed Worker messages cannot access released Rust state.

The `executor` feature runs on each of these targets. WASI contracts exercise
real TCP buffers, sleeps/timeouts and task cancellation without OS threads; the
release allocation binary also runs the native shared executor I/O/sleep gate.
Web tests run byte-verified WebSocket tasks, sleeps/timeouts and explicit join
cancellation using only host-scheduled `turn(Now)`. Their synchronous Rust turn
paths have a zero-allocation gate; JavaScript/runtime allocations remain outside
that measurement. A separate Worker test delivers 64 condition completions to two
loops, alternates changed/same values, and checks full-ring rejection and closure.
Both external-wait and ordinary timer tests retain all sixty no-spin expiries.

## Explicit contract exclusions

These are platform exclusions, not successful tests. WASI UDP, release precision
and release allocation subjects remain mandatory.

| Contract family | WASI p2/p3 | Web/Node |
| --- | --- | --- |
| TCP connect/listen/accept, UDP, writev, shutdown | Exercised; reuse-port Unsupported, nodelay remains a hint because bindings lack a setter | Native socket cases excluded: platform lacks raw sockets. Actual Unsupported results tested; fetch/WebSocket byte paths replace transport workloads |
| Blocking pool / native cross-thread wake and 8-peer posting | Excluded: these single-agent targets cannot spawn OS threads; Running-notify/post behavior exercised | Native threads/pool excluded and Unsupported tested; two actual JS Workers exercise SAB posting |
| Native detach/attach transfer | Excluded: WASI resource transfer Unsupported (accept attaches owned transport internally) | Excluded: browser transport transfer Unsupported, tested |
| Integration fd/event, POSIX signal EINTR, kqueue/epoll/IOCP specifics | Excluded: runtime-owned integration, no native fd | Excluded: HostCallback and zero OS waits |
| Native sub-ms timer ceiling | Release uses measured host bound ≤2 ms; debug semantics/no-spin remain required | Native ceiling excluded: browser timers are clamped. Host-clock tests require no early firing, bounded lateness, 60 actual expiries and one scheduled turn each |
| Real Postgres/MySQL/Redis/SMTP/MongoDB server integration | Outside backend contract scope; existing native protocol fixtures remain separate | Raw-socket protocol integration excluded by platform capability; sans-IO protocol crates still cross-check |

For browser reruns outside the sandbox, use the setup/PATH above and:

```sh
python3 scripts/ci/run-tests.py web --browser chrome
python3 scripts/ci/run-tests.py web --browser firefox
```

## Pinned browser CI and diagnostics

Linux x86_64 CI installs Chrome for Testing **153.0.8010.36** and exactly matching
chromedriver, Firefox **155.0.1** and geckodriver **0.37.1**. URLs and SHA-256 pins
are committed in `scripts/ci/browsers.json`. `install-browsers.py` verifies each
archive before extracting, checks executable versions on Linux and writes
absolute binary paths. Firefox's SHA256SUMS and geckodriver's GitHub release
asset digest were independently matched; Chrome/driver digests were computed
from the official Chrome for Testing HTTPS archives.

```sh
python3 scripts/ci/install-browsers.py               # Linux x86_64
python3 scripts/ci/install-browsers.py --verify-only # verify/extract without execution on other hosts
```

`BrowserDriver` starts and owns each pinned driver, polls its `/status` with a
15-second bound, supplies an explicit browser binary in capabilities, and points
wasm-bindgen-test at that driver. It does not treat normal driver stderr as a
startup failure. `wasm-pack --mode no-install` receives the exact driver path.
Driver stdout/stderr (Chrome verbose, Gecko trace) stay in `.tools/browser-logs/`
and are printed on any test/fixture/startup failure. Cleanup terminates the
owned process group, including browser children. Chrome and Firefox are each
attempted, and zero executed browser tests cause failure even if compilation or
Node tests passed. Node remains a separately executed suite with positive subject counts.

The integrator reproduced ChromeDriver SIGKILL on this Mac **outside the
sandbox**, with matching 153.0.8010.36 versions: this is a host restriction,
not a sandbox diagnosis. Real browser execution remains UNRUN here and required
on GitHub's Linux runner. Linux binary checksum/extraction verification and mock
WebDriver lifecycle tests do not count as browser test execution.
