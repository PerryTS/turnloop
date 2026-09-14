# Backend drafts awaiting trait-v0

No trait tag exists at the post-spike check. These standalone drafts deliberately
name their temporary trait `DraftBackend`; they do not change or claim to implement
core's trait. Files are under the fallback directory explicitly assigned to this lane.

- `wasi_p2.rs`: executable adapter for Timeout Now/After/Until/Forever and RuntimeOwned.
  Its timer/cancel/close example runs under Wasmtime. It inherits p2's allocation
  failure and prototype resource table. Core must replace that table with its
  generational OpIds, handles, completion/result kinds and buffer contracts.
- `web.rs`: fixed-capacity generic Rust completion inbox with explicit backpressure
  and a ScheduleTurn/Coalesced return value. The caller schedules JS after posting.
  Unsupported timeout and insufficient output capacity leave pending records intact.
  Its native pure-Rust test verifies 256 ordered completions after both rejected
  turns. The actual browser imports, generation guards and scheduling-epoch behavior
  are exercised by `../web/`, ready to be wired here using core's Completion type.
- `wasi_p3.rs`: typechecked boundary for a persistent WaitSet provider that steps
  exactly once and finishes one event. **No real provider exists here**: the private
  wit-bindgen wait-set boundary is the unsolved work documented in the p3 README.
  The p3 timer/TCP prototype works; this does not make this provider runnable.

A successful turn replaces output. The draft trait intentionally leaves liveness,
the deadline heap, stable user buffers and multishot/terminal ordering in core.
P2's buffer capacity requirement is its fixed maximum burst (512 records); a real
backend must follow core's saturation/deferred-delivery contract instead.

```sh
cargo fmt --manifest-path spikes/backend_draft/Cargo.toml
cargo clippy --manifest-path spikes/backend_draft/Cargo.toml --all-targets --target wasm32-wasip2 -- -D warnings
cargo clippy --manifest-path spikes/backend_draft/Cargo.toml --all-targets --target wasm32-unknown-unknown -- -D warnings
cargo test --manifest-path spikes/backend_draft/Cargo.toml
cargo build --manifest-path spikes/backend_draft/Cargo.toml --example wasi_p2 --release --target wasm32-wasip2
wasmtime run -W timeout=5s spikes/backend_draft/target/wasm32-wasip2/release/examples/wasi_p2.wasm
```

Integrator adaptation checklist: op/resource ID types; registration/ownership;
turn output saturation and wait accounting; nearest-deadline clock conversion;
ref counts; cancellation race winner; final resource release; same-agent WASI wake;
web scheduling callback setup; Unsupported vocabulary; required contract runner.
Do not call these production backends or mark core contract tests passed.
