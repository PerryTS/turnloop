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
`python3 scripts/servers.py test` from the workspace root.
