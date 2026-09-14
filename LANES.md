# Implementation lanes (M1/M2)

Each lane works in its own clone. Only the integrator merges into the public
[PerryTS/turnloop repository](https://github.com/PerryTS/turnloop). Read other
clones when needed; never write to them. Git commits are made by the integrator
when the working sandbox exposes Git metadata read-only.

| lane | owns | first deliverable |
|---|---|---|
| core | Cargo workspace; `crates/turnloop` core (Loop, turn, tokens/op table, completions, timers, Notifier/Poster, ref/unref, close/cancel semantics, detach/attach, Integration); the internal `Backend` trait in `crates/turnloop/src/backend/mod.rs`; epoll + kqueue backends; `crates/turnloop-contract` contract tests; `crates/turnloop-bench` instruction micro-benchmarks | Backend trait tagged `trait-v0`, then Unix TCP echo + timers + multi-loop contract tests green on macOS |
| windows | `spikes/iocp/` (standalone), then `crates/turnloop/src/backend/iocp/` | Spike: IOCP turn, PostQueuedCompletionStatus wake, high-resolution timer (both approaches), AcceptEx/ConnectEx/WSARecv zero-byte read/WSASend, overlapped named pipes, Integration::Event helper thread; compio-driver evaluation |
| wasm | `spikes/wasi-p2/`, `spikes/wasi-p3/`, `spikes/web/` (standalone), then `crates/turnloop/src/backend/{wasi_p2,wasi_p3,web}.rs` | Spikes: wasi:io/poll loop with clocks + wasi:sockets echo under wasmtime; WASI 0.3 multi-wait mechanism; web HostCallback loop with setTimeout/fetch/WebSocket |

Rules for every lane:
- DESIGN.md is the specification. If the spec is wrong or ambiguous, write the problem and your proposed change in your lane's `LANE_REPORT.md`; don't silently diverge.
- Only touch files your lane owns. The Backend trait belongs to core; other lanes adapt to it and file requested changes in their report.
- Commit early and often on your lane branch, with plain commit messages and no attribution or co-author lines.
- No network publishing: no pushes, no remote repositories, no crates.io publish.
- Dependencies: only those allowed by DESIGN.md §13 (libc/rustix, windows-sys, wasi/wit-bindgen, wasm-bindgen/js-sys/web-sys, plus dev-dependencies for tests such as loom). No tokio anywhere. Any other dependency must be justified in LANE_REPORT.md.
- `unsafe` blocks carry a `// SAFETY:` comment; `#![deny(unsafe_op_in_unsafe_fn)]`.
- `cargo fmt` and `cargo clippy -- -D warnings` (for each target you can check) before each commit.
- Report honestly: failing or unrun tests are listed as failing or unrun, with the command.
