# core5 lane report

Status: implementation complete; final broad verification in progress. Base `fcea55e`; no commits
are made by this agent (`.git` is read-only).

## Findings

- The reported between-round close/rebind hypothesis does **not** match the test:
  both loops and all 16 IPv6 sockets are created before `0..=ROUNDS`, remain bound
  through all 21 rounds, and drop only after every receive/send is verified.
- A separate production binding bug is present: `Unix::open` unconditionally
  enables `SO_REUSEADDR` for UDP even when `UdpOpts::reuse_port` is false. Linux's
  `udp_lib_get_port` (used for IPv4 and IPv6) excludes an existing socket from its
  ephemeral-port conflict bitmap when both sockets have `sk_reuse`. Thus two live
  default sockets can share an endpoint and the kernel can deliver a self-send
  to the other socket. This explains the failure shape; the original CI failure
  lacks addresses, so its exact cause remains unconfirmed by runtime evidence.
  Source: [Linux v6.8 UDP port allocation](https://github.com/torvalds/linux/blob/v6.8/net/ipv4/udp.c#L141-L291).
- macOS rejects an explicit duplicate UDP bind with only `SO_REUSEADDR` for both
  IPv4 and IPv6 (local Python socket probe). This explains why that particular
  Linux binding defect need not reproduce on kqueue.
- Epoll registers full generational handle keys, deregisters before releasing
  owned descriptors, and Unix removes cached scheduling entries on release and
  detach. Polling validates full keys; native I/O executes synchronously against
  the matched resource. Timerfd uses a private key and the same dispatch path.
  No fd/generation misattribution found. Deterministic reuse regressions pass on macOS.
- Pooled receive length comes directly from `recvfrom`, independently of the
  lease. Each lease owns its vector and originating pool. Review found no route
  for a lease mix-up to change a one-byte syscall result to zero.

## Implemented

- Default Unix UDP no longer enables `SO_REUSEADDR`. TCP listeners and explicit
  UDP `reuse_port` retain their options. No polling, completion, generation or
  allocation path changes were needed.
- Deterministic IPv4/IPv6 tests inspect the actual default socket option, reject
  duplicate live binds, rebind after release, and preserve explicit sharing.
  The option regression FAILED before the fix on macOS (`SO_REUSEADDR = 4`),
  then passed unchanged after the fix.
- A separate regression arms/cancels a receive with a zero-byte packet queued
  and actual old-generation poll events collected, detaches, uses `dup2` to
  atomically close/reuse the exact owned descriptor, binds the exact old port,
  and reattaches under a new handle generation. It asserts stale identities do
  not affect the new operation, the empty socket really waits, only the new
  packet arrives with its correct source/op/bytes, and no duplicate follows.
  Keeping the destination fd owned throughout avoids overwriting another test's
  descriptor. Both IPv4 and IPv6 cases must execute.
- The original allocation test still keeps all 16 sockets bound for 21 rounds.
  It now asserts endpoint uniqueness across both loops before sending, prints
  fixture handles/endpoints outside the allocation window, checks cancellation
  and receive/send op IDs, handles, tokens, terminal status and buffer mode,
  and checks sender **before** length with round/loop/identity context. No stray
  result is ignored/retried. Counts, capacity-one backpressure, every length
  (including zero), cancellation/drop work and zero-allocation limits remain.
- Other UDP contracts (steady allocation, shared echo, executor and WASI empty
  datagram) now assert distinct endpoints; completion-based cases check sender
  before length. These sockets likewise remain alive throughout their traffic.
- TCP audit: echo/pair, accept churn, transfer/handoff, no-spin and lifetime tests
  keep their listeners/streams alive until the associated work is done. Shared
  listeners intentionally use `reuse_port`; TCP connections are separate streams,
  not UDP receive queues. The independent `refused_connect_once` fixture closes
  a temporary listener before connecting: another process can claim that port in
  the gap. It is unchanged and documented below as a separate fixture issue.

## Verification

PASS: required design/contributing/integration and core/core3/CI lane reports read
in full; no applicable AGENTS.md found; working tree initially clean; installed
target inventory includes both Linux triples. PASS: local IPv4/IPv6 socket probe
above. Verification commands and raw logs will be recorded under `.tools/core5/`.
The focused test passed **200/200 fresh processes per macOS mode** (default,
executor, all-features), **600/600 total**, with exactly one passed, zero failed,
zero ignored test required in every subprocess. This executes 192,000 measured
receives, 192,000 cancellations, plus warm-up/drop and nested steady UDP work.
The new backend regressions initially passed 3/3; final native/mode checks follow.
Linux runtime is **UNRUN (no Linux host)**; Windows runtime likewise UNRUN.
No-tokio PASS on all eight target graphs and union, default/all features. Soak
PASS: 251 locked registry versions, only the inherited exact rustls exception.
Initial native strict Clippy FAIL: the new getsockopt safety comment needed to
be inside the multiline assertion; comment moved, no lint weakened.
Linux x86_64 full-workspace Clippy PASS in all six modes. Initial arm64 workspace
Clippy FAIL: missing `aarch64-linux-gnu-gcc` in ring's build; final retry uses the
installed Zig C cross compiler with its cache under `.tools/core5/`.

## Deviations / proposed DESIGN changes

No dependency, soak setting, allocation threshold or wait path changed.
Proposed clarification: default UDP binds are exclusive; address/port sharing is
an explicit `reuse_port` opt-in. DESIGN.md remains unchanged.

## Open questions / next steps

- Linux runtime must confirm the default endpoint exclusivity and exact fd/port
  reuse regression. The original failure did not record endpoints or identities;
  live endpoint aliasing is supported by kernel source, not reproduced locally.
- `refused_connect_once` needs a portable bound-but-not-listening TCP reservation
  fixture (including WASI). Its existing close-before-connect race can cause a
  spurious success if an unrelated listener claims the port. This is separate
  from UDP and left for a focused fixture change, with no retries or skips added.
- Finish broad verification and attach Linux x86_64 repetition commands below;
  arm64 runtime belongs to the required CI matrix. Integrator commits the tree.
