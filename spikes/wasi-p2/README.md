# WASI 0.2 bounded driver spike

Standalone crate, no tokio. `Driver::turn` performs nonblocking state-machine work,
then **at most one** `wasi:io/poll.poll`, then more nonblocking work. Queued
completions and `Timeout::Now` skip poll. No nested blocking WASI stream calls.
The timeout is a monotonic-clock subscription alongside socket/stream/timer pollables.
Fixed capacity: 256 resource slots; one operation per socket; 257-byte payload buffer.
Subscriptions drop before streams, which drop before sockets. Cancelled/closed
resources survive until a subsequent turn after terminal completion delivery.

Run from the repository root:

```sh
cargo build --manifest-path spikes/wasi-p2/Cargo.toml --release --target wasm32-wasip2
wasmtime run -S inherit-network=y -W timeout=45s spikes/wasi-p2/target/wasm32-wasip2/release/turnloop-wasi-p2-spike.wasm
cargo clippy --manifest-path spikes/wasi-p2/Cargo.toml --all-targets --target wasm32-wasip2 -- -D warnings
cargo +stable check --manifest-path spikes/wasi-p2/Cargo.toml --target wasm32-wasip2 --locked
python3 spikes/wasi-p2/scripts/fuel.py
```

Wasmtime 44.0.0 needs `-S inherit-network=y`; no preopened listener is needed.
The listener binds loopback port zero; all 64 clients and 64 accepted sockets are
live before transmission. All 16,448 response bytes are verified, partial reads
and writes handled. The harness asserts 128 cancellations followed individually
by Closed and an additional listener close. Timer tests use 32 samples each at
100 µs, 500 µs, 1 ms and 5 ms; they reject early delivery and lateness over 100 ms
(the scheduler-noise failure limit, **not** a claimed precision guarantee).

## Cost method

`fuel.py` finds the minimum fuel T that completes 100 and 200 iterations, checking
success at T and fuel exhaustion at T−1. Three fresh-process interleaved rounds
include a black-box integer control. The slope `(T200-T100)/100` cancels startup;
subtract the control slope to estimate incremental guest work. cgu=1, release LTO.
These are Wasmtime **guest fuel units**, excluding host I/O/runtime instructions,
not CPU instructions or a cross-runtime performance promise. Raw results are in
`results/fuel.json`; exact verification output is in `../verification.jsonl`.

## Failed gate and prototype limits

```sh
wasmtime run spikes/wasi-p2/target/wasm32-wasip2/release/turnloop-wasi-p2-spike.wasm allocation-gate 100
```

FAIL: **200 guest allocations / 100 poll turns** after warm-up, expected zero.
The generated wasip2 bindings allocate an input handle list and the returned
ready-index list. `InputStream::read` also allocates its returned byte list.
Do not promote this implementation as satisfying DESIGN §10. An allocation-free
canonical ABI lowering with reusable scratch storage needs separate design and
validation; replacing only turnloop's collections cannot remove these allocations.
Idle turns and timer-cancel can be measured with the `idle`/`timer-cancel` modes.

This is an experiment, not the core API: linear scans, nongenerational indices,
one active operation per socket, no ref/unref, provided/pooled user buffers,
multishot, cross-thread posting, UDP, DNS or filesystem adapter. Queued completion
storage is reserved for the test's maximum batch, not general submission backpressure.
