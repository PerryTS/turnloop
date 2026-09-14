# Windows contract-test plan

The core trait and turnloop-contract crate are not available in this clone yet.
The rows below map DESIGN §11/§5a **contract subjects**, not invented test names.
All Windows runtime tests in this lane are **UNRUN**; cross-check and Clippy only
verify compilation. Run the standalone probes unchanged on Windows before merge.

## Common contracts that apply unchanged

| Contract | Windows assertion / existing probe | Remaining contract work |
| --- | --- | --- |
| Bounded turn | `port`, `draft`: elapsed bound, one wait maximum, zero waits for software-ready work | Core Now/After/Until/Forever plus queued pool/timer completions |
| Cross-thread notify | `port` actual packet; `draft` producer waits for observed PARKED then wakes | N loops/N producers, teardown races, loom handshake |
| Running notifier no syscall | `draft`: 10,000 calls and exact zero post counter, then one parked syscall | ETW syscall evidence; counter alone is not syscall tracing |
| Exactly once | `tcp`, `pipe`, `draft`: successful bytes, cancellation packet, no duplicate packet; stale OpId rejection | Cancel-vs-success/error races, handle exhaustion and randomized op churn |
| Close ordering | `draft` one-element output forces Cancelled then Closed on separate turns | Multiple in-flight ops, accept's secondary socket, close after partial writes |
| Buffer stability | Stable op slab, provided-buffer ownership through completion; test accesses result only after delivery | Guard-page/release-after-completion test, GC integration belongs to Perry |
| Liveness / ref-unref | Design applies to Windows | Core O(1) ref counters; draft's handle scan is not this contract |
| Timer insert/reset/cancel/repeat | `timer`: 100 real expirations each route, APC cancel and NT cancel/rearm | Core heap and repeat/terminal events; no per-timer thread |
| Error propagation | Preserve per-entry NTSTATUS; draft maps to Win32 code | WSAECONNRESET/WSAECONNABORTED, EOF and partial transfer fault injection |
| Zero steady-state allocations | `draft`: warm-up then 256 provided read/write cycles under TLS counting allocator | Accept/timers/pooled read gates after core operation-table integration |
| Owning-thread delivery | Driver is !Send; helper only forwards opaque entries | N loops on non-main threads, cross-post routing, shutdown under load |

## Windows-specific variants

| Subject | Variant / assertion |
| --- | --- |
| External host integration | Replace fd polling with auto-reset Event + MsgWaitForMultipleObjectsEx. `integration` forwards 1,024 unique packets, a timer, partial drains of seven and proves helper queue reached capacity before shutdown. Direct turns cease when helper takes over. |
| Local IPC | Named pipes replace AF_UNIX. Test pending ConnectNamedPipe and ERROR_PIPE_CONNECTED; byte mode, overlapped ReadFile/WriteFile, EOF, cancellation and one Closed event. `pipe` covers both connect races and payloads. Add multiple pipe instances and busy clients. |
| TCP | AcceptEx unbound accept socket and address buffer, ConnectEx explicit bind, both context updates, per-provider extension pointers. Zero-byte reads must not consume payload or allocate an idle payload buffer; recv may return WSAEWOULDBLOCK and must rearm. Test skip-success true and provider fallback/error handling separately. |
| Non-overlapped stdio | Dedicated opt-in reader thread with real synchronous pipe. `stdio` checks received bytes and EOF posts. Add explicit cancellation/shutdown while blocked and console-input modes. |
| Process | Three independent pipe pairs, parent ends overlapped, child ends synchronous. STARTUPINFOEX handle list, suspended creation, job assignment before ResumeThread, one-shot wait, unregister join. `process` checks all stdio, exit=23, exit-before-registration, and Job Object termination of a live child AND grandchild. |
| Signals | Process-global dispatcher fans out to loops. `console` isolates real C/BREAK delivery in CREATE_NEW_CONSOLE child and checks mappings. SIGTERM is unsupported as a console signal. CTRL_CLOSE-to-HUP is best-effort; process termination prevents ordinary deferred cleanup guarantees. Add fan-out/storm tests without affecting the test runner console. |
| Console/TTY | Add ReadConsoleInputW reader-thread input, resize -> WinCh, VT mode restore, cancellation and non-console handle errors. These are contract-plan work, not implemented by the current signal probe. |
| Detach/attach | IOCP association is permanent until close, including duplicated file objects. Do not run Unix reassociation test unchanged or claim DuplicateHandle fixes it. Resolve socket reconstruction/routing and named-pipe transfer policy, then test with outstanding cancellation. |
| Leak/churn | `handles` verifies exactly 128 posted/dequeued packets and port handle count returns to baseline every round. Add full socket/pipe/process/helper handle and thread baselines after OS/runtime warm-up, with serial test execution. |

## Precision bounds and measurement protocol

No Windows timing has been measured. A 100ns due-time unit is representational
precision, **not** a scheduler guarantee. Hardware, load, power state and VM scheduling
affect all upper bounds. Windows 10 1803+ is required for the high-resolution flag.

| Probe / environment | Expected bound / gate |
| --- | --- |
| Empty basic IOCP 20ms wait | Elapsed >=15ms and <1s (existing functional test); no high-resolution claim for GQCSEx's integer timeout alone |
| Direct high-res timer, 250us, 100 samples | No early expiry; p95 lateness <5ms, max <100ms (existing functional CI assertions) |
| Dedicated idle physical Windows host | Proposed acceptance target: p50 lateness <500us, p95 <1ms for 250us and 750us deadlines. Measure before adopting this as a release gate. |
| GUI helper timer | Existing test asserts actual timer packet forwarding, not latency. Measure separate p50/p95/max because queue/thread scheduling adds delay; proposed functional max <100ms under unloaded conditions. |
| Parked wake / process / pipe liveness | Completion must arrive within 2–3s watchdogs; these prove progress, not latency. GUI total drain watchdog 5s; console child watchdog 10s. |

Extend precision runs to 100us, 250us, 750us, 1ms and 10ms budgets; report requested
delay, sample count, min/p50/p95/max **lateness**, elapsed time, OS build and physical/VM
configuration. Include concurrent TCP, cancellation storms, power changes and GUI
load. Compare APC and NT paths in interleaved fresh processes. Require a recorded
expiry counter equal to sample count. Do not relax a failed gate; report it with
command/output and characterize the cause.

## Commands for a Windows host

```powershell
cargo test --manifest-path spikes/iocp/Cargo.toml --all-targets -- --nocapture --test-threads=1
cargo test --manifest-path spikes/iocp/Cargo.toml --test timer -- --nocapture --test-threads=1
cargo test --manifest-path spikes/iocp/Cargo.toml --test draft -- --nocapture --test-threads=1
cargo clippy --manifest-path spikes/iocp/Cargo.toml --all-targets -- -D warnings
cargo +stable check --manifest-path spikes/iocp/Cargo.toml --all-targets
```

`console_child` is a harness helper, not a binary to run against a shared console.
All actual Windows executions above are **UNRUN** here. Integrator should add these
standalone tests to its Windows CI arm and wire the adapted tests into
turnloop-contract when the trait is available. No Unix/WASI/web backend changes.
