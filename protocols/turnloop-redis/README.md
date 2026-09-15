# turnloop-redis

Runtime-free RESP2/3 connection and Cluster/Sentinel routing primitives.

The host calls `connect(now)`, consumes `Connect`, connects the socket and calls
`transport_connected()`. On `UpgradeTls` it completes TLS externally, then calls
`tls_established()`. It writes `output()` and acknowledges only bytes actually
written with `consume_output(n)`. Supply received bytes to `receive()`, drain
`poll_event()`, and schedule `next_timeout()` with the host's own timer. Pass the
current time to `handle_timeout(now)`; the core never obtains time itself.

`command(token, &[b"GET", b"key"], deadline)` accepts arbitrary binary arguments.
Use a distinct token per outstanding command. Submit multiple commands before
flushing to pipeline them. MULTI/WATCH/EXEC/DISCARD are ordinary wire commands;
QUEUED responses and nested EXEC results retain server order. Return `Err` from
submission means no command was accepted; accepted tokens receive one terminal
Reply, including timeout/close. A timed-out sent command retains a reply-order
tombstone, so its later response cannot complete another command. To break a
stalled blocking connection after timeout, the host closes the socket and calls
`transport_lost()`; Redis's own blocking timeout is still a command argument.

Reconnect emits `Retry { attempt }`; evaluate the desired ioredis retryStrategy
(including jitter, if configured) outside the core and call `retry(now, delay)`.
None stops retries and completes queued commands with errors. Reconnection
repeats authentication/database/name setup and subscription restoration before
flushing offline commands. Automatic unfulfilled-command replay may execute a
write twice if its first reply was lost. Set `auto_resend_unfulfilled=false` when
that policy is inappropriate. Connection failures during AUTH are terminal;
transport failures use retry policy. The host reports transport/DNS/TLS errors.

## Getting started on turnloop

Enable `turnloop-redis = { version = "0.1.0-alpha.2", features = ["turnloop"] }`
and use the `asynchronous` module: `Client::command/pipeline, Subscriber::next, ClusterClient and sentinel`. The default feature set remains sans-I/O.
The adapter reuses `turnloop-io`; the host owns `LocalExecutor` and calls `turn`.
Spawn local futures through its handle and keep their `JoinHandle`s until completion.

```sh
cargo run -p turnloop-redis --features turnloop --example turnloop
```

[The complete example](examples/turnloop.rs) connects to `127.0.0.1:6379` by default;
`TURNLOOP_DB_ADDR` overrides that development endpoint. All operations take an
absolute `turnloop_io::Instant` deadline, shared across authentication, I/O and retries.
Callbacks borrow row/reply storage; copy values only when retaining them.
Dropping a pending operation closes its transport before a pool can reuse it.

For TLS, set the protocol's TLS mode and provide
`turnloop_tls::asynchronous::ClientTls` with the trust configuration, verified
server name and current Unix seconds. Certificates are verified; a requested TLS
upgrade without a configuration fails. Native TCP and WASI 0.2 sockets share the
same driver. Browser raw TCP is unavailable; Windows accepts a host-provided
`Backend` until the repository's production IOCP provider is integrated.

Pipelines encode the entire batch before awaiting ordered replies. Per-command
Redis errors are returned to the pipeline callback while later replies continue.
`subscribe` lends an exclusive subscriber with async `next(deadline)`; dropping
it closes the subscribed session. Reconnects use real turnloop timers, restore
subscriptions, and follow the core's `auto_resend_unfulfilled` policy. An ambiguous
write can execute twice if that policy permits replay. `ClusterClient` follows
MOVED/ASK and reuses per-node sessions; `sentinel` verifies the discovered primary's
ROLE, with separate Sentinel/data-node authentication settings.


## Reply conversion boundary

| Wire/core result | ioredis/Perry conversion in the host |
|---|---|
| Simple bytes | UTF-8 string (SET returns `"OK"`) |
| Bulk bytes | UTF-8 replacement decoding (`text`) or exact Buffer (`bytes`) |
| Integer | JS number; precision above 2^53 requires a host string/bigint policy |
| Null (RESP2 nil, RESP3 null) | JS null |
| Array | Recursive JS array, preserving order and nested ReplyErrors |
| Map / Set | Explicit RESP3 map/set values; host selects JS Map/Set or command transformer |
| HGETALL | Object from alternating RESP2 pairs or RESP3 map entries |
| EXISTS / EXPIRE | ioredis number; current Perry bindings convert to boolean |
| EXEC | Null for WATCH conflict; otherwise array transformed to `[error, result]` entries by host |
| Error bytes | `Error { name: "ReplyError", message }`; wire message preserved |
| Push | Pub/sub routed to Message; other pushes retained as Value |

Attributes are exposed by the codec but stripped by Connection before dispatch.
No per-command JS closures or callbacks are stored. JS promises/events,
transformers, key prefixes, URL/env config, ready checks via INFO, and pooling
belong to the later host adapter. Built-in ioredis command transformers are not
implemented here. A Buffer suffix affects host decoding, never the wire command.

`SlotMap` consumes CLUSTER SLOTS or SHARDS atomically. `route_command` knows the
listed common key layouts; use `route_keys` for other/module commands, including
complex XREAD/ZUNION layouts. Unknown layouts fail explicitly. All keys must
share a slot. `RedirectTracker` limits retries (default 16): MOVED changes the
slot owner, ASK requires ASKING on the target connection without changing the
stable map. The host retains the logical token and arguments during retries,
refreshes the map on topology changes, and pins transactions to a single slot.
`SentinelDiscovery` iterates seeds, verifies ROLE, and exposes retry deadlines.
Replica selection, Sentinel failover monitoring and cluster connection pooling
remain host policies; this core discovers primaries.

The codec accepts bounded, non-streaming RESP2/3 frames; streamed RESP3 lengths
(`$?`, `*?`, etc.) fail with StreamingUnsupported. Defaults: 16 MiB per frame,
1,000,000 values, 64 nesting levels. Fragment validation is allocation-free;
repeated small fragments can rescan the incomplete frame (quadratic worst case).
The owned result allocates once per blob/aggregate. TX/RX buffers, command replay
buffers, queues and scalar commands reuse capacity after warm-up. Topology,
subscription changes and error/result ownership allocate as documented in the
lane report. No production unsafe code.

Tests: `cargo test -p turnloop-redis`; full real Redis/TLS/cluster/Sentinel tests:
`python3 scripts/test-servers.py run cargo test --workspace -- --include-ignored` from the workspace root.

## Getting started on turnloop

Enable the `turnloop` feature and construct `asynchronous::AsyncConnection::new`
with a connected `turnloop::AsyncIo` (TCP/pipe) or `turnloop_tls::TlsStream` and
this crate's sans-I/O `Connection`. Submit commands through `core_mut()`, then
`next(&executor_handle, |event| { /* consume the event */ Ok(()) }).await`.
Callbacks finish consuming borrowed rows before the next event. Authentication,
entropy and TLS-upgrade requests are explicit events; acknowledge them through
the core. For an upgrade, `into_parts` preserves the stream, core and unread bytes.
Apply `turnloop_io::deadline` using the core's next deadline. Dropping a pending
drive future closes the stream and aborts the core; terminal core events remain
available for draining. See `turnloop-io` for the shared adapter ownership pattern.
