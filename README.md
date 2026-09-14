# turnloop

A host-driven, completion-shaped event loop. Creating a loop creates no thread.
The host calls `turn(timeout, &mut completions)` and dispatches results itself.

The core lane implements kqueue on macOS and epoll on Linux, TCP/UDP, timers,
per-loop notification/posting, handle transfer, and a lazy shared blocking pool.
IOCP, WASI and browser adapters are developed in the other lanes. Their shared
contract is [backend/mod.rs](crates/turnloop/src/backend/mod.rs), revision 1;
`Driver<B>` supports an adapter without changing the core. The native `Loop` alias
currently selects the Unix adapter.

```rust
use std::time::Duration;
use turnloop::{Completions, Config, Loop, OpResult, Timeout, Token};

fn main() -> turnloop::Result<()> {
    let mut driver = Loop::new(Config::default())?;
    let timer = driver.timer(driver.now() + Duration::from_millis(1), None, Token(7))?;
    let mut completions = Completions::default();
    let mut fired = false;
    while driver.alive() {
        driver.turn(Timeout::Forever, &mut completions)?;
        for completion in completions.drain() {
            if matches!(completion.result, OpResult::Timer) {
                assert_eq!(completion.token, Token(7));
                fired = true;
            }
        }
    }
    assert!(fired);
    driver.close(timer, Token(8))?;
    driver.turn(Timeout::Now, &mut completions)?;
    Ok(())
}
```

Capacity and ownership are explicit:

- `Config` reserves handle/operation tables, buffers and posting slots. Exhaustion
  returns `ResourceLimit`/`WouldBlock`; delivery returns operation credits. A small
  `Completions` buffer defers excess results without losing work or allocating.
- A one-shot timer stops keeping the loop alive when its result is delivered, but its handle remains valid
  until `close`. `timer_reset` changes an active timer. Repeats coalesce missed
  intervals and schedule the next tick relative to the actual expiration turn.
- `IoBuf`/`IoBufMut` constructors are unsafe: retain stable memory until the terminal
  completion. Writes complete after all bytes, unless cancelled or errored.
  `WriteVectored` holds up to eight segments inline.
- `ReadBuf::Pooled` returns a `BufLease`. Drop/release it to return the buffer;
  retaining leases across turns is safe and applies backpressure when the pool is full.
- `close` delivers outstanding cancellations before `Closed`, then releases the
  native resource. `detach` initiates cancellation and returns `WouldBlock` until
  those completions have been delivered; turn and retry, then send the owning
  `Detached` to another loop and `attach` it there.
- `Poster` delivers only to its loop. Its bounded lock-free queue does not promise
  FIFO order. A rejected post returns the payload. A wake failure
  with `PostError.payload == None` means the post was accepted; do not resubmit it.
- `integration()` opts into external parking: notifications wake the external
  fd/event between turns. The host must also honor `next_deadline()`.
- The shared blocking pool starts on first submission. The first `PoolConfig`
  fixes its process-wide limits; later conflicting settings return `InvalidInput`.
  Native jobs may outlive a dropped loop; their results are then discarded.
  Wasm requires a host async adapter and reports `Unsupported` for native pool jobs.

Run `sh scripts/verify-core.sh` for native tests, loom, both timer structures,
Linux cross-Clippy and stable checking. The allocation and fd-lifetime gates run
in separate test binaries. Run `python3 scripts/benchmark-core.py --rounds 5` for
interleaved fresh-process instruction measurements at codegen-units=1. The
`timer-btree` feature selects the BTreeMap benchmark queue and its tests. The
production Driver always uses the indexed four-ary heap: measured BTreeMap churn
allocated nodes after warm-up and failed the allocation gate. Linux `epoll-timerfd`
forces the old kernel fallback for testing. CI is configured to run both Linux
wait paths on real Linux runners; hosted execution is still pending.

Status and exact verification limits: [LANE_REPORT.md](LANE_REPORT.md).
Specification: [DESIGN.md](DESIGN.md). Ownership: [LANES.md](LANES.md).
