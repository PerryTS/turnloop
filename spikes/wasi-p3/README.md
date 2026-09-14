# WASI 0.3 experiment

## Working result

`src/lib.rs` exports `wasi:cli/run@0.3.0` using `wasip3 0.8.0+wasi-0.3.0`
and its reexport of wit-bindgen 0.61.1. Real 0.3 monotonic clock async imports,
TCP connect futures, an accept stream, byte streams and result futures run
concurrently. A server echoes 257 received bytes; the client verifies every byte.
Eight 100 µs timer futures complete while a 20 ms timer is pending. The two TCP
tasks and timer tasks are joined by a tiny dependency-free Rust combinator.
No tokio or private Rust executor crate is in the guest dependency graph.

```sh
cargo build --manifest-path spikes/wasi-p3/Cargo.toml --release --target wasm32-wasip2
.tools/wasmtime-v46.0.0-aarch64-macos/wasmtime run -S inherit-network=y -W timeout=45s spikes/wasi-p3/target/wasm32-wasip2/release/windlass_wasi_p3_spike.wasm
cargo clippy --manifest-path spikes/wasi-p3/Cargo.toml --all-targets --target wasm32-wasip2 -- -D warnings
cargo +stable check --manifest-path spikes/wasi-p3/Cargo.toml --target wasm32-wasip2 --locked
```

This deliberately uses Rust's **wasip2 packaging target**: std imports remain
0.2 and the experiment's sockets/clocks and command export are 0.3. It is not a
pure wasip3-target validation. The bindings document this interoperable route.
First passing run: 100 µs waits elapsed 1.295–2.559 ms; 8 short timers + 1 long;
257 bytes on server + 257 verified on client. Exact logs: `../verification.jsonl`.

## Toolchain and runtime evidence

- Pinned nightly-2026-08-20 recognizes `wasm32-wasip3` in `rustc --print target-list`.
- `rustup target add wasm32-wasip3` FAIL: no prebuilt artifacts on that nightly.
- Installed rust-src without changing the repo pin. An initial binary build with
  `cargo build --manifest-path spikes/wasi-p3/Cargo.toml --target wasm32-wasip3 -Z build-std=std,panic_abort`
  compiles std but FAILS linking: `cannot open crt1-command.o`, `unable to find library -lc`.
  A pure-target runnable artifact needs a matching WASI SDK/libc/sysroot (and a
  compatible component linker), or a toolchain shipping the complete target.
- The [Tier 2 proposal](https://github.com/rust-lang/compiler-team/issues/1001)
  being approved does not imply artifacts exist for this particular pinned nightly.
- `wasip3 0.9.0` was rejected by the seven-day soak (published four days ago).
  Pinned older 0.8.0 instead. Soak unchanged. wasip3 is the official WASI bindings
  package split, justified under the design's `wasi`/`wit-bindgen` dependency rule.
- Downloaded **only inside `.tools/`** from the official
  [Wasmtime 46.0.0 release](https://github.com/bytecodealliance/wasmtime/releases/tag/v46.0.0).
  Asset `wasmtime-v46.0.0-aarch64-macos.tar.xz`, SHA-256
  `ab4bdab6ea42a3245cda91cdc6e0430491c4b78ecd643406fc1764ccddbdcd25`, matched
  the GitHub release API digest. Version output: `wasmtime 46.0.0 (423be7a4e 2026-06-22)`.
  System Wasmtime 44 is untouched. 46 needs no opt-in p3/async flags for this test.
- WASI 0.3's June 11 release and Wasmtime 46 support are confirmed by the
  [Bytecode Alliance release announcement](https://bytecodealliance.org/articles/WASI-0.3).

## How many pending operations share a wait

The [Canonical ABI](https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md)
provides waitable sets, distinct from WASI 0.2 pollables:

1. Start an async-lowered call or a stream/future read/write. Its blocked operation
   yields a waitable handle; retain the operation and its stable memory.
2. Join each waitable to one persistent set. Associate the handle with an op-table
   generation and host token. A waitable belongs to one set at a time.
3. `waitable-set.poll` is the nonblocking path; `waitable-set.wait` suspends until
   an event or task cancellation. The event has a kind plus two payload words.
4. Decode the call/stream/future event, finish the operation, and enqueue its
   completion. A wait-set event need not be a user-visible final completion.
5. Race the set with a monotonic `wait-until` operation for the effective deadline.
   Remove/cancel a superseded timeout; never leave expired timer tasks accumulating.
6. Cancel underlying operations and account for cancellation/partial-transfer
   results before releasing pinned storage or returning terminal completions.

Source inspection of wit-bindgen 0.61.1 `rt/async_support.rs`, `waitable_set.rs`,
`waitable.rs`, and stream/future support confirms this implementation. The runtime
joins blocked operations into an exported task's private waitable set, recording
wakers. Async exports return a callback code `Wait(set)` to the host. The next
host event reenters the callback to poll Rust futures. Synchronous `block_on`
instead loops over `waitable-set.wait`/`poll` until the whole future completes.
`StreamReader::read` and writers expose cancellation APIs and reusable owned buffers;
`collect` in this experiment is convenience code and allocates.

## Mapping to windlass and the unresolved boundary

A strict synchronous `Backend::turn` needs a persistent set and a single-call
`wait/poll` interface that returns after one host event, even if it only advances
an intermediate subtask state. It must poll only driver state machines, never user
futures inside turn. A fresh `block_on` for every turn is insufficient: it owns its
own task/set, allocates and can make multiple waits. The public wit-bindgen runtime
does not expose its persistent set or one-step dispatch. Options for integrator:

- Add/obtain a supported runtime step API; keep a persistent TaskState and operation
  table, with callbacks only enqueueing driver completions.
- Generate dedicated lowerings around the ABI primitives and own the set directly.
  This is a nontrivial resource/cancellation/ABI-lifetime implementation, not just
  substituting a wait function.
- Let a WASI async export own waiting and expose `turn(Now)` to the host. That needs
  an explicit design/API change akin to HostCallback; it is not current D7 blocking turn.

This spike proves timer+TCP+multiplexing, **not** D7's bounded wait, D4's full
cancellation contract, or §10's zero-allocation gate. Those remain integration work.
OS-thread `std::thread` was not tested; component cooperative scheduling is not a
Send+Sync cross-thread Poster. Do not run arbitrary blocking pool jobs inline in a
bounded turn: unsupported or host-async is the correct default.

Additional checks: pure `wasm32-wasip3` **clippy/check** with `-Z build-std=std,panic_abort`
passes, because it does not link. Wasmtime 44 with `-S p3=y,inherit-network=y
-W component-model-async=y,timeout=45s` fails to link the final 0.3.0 monotonic
clock imports (`now` missing/wrong type). Its experimental p3 flag is insufficient
for this final-spec artifact; 46 is the working verified version here.

## Pure target working on a separate newer toolchain

After the pinned-nightly failure, installed `nightly-2026-09-07` **without changing
rust-toolchain.toml**. This newer toolchain supplies prebuilt wasm32-wasip3 artifacts:

```sh
rustup toolchain install nightly-2026-09-07 --profile minimal --component clippy,rustfmt --target wasm32-wasip3
cargo +nightly-2026-09-07 build --manifest-path spikes/wasi-p3/Cargo.toml --release --target wasm32-wasip3 --locked
cargo +nightly-2026-09-07 clippy --manifest-path spikes/wasi-p3/Cargo.toml --all-targets --target wasm32-wasip3 -- -D warnings
.tools/wasmtime-v46.0.0-aarch64-macos/wasmtime run -S inherit-network=y -W timeout=45s spikes/wasi-p3/target/wasm32-wasip3/release/windlass_wasi_p3_spike.wasm
```

All PASS. Pure-target timer/TCP subject counts are identical (257-byte echo, eight
short + one long timer). Short timers elapsed 0.893–5.516 ms in this run. Cargo emits
an `unused config key unstable.min-publish-age` warning on the newer toolchain;
these builds use `--locked` against the lockfile already resolved by the pinned
nightly with the seven-day policy. The repo config is unchanged. This successful
route supersedes the pure-target runtime blocker, not the bounded-step API blocker.
