# WASM backend evaluation — 2026-09-14

The experiments establish working WASI 0.2 timer/TCP/close behavior and WASI 0.3
async timer/TCP progress, plus the browser-host design running as wasm in Node.
They do **not** establish production conformance for all three backends. The
allocation gate fails on p2, the p3 runtime lacks a public bounded step interface,
and Chrome exits before browser assertions. Keep those gates open.

## Measured results

| Experiment | Result |
|---|---|
| p2 / Wasmtime 44, TCP | PASS: 64 simultaneous clients plus 64 accepted peers, 16,448 echoed bytes verified; 256 read/write completions |
| p2 cancel/close/error | PASS: 128 pending reads cancelled, each before Closed; listener Closed; an accepted failing operation completes once; delayed subscription/stream/socket destruction does not trap |
| p2 timers | PASS: 32 samples each at 100 µs, 500 µs, 1 ms, 5 ms; observed lateness ~0.15–2.61 ms across runs, never early |
| p3 / Wasmtime 46 | PASS: genuine final-0.3 timer futures + TCP accept/byte streams and result futures; 257 bytes echoed and verified concurrently with timers |
| p3 timers | Eight 100 µs requests: 1.295–2.559 ms using p2 packaging; 0.893–5.516 ms using pure p3 on newer nightly; one 20 ms timer pending concurrently |
| Node wasm host contract | PASS: ten groups of assertions, 257 fetch bytes, one actual fetch abort, 257 WebSocket bytes, lossless u64 tokens, stale callbacks, close ordering, capacity, scheduling, two-loop routing |
| Node timers after deadline recheck | Sixteen 1 ms requests: callback elapsed 1.283–2.136 ms. Earlier raw setTimeout run included a 0.044 ms early callback; deadline checking now prevents this |
| Node worker Poster | PASS: two concurrent producers, 2,000 unique verified completions; notify count > 0 when parked, zero while running; full-ring backpressure |
| Chrome contract and isolated Worker | UNRUN assertions: startup commands FAIL before test execution. See web README and verification ledger |
| Strict zero-allocation gate | p2 poll FAIL: 200 allocations / 100 waits. Idle and timer-cancel PASS: 0 / 10,000. p3/full JS path not zero-allocation implementations |

Fuel uses the minimum successful fuel threshold T, verifies failure at T−1,
and takes the 100→200 iteration slope. Three interleaved fresh-process rounds,
release cgu=1/LTO, constant-capacity loop, integer control. All three rounds agreed:

| Workload | Fuel units/iteration [min,max] | Minus control |
|---|---:|---:|
| Control | [198.31,198.31] | — |
| Empty Now turn | [14497.31,14497.31] | 14299 |
| Poll with immediately due budget | [26934.31,26934.31] | 26736 |
| Timer submit/cancel/Now | [14957.31,14957.31] | 14759 |

These are guest instruction proxies, excluding host/runtime I/O and scheduler
cost. Fixed 256-slot linear scans dominate; this is a spike baseline, not a
production performance budget. Source, method and raw thresholds:
[wasi-p2 README](wasi-p2/README.md), [fuel.json](wasi-p2/results/fuel.json).

## Capability matrix correcting DESIGN §7.6

“Available” below means the platform interface exists, not that this lane
implemented/tested it. Permissions and runtime support remain necessary.

| Capability | WASI 0.2 | WASI 0.3 | Browser host |
|---|---|---|---|
| Wait | pollable list; one poll per turn demonstrated | ABI waitable sets; multiplexing demonstrated, bounded synchronous step pending | Now-only drain; coalesced scheduling callback |
| Wake/post | same-agent flag; no portable OS-thread wake for blocked poll | task/waitable event scheduling; no demonstrated Send+Sync OS-thread Poster | runnable host posts callbacks; postMessage across workers; SAB ring with isolation |
| Timers | ns timestamp representation; measured ms-scale runtime wake | same distinction; wait-until/wait-for futures | browser clamps/throttles setTimeout; explicitly check deadline to avoid early completion |
| TCP/UDP | wasi:sockets; TCP tested, UDP UNRUN | wasi:sockets; TCP tested, UDP UNRUN | unsupported raw sockets/listeners |
| HTTP outbound | wasi:http or protocol layer; UNRUN | wasi:http async or protocol layer; UNRUN | fetch tested in Node; browser assertions UNRUN; browser CORS applies |
| WebSocket | protocol layer on TCP/TLS; UNRUN | protocol layer; UNRUN | host WebSocket; one exchange tested in Node |
| Local IPC / handle transfer | no standardized Unix socket/named pipe or cross-process fd passing | same limitation | postMessage transfers only supported JS transferables; WebSockets/TCP handles are not portable transferables |
| Stdio | wasi:cli input/output streams, readiness through wasi:io; UNRUN | native byte streams/result futures; UNRUN | no process stdio pipe API |
| TTY | terminal-presence detection, **not size** in inspected interfaces | terminal-presence detection, **not size** | no TTY API |
| Processes/signals | unsupported | unsupported | unsupported |
| Files | capability-scoped wasi:filesystem; some calls synchronous; UNRUN | async metadata operations, stream/future data paths; UNRUN | host-mapped OPFS only; UNRUN |
| DNS | wasi:sockets/ip-name-lookup; host permission; UNRUN | lookup-ip-addresses async stream; UNRUN | implicit resolution through fetch/WebSocket, no general DNS query API |
| Pool / arbitrary host jobs | no OS thread pool; inline jobs violate bounded turn | no demonstrated OS thread pool; host async operations instead | no main-thread blocking pool; explicit workers for supported serialized jobs |
| Integration | RuntimeOwned | RuntimeOwned, with unresolved synchronous step boundary | HostCallback |

The `wasi:cli/terminal-output` WIT in wasip2 1.0.4 and wasip3 0.8.0 only declares
an empty terminal resource; its comments reserve size/mode extensions for the
future. Replace the design's **“size only”** entries with **“detection only”**.
The inspected package WIT is the source of truth here, rather than future plans.

Pure `wasm32-wasip3` build/run also PASS on separate `nightly-2026-09-07`, which
ships its standard library. Pinned nightly recognizes the target but lacks its
prebuilt std/sysroot. Both mixed p2 packaging and pure p3 run with Wasmtime 46;
44 fails to link final 0.3 clock imports. The repo toolchain pin remains unchanged.

## Threads and host boundaries

`wasm32-wasip1-threads` is a separate shared-memory/host-thread target. Its installed
presence does not confer threads on 0.2/0.3 components. Thread spawning, shared
blocking pools, multi-loop cross-thread posting and detach/attach are UNRUN for
WASI; do not infer them from component async concurrency. The 0.3 runtime's
cooperative tasks progress on an agent, not automatically on separate OS threads.

Browser workers can each own independent wasm instances. The tested Poster shares
a separate numeric SAB ring, not a Rust heap or wasm linear memory. True shared
wasm memory needs atomics-enabled builds, compatible allocators and Rust-side
synchronization. COOP/COEP and a trustworthy origin gate SAB availability. A blocked
worker cannot run its own fetch/timer/WebSocket callbacks: another runnable agent
must post into the ring, or the worker must use asynchronous scheduling.

Perry keeps token→JS-root mapping, promises, errors and phase ordering (§9).
Backend callbacks must only enqueue data and request scheduling, including WASI
async callbacks. Calling arbitrary host jobs or user futures inside a backend turn
would break D1/D7. Unsupported operations must fail explicitly, never silently run
blocking work inline. Host-managed browser bytes are copied/leased at the Perry
boundary; browser fetch buffers are not automatically stable GC-provided IoBufs.

## Proposed mapping and integration risks

- **p2:** core owns generational handles/ops, timer heap, ref counts, completion
  buffers and lifetimes. Backend resource records own subscription→stream→socket
  drop order. Translate read/write/accept readiness into completed op IDs. Include
  one nearest-deadline clock subscription in the same poll list; skip poll on
  queued work/Now. Immediate readiness may advance an operation without completing
  it. Cancellation removes interests; core orders terminal records before release.
  Generated list lowerings currently defeat zero allocations. Need audited reusable
  ABI storage (both poll list directions and stream read buffers).
- **p3:** use one persistent waitable set and operation table, with a timeout
  waitable. A single wait/poll returns one ABI event; dispatch internally and return
  even if it does not yield a completion. Stock wit-bindgen TaskState and set are
  private; block_on builds/drives a whole task and can wait repeatedly. Obtain a
  supported stepping API or own dedicated ABI lowerings; otherwise explicitly
  change D7 to async host scheduling. Stream I/O needs cancellation outcome and
  memory-lifetime auditing before D3/D4 can be claimed.
- **web:** callbacks post generational op IDs; core resolves tokens and drains
  completions. Coalesce schedule requests per loop, invalidate a pending scheduling
  task when the host already turned, and reject blocking timeout modes. Schedule
  imported JS callbacks outside the Rust turn. A fixed completion buffer can be
  allocation-free in Rust, while the host APIs/JS FFI are not. State exactly where
  the allocation gate is measured. Browser close cannot prove network teardown
  finished, only that turnloop released ownership and rejects late callbacks.

No `trait-v0` tag was present at initial and post-spike checks. No other lane's
implementation was read. Drafts under `backend_draft/` define an explicit temporary
turn interface based on §6; **it is not core's Backend trait**. Adapt op IDs,
resource registration, capacity/backpressure, wake sources and error vocabulary
when core publishes its contract. Git operations cannot merge or commit here
because this managed sandbox mounts `.git` read-only.

Sources and runnable instructions: [p2](wasi-p2/README.md), [p3](wasi-p3/README.md),
[web](web/README.md). Every executed verification command and its full output,
including failed intermediate attempts, is retained in [verification.jsonl](verification.jsonl).
