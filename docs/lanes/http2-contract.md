# http2-contract — the afterlife of a terminated HTTP/2 stream

Base `bccd379e18`, branch `http2-contract-fixes`, macOS 26.5 arm64, pinned
nightly-2026-08-20, 2026-09-16. Implementation, tests, h2spec and the local lint
ladder complete here. Linux, Windows, WASI 0.2/0.3, web, miri and loom are
**UNRUN here** (integrator CI). Nothing was benchmarked.

Three defects, each proven by a runnable probe before anything was changed
(Perry's `docs/turnloop/http2-contract-probe.rs`, branch `turnloop/http2`), and
each now covered by a test that fails when its own fix is reverted.

**h2spec's strict suite — 147 tests, a required CI job — passed with all three
present, and passes unchanged now.** These are cases it does not reach: h2spec
drives one stream at a time against a server that never resets a stream, never
closes gracefully and never fills its table, so the afterlife of a terminated
stream is invisible to it. Everything below lives in that afterlife.

## 1. A reset stream burned its table slot, and the connection then died

`add_stream` recycles a closed stream's slot only when its `unreleased` credit
is zero. `reset` set `local_end`/`remote_end` and left `unreleased` alone, so a
stream reset while it still held DATA credit — which is exactly what a stream
error looks like — held its slot for the life of the connection. When the table
filled, `add_stream` returned `REFUSED_STREAM` *from inside `receive`*, which
set `failed` and emitted GOAWAY; `receive`'s error map had no case for
`REFUSED_STREAM`, so the peer was told PROTOCOL_ERROR and the session died.

Measured on the base commit, a server table of 2 and 6 streams attempted:

```
-- server resets WITHOUT releasing capacity --
  server refused stream #2: CONNECTION ERROR REFUSED_STREAM
   streams the server accepted: 2
-- server releases capacity, THEN resets --
   streams the server accepted: 6
```

`release_capacity`'s contract reads as "return window for data you have
consumed", and a stream you just reset is precisely the data you did *not*
consume, so the working host is the one that reasons about it wrongly.

**Fix.** `reset` returns the stream's outstanding connection-level credit as a
connection WINDOW_UPDATE and drops the record, freeing the slot at once. RFC 9113
§5.1 requires the credit back either way: DATA the peer sent before it saw the
RST_STREAM counts against the connection window whether or not it is delivered,
so leaving it burned the connection window as well as the slot — a second,
silent failure the probe did not reach. `h2_reset_returns_the_connection_window`
covers that half.

Exceeding the concurrent-stream limit is now a **stream** error
(RFC 9113 §5.1.2): RST_STREAM(REFUSED_STREAM) on that stream, the connection
untouched.

## 2. A stream opened after a graceful GOAWAY killed the connection

After `shutdown()`, `receive`'s HEADERS arm rejected a new stream with
`protocol("invalid new stream")` — a **connection** error — because `draining`
was set. The race is unavoidable: a peer that opens a stream during a graceful
close cannot have seen the GOAWAY yet. Node answers
`RST_STREAM(REFUSED_STREAM)` and keeps the session. The decision sat inside
`receive_inner`, so no host could choose otherwise.

**Fix.** A stream that arrives while draining is refused with
RST_STREAM(REFUSED_STREAM) and the session continues, matching Node and RFC 9113
§6.8. The refused header block is still **decoded and discarded**: HPACK is
connection state, and skipping one block desynchronises every block after it.

## 3. `Step`'s zero cases are now a stated, tested contract

`receive` returns `consumed == 0, event == None` (partial preface or partial
frame: **stop**) and `consumed > 0, event == None` (the preface itself, a
SETTINGS acknowledgement, PRIORITY, an unknown frame: **keep going**). Both are
normal and neither was written down. A host looping while "an event came back"
stalls at the *preface* — before a single frame is read, so the connection never
starts.

**Fix.** A `# The Step contract` section in the crate root covering both
decoders in the crate, a per-decoder table on each `Step`, a doctest of the
correct loop, and `step_contract_rejects_both_wrong_loop_conditions`, which
asserts that each wrong condition actually breaks:

* HTTP/2, "loop while an event came back": zero events, not one byte read.
* HTTP/1, "loop while input was consumed": `Event::End` reads no input
  (PerryTS/turnloop#50), so the loop never sees the end of the message.

The test also pins a third edge found while writing it: `receive` is **not
idempotent**. It advances decoder state by `consumed`, so a stalled loop that
retries the same input fails the connection (`FRAME_SIZE_ERROR`, the preface
re-read as a frame header). That is now in the contract.

## Consequences the first two fixes forced, and which stand on their own

Freeing a slot means the record is gone, and a refused stream never had one.
Frames the peer already had in flight then arrive for a stream with no record,
where `index()` produced a connection error. Three of those are races no host
can avoid, and RFC 9113 requires each to be tolerated:

- **DATA for a gone stream** (§5.1): counted against the connection window and
  the credit returned at once, since the payload is discarded. Without this,
  fix 1 only moves the death of the connection one frame later — the ordinary
  "reset a stream mid-body" case.
- **RST_STREAM for a gone stream**: crossed resets are normal; ignored.
- **WINDOW_UPDATE for a gone stream** (§6.9): ignored.

A frame for a stream id this connection has *never used* is still a connection
error, which is what keeps h2spec's `5.1 idle` tests green. The distinction is
`seen(id)`: local ids below `next_id`, remote ids at or below `last_remote`.

A stream the **peer** reset keeps its record (with its credit returned), so
frames after a peer RST_STREAM stay the error RFC 9113 §5.1 asks for and
h2spec's `closed` tests are unchanged. Only frames after *our own* RST_STREAM
are ignored, which is the asymmetry the RFC states.

`release_capacity` for a terminated stream is now a **no-op, not an error**:
a host that buffers a body and releases when the application consumes it can
always call it, whatever happened to the stream meanwhile. Without that, fix 1
would have turned a working host into a broken one, because slot recycling makes
the record disappear under it.

## API

- `Connection::unreleased(id) -> Option<u32>` — the credit the host still owes,
  so the flow-control policy no longer has to mirror the core's counter by hand.
- `Connection::goaway(code, last_stream, opaque)` — the GOAWAY form `shutdown`
  cannot express (`session.goaway(code, lastStreamID, opaqueData)` in Node).
  `shutdown` is now `goaway(0, last_remote, &[])`; the bytes on the wire are
  unchanged.
- `Event::Headers` gains `kind: HeadersKind::{Head, Informational, Trailers}`.
  The connection already enforced the distinction; the host had to keep its own
  `received_head` to recover it. **Breaking** for an exhaustive pattern without
  `..`; two in-crate sites and one test were updated.
- `Event::Reset` may now name a stream the host never saw opened — a refusal is
  decided before any `Event::Headers` for that stream exists. Documented on the
  variant.
- `send_headers`/`send_data` for a terminated stream report `STREAM_CLOSED`
  rather than the idle/unknown protocol error.

## Verification

| check | result |
|---|---|
| `cargo test -p turnloop-http --all-features` | 27 codecs (14 new), 19 interop, 7 server, doctest, all pass |
| `python3 scripts/ci/h2spec.py` | **147 tests, 147 passed, 0 failed, 0 skipped** |
| sabotage: revert each fix in turn | each named test fails; see below |
| `cargo fmt --all --check`, clippy (default + all-features, all-targets) | clean |
| `RUSTDOCFLAGS=-D warnings cargo doc --all-features` | clean |
| `cargo +stable check`, `cargo +1.97.1 check` (MSRV) | clean |
| `check-paths.py`, `feature_modes.py`, `no-tokio.sh` | pass |

A green test proves nothing if the code under it never ran, so each fix was
reverted in place and the suite re-run:

| reverted | tests that fail |
|---|---|
| `reset` returns no credit, frees no slot | `h2_reset_with_unreleased_data_keeps_its_table_slot`, `h2_reset_returns_the_connection_window`, `h2_release_after_termination_is_a_no_op`, `h2_late_frames_for_a_locally_reset_stream_are_ignored` |
| post-GOAWAY stream is a connection error | `h2_stream_after_graceful_goaway_is_refused_not_fatal` |
| stream limit is a connection error | `h2_stream_limit_refuses_one_stream_not_the_connection` |
| late DATA for a gone stream is fatal | `h2_late_frames_for_a_locally_reset_stream_are_ignored` |

The first row is why `h2_reset_with_unreleased_data_keeps_its_table_slot`
asserts the *events* each stream produced and not merely that the connection
survived: with only the refusal fix in place, a burnt slot answers RST_STREAM
and the connection lives, so "6 connections survived" passes while "6 streams
were accepted" does not.

## Should the step contract be generalised to every sans-I/O decoder?

Recommendation: **not as part of this**, and the reason is in the evidence.

All three step-contract incidents — #50 (`http1::Decoder`'s zero-consume
`Event::End`, two lanes), #46 (`Event::Upgrade`'s mode asymmetry, P5) and this
one — are in `turnloop-http`, and both of its decoders are now covered by one
stated contract with tests that fail if it is violated. That is the whole
observed class.

The workspace-wide version is a different, larger job, because the decoders do
not share a shape:

| decoder | step type | zero cases |
|---|---|---|
| `http1::Decoder` | `Step { consumed, event }` | all four combinations occur |
| `http2::Connection` | `Step { consumed, event }` | an event always consumes |
| `compression::Decoder` | `DecodeStep { consumed, written, finished }` | documented; `decode` already errors on both-zero |
| `turnloop_websocket::Connection` | `Received { consumed, message }` | undocumented, same trap |
| postgres / mysql / redis / smtp / mongodb | `receive(&[u8]) -> Result<()>` plus a separate poll | consume the whole buffer; no step at all |

Making that one contract means either a shared `Step` trait the five
protocol crates adopt (a breaking change across the workspace, with a
`turnloop-contract` conformance suite to make it *tested* rather than merely
stated), or a documentation convention that is not enforceable and will drift
the way this one did. Worth doing; worth scoping and reviewing as its own
change rather than riding along with a protocol fix. The one cheap piece that
does not need the design settled is `turnloop_websocket::Received`, which has
the identical two zero cases and no words about them.

## Not done

- Server push: `turnloop_http::http2` rejects PUSH_PROMISE outright and Perry
  does not implement `createPushResponse`; unchanged.
- Rapid-reset (CVE-2023-44487) accounting. A refused stream costs an HPACK
  decode and one RST_STREAM, the same as before for an accepted-then-reset
  stream, so this change neither adds nor removes that exposure — but the crate
  has no reset-rate bound, and now that a refusal is survivable it is worth one.
- Nothing ran on Linux, Windows or WASI here, and nothing was benchmarked.
