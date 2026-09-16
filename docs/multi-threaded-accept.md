# Multi-threaded accept

DESIGN [§5a.6](../DESIGN.md#5a-multithreading). One server on more than one core,
without a work-stealing loop.

A work-stealing loop is the wrong answer here and is deliberately not offered.
turnloop gives each JS agent its own loop on its own thread because JavaScript
values are thread-local: a completion has to be delivered on the thread whose
heap owns the promise it settles. A loop shared across cores would have to route
every completion back to its owning agent, which rebuilds per-agent queues and
wakeups with worse locality — the second scheduler this crate exists to remove.

So the parallelism is in the *accept*, not in the loop. Two routes.

| | Kernel-distributed | Handoff |
|---|---|---|
| Shape | N loops, N threads, N listeners, one port | 1 accepting loop, N sibling loops, `detach`/`attach` |
| Who decides | the kernel, by 4-tuple hash | the host, by whatever policy it writes |
| Linux, Android | `ReusePort::Distribute` (`SO_REUSEPORT`) | yes |
| FreeBSD | `ReusePort::Distribute` (`SO_REUSEPORT_LB`) | yes |
| macOS, other Apple, NetBSD, OpenBSD, DragonFly | **refused** (`Unsupported`) | yes — **the** route |
| Windows | **refused** (`Unsupported`) | yes — **the only** route |
| WASI 0.2/0.3, web | **refused** (`Unsupported`) | not available |

## `ReusePort`: which behaviour you are asking for

`SO_REUSEPORT` is spelled the same on Linux and on the BSDs and does not mean the
same thing, so `reuse_port` is not a `bool`.

```rust
pub enum ReusePort { No, Share, Distribute }
```

* **`Share`** — permit the duplicate bind, promise nothing about delivery. This
  is the traditional BSD use: multicast and broadcast receivers, and handing a
  port to a replacement process during a zero-downtime restart, where
  last-binder-wins is the effect you want.
* **`Distribute`** — permit the duplicate bind *and* spread incoming connections
  across the listeners. Refused where the platform cannot do it.

There is deliberately **no third outcome**. A platform either distributes or the
listener is refused when it is created. The failure this prevents is not
hypothetical, and it is silent: two loops sharing one port under plain
`SO_REUSEPORT` on macOS 15 split 32 connections **`[0, 32]`** — the first
listener is not merely under-served, it never accepts anything at all, for the
life of the process, with no error anywhere. A host that developed the Linux path
and shipped it would have a server that looks fine and uses one core.

`Share` and `Distribute` are the same `setsockopt` on Linux, and that is fine: a
variant states a floor, not a ceiling. On FreeBSD they are genuinely different
options — plain `SO_REUSEPORT` keeps its original BSD meaning there, and
`SO_REUSEPORT_LB` (12.0+) is the one that distributes.

### `Distribute` distributes by hash, not by load

The kernel selects a listener by hashing the connection's address 4-tuple.
Nothing asks how busy a loop is. A loop whose agent is inside a long turn keeps
being handed its share, and those connections wait in its queue while another
loop is idle.

Measured (Linux 6.8, contract test `multi_threaded_accept_by_reuse_port`), 64
connections over 4 loops:

```
reuse-port: per-loop [7, 12, 21, 24]
```

A 3.4x spread. At larger connection counts the hash evens out — 4000 connections
over 4 loops gave `[970, 1016, 1019, 995]` — but the *work* behind each
connection is still not what was balanced. If a host needs load-sensitive
placement, that is the handoff route, where the policy is the host's to write.

## The handoff route

`Loop::detach(h) -> Detached` (which is `Send`) and `Loop::attach(d, tok)`.
`detach` cancels in-flight operations with their usual exactly-once completions
and refuses with `WouldBlock` until the transport is quiescent, so the sibling
loop adopts something with no operation, buffer, registration or completion
outstanding.

**This works today on every native backend**, including Windows, and is covered
by contract tests that accept on one loop and then drive reads and writes on a
sibling loop on another thread. It is not a planned capability.

On **Windows it is the only route**, for a reason that is structural rather than
an omission: a handle's IOCP association is permanent. Windows will not
dissociate one, rejects a second `CreateIoCompletionPort` with
`ERROR_INVALID_PARAMETER`, and neither `WSADuplicateSocketW` nor
`DuplicateHandle` escapes it — both produce another descriptor for the *same*
socket object, which is where the association lives. A second loop therefore
cannot share a listener, and `SO_REUSEADDR` is not a substitute: on Windows it
permits *hijacking* an address rather than sharing it, which is why it is not
mapped onto either `ReusePort` request.

The handoff's cost, stated plainly: one extra `detach`/`attach` pair and one
cross-thread wake per connection, and an accepting loop that is a single point
of serialization for the accept itself. The kernel route has neither. That is
what macOS and Windows pay for not having `SO_REUSEPORT_LB`.

## What the contract tests assert

| test | asserts |
|---|---|
| `reuse_port_share_binds_twice` | the duplicate bind works and every connection is accepted by *someone*; deliberately asserts nothing about which listener, because `Share` promises nothing about it |
| `reuse_port_distribution` | no third outcome: the platform distributes, or the listener is refused with `Unsupported` and leaves nothing behind |
| `reuse_port_is_explicitly_unsupported` (Windows), `reuse_port_has_no_wasi_interface` (WASI) | both requests refused where there is no mechanism |
| `accept_handoff_distribution` | accept on one loop, drive I/O on a sibling loop, round-robin over 4 workers |
| `multi_threaded_accept_by_handoff` | 64 connections, 4 loops on 4 threads, **every connection served exactly once** — each client sends a distinct id and requires that exact id back — and every loop `!alive()` at shutdown |
| `multi_threaded_accept_by_reuse_port` | the same, by the kernel route, where it is available |

The two `multi_threaded_accept_*` tests run on loops whose handle ceiling
(`max_handles: 8`) is far below the 64 connections they serve. That is the
orphan gate: the workload fits comfortably, but a handle leaked per connection —
at accept, at `attach`, at `detach` or at `close` — exhausts the ceiling long
before the run ends and turns an invisible orphan into a `ResourceLimit` failure.

Both use **single-shot** accept, re-armed per connection. turnloop[#77] is open —
a multishot accept can outrun the handle ceiling within one turn — and a test
that ran multishot at a low ceiling would be exercising that open issue rather
than the accept route.

[#77]: https://github.com/PerryTS/turnloop/issues/77

## Scaling harness

`turnloop-bench --accept-scaling` drives N loops on N threads serving one port by
either route: accept, one request, one response, close. Connection-oriented on
purpose, because that is the shape where the accept path is the subject rather
than a rounding error.

```
cargo run --release -p turnloop-bench --locked -- --accept-scaling \
    --route handoff|reuse-port \
    --loops N --connections C --clients K --payload B [--portable]
```

`--portable` reports elapsed nanoseconds; without it the harness uses the
platform instruction counter (`perf` on Linux, `ri_instructions` on macOS,
`QueryProcessCycleTime` on Windows). Output is one JSON line with
`connections_per_second` and, beside it, `per_loop` — the service count for each
loop, so the distribution is visible next to the total instead of inferred from
it.

The harness refuses to print anything unless its subject demonstrably ran: the
per-loop counts must sum to the budget, every loop must have served at least one
connection, and `--connections` must be at least 100 per loop so that a loop
cannot be given nothing by chance. A flat scaling curve produced by three of four
loops sitting idle is the failure mode this exists to make impossible.

### Before believing any curve: saturate the clients first

The client side is synchronous — `K` threads each looping connect, write, read,
close — so the achievable rate is bounded by `K / RTT` no matter how many server
loops are running. **A server-scaling curve measured against a saturated client
is a picture of the client.**

So the first run is not a scaling run. Hold `--loops` at its largest value and
raise `--clients` until the number stops moving:

```
for k in 8 16 32 64 128; do
  cargo run --release -p turnloop-bench --locked -- --accept-scaling \
      --route handoff --loops 8 --connections 40000 --clients $k --portable
done
```

Take the smallest `K` at the plateau, double it, and use that for every point in
the scaling sweep. If the plateau is below the single-loop rate, the client is
the bottleneck everywhere and no scaling conclusion is available from this host.

### The sweep

```
for route in handoff reuse-port; do
  for n in 1 2 4 8 16; do
    cargo run --release -p turnloop-bench --locked -- --accept-scaling \
        --route "$route" --loops $n --connections 100000 --clients "$K" --portable
  done
done
```

`--route reuse-port` exits 2 with one line on a platform that cannot distribute,
so a macOS or Windows sweep is the handoff row only.

Run it on a quiet host. Interleave the arms rather than running all of one then
all of the other, so a drift in machine state does not land entirely on one
route.

### What the results would mean

Let `T(n)` be `connections_per_second` at `n` loops.

* **The design works** if `T(n)/T(1)` grows close to linearly to the core count
  and then flattens — for the kernel route with `per_loop` counts within roughly
  ±10% of each other, and for the handoff route with them near-exactly equal
  (round-robin is exact by construction, so anything else is a bug, not
  imbalance). Expect the handoff route to start lower than the kernel route at
  `n = 1` and to fall behind it as `n` grows: it pays a `detach`/`attach` pair
  and a cross-thread wake per connection, and its accepting loop serializes the
  accept. That gap is the price of macOS and Windows, and quantifying it is the
  point of running both arms.
* **The design does not work** if `T(n)` is flat from `n = 1` — one server cannot
  use more than one core by this route — or if it *falls* as `n` grows.
  Before concluding either, rule out the client (above) and check `per_loop`: a
  flat curve with lopsided `per_loop` is a distribution problem, not a scaling
  ceiling.
* **The handoff route's acceptor is the ceiling** if `T(n)` for handoff plateaus
  at a value the kernel route passes, while `per_loop` stays even. The accepting
  loop is then saturated, and the next move is more than one accepting loop
  (several acceptors each with their own listener under `Share`, each feeding a
  subset of workers) rather than more workers.
* **Nothing at all** if a run fails its liveness assertions. The harness panics
  rather than printing in that case, on purpose.

No numbers are recorded here. The harness has been run only for correctness, on
loaded machines, where a timing figure would be worse than none.
