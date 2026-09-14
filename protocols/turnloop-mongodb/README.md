# turnloop-mongodb

Host-driven MongoDB protocol components. The library opens no sockets, reads no clock,
starts no threads and uses no async runtime. It forbids unsafe code. BSON values stay
BSON values; converting them to Perry's JSON strings or JS result shapes is the host's job.

The crate currently provides connection/command/operation state machines, topology and
pool components, and explicit session/transaction/change-stream helpers. It is not a
drop-in implementation of every Node MongoClient feature. See the root LANE_REPORT.md
for the precise coverage and remaining conformance work.

## Driving a connection

1. Parse `Options`. For `mongodb+srv`, execute both `resolution_requests()` and provide
   the answers to `resolve()`. The host opens the resulting address using its transport.
2. Construct `Connection`, call `connected(now, nonce)`. A nonce must be supplied by a
   host CSPRNG for authenticated connections. `UpgradeTls` means finish certificate-
   validated TLS on the transport and call `tls_established()` before sending Mongo bytes.
3. Write `transmit()` and acknowledge only bytes actually written with
   `consume_transmit(n)`. The slice must not be retained across mutating calls; an
   adapter using completion I/O keeps the connection immobile until write completion.
4. Feed plaintext bytes through `receive()`. It returns the consumed prefix: loop on
   the remainder. It deliberately stops at a frame boundary. Drain events and release
   a complete reply before feeding another frame. Partial reads and writes are normal.
5. On `Ready`, submit one command. An accepted token receives one `Reply`, `Failed` or
   `Unacknowledged`; rejected submissions receive no completion. `Reply` is a borrowed
   view obtained with `reply()`. Consume or copy it, then `release_reply()`.
6. Pass transport errors to `fail()`. Drive `handle_timeout(now)` at `next_timeout()`.
   A timed-out connection must be physically closed by the adapter. Closing cancels an
   outstanding command once, followed by `Closed`. A reply already received remains
   readable after close until released.

`Instant` is std::time::Instant on native/WASI. Browser Wasm uses `HostInstant`, created
from host monotonic ticks with `Instant::from_duration`. It has no `now()` method. The
browser host must supply a permitted transport (e.g. a tunnel); browsers do not expose
raw MongoDB TCP sockets.

## Commands and results

`Command` and `WriteModel` retain BSON buffer capacity. Options are **wire fields**, such
as `projection` on find, `fields` on findAndModify, and `new` for return-after. Inputs
are borrowed RawDocuments. Callers must not reuse a builder while its raw view is in use.

```rust
use turnloop_mongodb::{bson::{doc, raw::RawDocumentBuf}, command::Command};
let filter = RawDocumentBuf::try_from(&doc! {"status": "open"}).unwrap();
let options = RawDocumentBuf::try_from(&doc! {"batchSize": 100, "limit": 250_i64}).unwrap();
let mut command = Command::new();
command.find("app", "tasks", &filter, Some(&options)).unwrap();
assert_eq!(command.raw().get_str("find").unwrap(), "tasks");
```

Insert/update/delete use OP_MSG document sequences named documents/updates/deletes.
`BulkBatcher` splits borrowed models by negotiated count, BSON and wire size limits;
pass the **final decorated body** when calculating overhead. `BulkResult` preserves
original error indices across batches. It exposes aggregate wire counts; the caller
maps them and inserted/upserted IDs to the desired JS result structure. Missing `_id`
values must be added before sending; `ObjectIdGenerator` accepts host entropy and Unix
seconds and never reads a clock or OS entropy itself.

`CursorBatch::rows()` yields borrowed RawDocuments without per-row allocation. `Cursor`
tracks namespace, server affinity and client limit. `needs_kill()` means issue
killCursors before discarding its live id. The host retains cursor affinity across
getMore calls and never retries getMore as a retryable read.

## Operations, topology and pooling

`Operation` retains an OP_MSG template. Its actions are Select, Checkout, Send, Waiting,
Complete and Failed. Use `Topology::candidates_deprioritized` and `choose` for selection,
`Pool` for checkout, and `Operation::send` for dispatch. On replies call
`Operation::response`; on failures call `failed`. Only terminal outcomes settle the
user's operation. An initial retryable failure produces a new Select action; the
second attempt keeps lsid/txnNumber and gets a fresh connection request id.

A retryable write needs a host-supplied `RetrySession` with a unique UUID and a txnNumber
incremented once per logical write. Reads can also use this session input. Raw explicit
session fields are preserved and validated against supplied identity. Multi-update,
multi-delete, transaction commands, getMore, $out/$merge and generic runCommand are
excluded from automatic retry. Operation timeouts/cancellation require the adapter to
close an outstanding connection before reusing its transport or pool lease.

`Topology` returns Check, ServerAdded/Removed/Changed, TopologyChanged and ClearPool
requests. Check each server using its own monitor connection; report hello responses
and measured RTT via update, and application errors with their connection generation
via application_error. Pool clearing must follow those requests. Host callbacks for
connections removed by a pool clear are stale and must not be made available again.
Call `operation_started/finished` to maintain the power-of-two selector's operation counts.

`Pool` starts paused; mark it ready after successful monitoring. Execute Connect/Close
requests, and report a connection ready only after its TLS/handshake/authentication
finishes. Checkout tokens are FIFO. Checked-out connections from old generations close
on checkin. Poll and handle its next_timeout for waiters and idle expiry.

`Options::raw` retains accepted write/read concern URI settings. Use `Command::apply_client_options` for URI defaults; existing command values take
precedence. The adapter handles database/collection inheritance. `Connection` itself
does not add concerns or read preference to arbitrary user commands.

## Sessions and change streams

`Session` decorates explicit sessions and transactions, tracks transaction numbers,
gossips cluster time, provides causal-read decoration, caches recovery tokens, and
builds commit/abort. `SessionPool` reuses clean identities until their host-driven expiry.
`TransactionEnd` coordinates one commit/abort retry and final error labeling; failed
abort results are suppressed after that attempt as the specification requires. Retried
commit uses majority, preserves configured wtimeout, and supplies 10 seconds if absent.
Mongos pin/unpin is explicit. Snapshot sessions, a withTransaction callback/replay
controller and complete transaction-spec conformance remain work.

`ChangeStream` tracks resume tokens, startAfter/resumeAfter transition and one-resume
error policy for getMore failures. Initial aggregate errors are terminal. Call observe_document for each emitted event and finish_batch only after
the whole batch is consumed. Build the new aggregate with stage() on resume; preserve
user pipeline/options in the adapter. Closing cursors and driving getMore remain
explicit. Full stream lifecycle orchestration is not yet included.

## Verification

From the workspace root: `scripts/verify-mongodb.sh`. Real servers:
`python3 scripts/mongodb.py run`. The latter starts five isolated MongoDB processes
(standalone, three replica members, TLS standalone), runs the blocking std socket /
rustls tests, then stops its children and verifies all private ports closed. Replica-set
initiation, users and all administrative commands use this crate; no Mongo shell or
official Mongo driver is used. `start` / `stop` are also available for debugging.

Official unmodified JSON fixtures are in tests/spec. COMMIT pins their upstream revision
and LICENSE.md retains their separate upstream license. Suites cover SDAM transitions
and application errors, server selection and max staleness. They exclude load-balancer,
unified integration, event-monitoring and logging fixtures; they are not the complete
MongoDB driver specification suite.
