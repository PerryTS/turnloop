# http2-contract — the afterlife of a terminated HTTP/2 stream

Base `bccd379e18`, branch `http2-contract-fixes`, macOS 26.5 arm64, pinned
nightly-2026-08-20, 2026-09-16. Implementation, tests, h2spec and the local lint
ladder complete here. Linux, Windows, WASI 0.2/0.3, web, miri and loom are
**UNRUN here** (integrator CI). Nothing was benchmarked.

Three defects, each proven by a runnable probe before anything was changed
(Perry's `docs/turnloop/http2-contract-probe.rs`, branch `turnloop/http2`), and
each now covered by a test that fails when its own fix is reverted.

**One of the three briefs' premises was wrong and had to be measured out.** The
brief stated that Node answers a post-GOAWAY stream with
`RST_STREAM(REFUSED_STREAM)`; it sends no frame at all. See §2 — the correction,
the raw-socket measurement behind it, and why the fix is now "report and send
nothing" rather than a different hard-coded answer.

**h2spec's strict suite — 147 tests, a required CI job — passed with all three
present, and passes unchanged now.** h2spec drives the subject from the peer
side only: every RST_STREAM in its §5.1 "Stream States" family is one *h2spec*
sends — `closed: Sends a DATA frame after sending RST_STREAM frame`, and so on —
never one the server decides to send, and the example server never closes
gracefully. So the afterlife of a stream the server itself terminated, and a
table filling with the slots those leave behind, are outside what it can reach.
§5.1.2 does fill the advertised concurrent-stream limit, with a hundred *live*
streams, and passes both before and after this change because its check accepts
a connection error as readily as a stream error.

Everything below lives in that afterlife, which is also why §5.1's thirteen
cases pin the shape of the fix rather than obstruct it: the record for a stream
the *peer* reset is deliberately kept, so frames after it stay the error §5.1
asks for, and a HEADERS on a numerically smaller remote id stays a connection
error for §5.1.1.

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
close cannot have seen the GOAWAY yet. The decision sat inside `receive_inner`,
so no host could choose otherwise.

### The premise this lane was given was wrong, and it was measured out

The brief for this work stated, as fact, that *"Node answers
`RST_STREAM(REFUSED_STREAM)` and keeps the session"*. This lane's first fix
implemented that, and an earlier draft of this document asserted it. **It is not
what Node does.** A sibling lane measured it, and rather than take that on
trust this lane measured it again independently, with a raw TCP peer and
hand-built frames — no HTTP/2 library anywhere in the client, so nothing under
test could shape the answer (`nodeprobe/` in this lane's scratch; Node 26.5.1,
`http2.createServer`, one stream held open so the session cannot finish
draining, `session.close()` for the graceful GOAWAY, then HEADERS on an id above
`lastStreamID`):

```
after /close:
   <- HEADERS stream=3 flags=4
   <- DATA stream=3 flags=1
   <- GOAWAY stream=0 flags=0 last=3 code=0
--> sending HEADERS on stream 5 (above lastStreamID 3)
Node's answer to the late stream:
   (NO FRAMES AT ALL)
VERDICT: RST_STREAM for the late stream: NO
VERDICT: any frame at all naming stream 5: NO
```

Node sends **nothing**. The Node application never sees the stream (no `stream`
event fires). The control matters as much as the result: *no frames* could also
mean *stopped reading*, so the probe sends a PING afterwards —

```
--> control: PING after the late stream
   <- PING stream=0 flags=1
CONTROL: session alive and processing our frames: YES (PING ACK)
CONTROL: transport still open: YES
```

— so the session is alive and still servicing our frames. nghttp2 simply does
not process a HEADERS above the GOAWAY's `lastStreamID`, which is RFC 9113
§6.8's "not processed": the peer is expected to retry it on a new connection.

The ladder, then: a connection error (before) is clearly wrong;
`RST_STREAM(REFUSED_STREAM)` (this lane's first attempt) is closer but still not
Node; silence is Node.

**Fix.** A stream that arrives while draining is reported as
`Event::Unprocessed { stream }` and **no frame is sent**. The header block is
still decoded and discarded, because HPACK is connection state.

### Why the core sends nothing rather than choosing for the host

This is the more important half, and it is the criticism that applies to this
lane's own first fix. The original defect was never "turnloop picks the wrong
answer" — it was *"the decision is inside `receive_inner`, so no host can choose
otherwise"*. Swapping one hard-coded answer for another leaves that intact.

The two directions are not symmetrical. `output()`/`consume_output()` is a byte
stream: a host cannot retract a frame the core has already encoded into it
without parsing and splicing its own core's output, which is not a supported
operation and should never be one. Emitting is irreversible. Staying silent is
not: a host that wants to answer calls `reset(id, code)` and gets exactly one
RST_STREAM. So the permissive direction preserves every option and the emitting
direction destroys one — and it destroys precisely the option a host that must
match Node needs.

That is also why `reset` now sends for a stream with no record, instead of
treating it as a no-op. Encoding what the host asks for is this type's job;
deciding not to is the host's. (Nothing depended on the old idempotence; a host
that resets twice now sends two frames, which is a host bug with a harmless
consequence. RFC 9113 §5.4.2's anti-looping rule is about answering a *received*
RST_STREAM, which this is not.)

### A stream reported but not processed must not be named by GOAWAY

Splitting the decision surfaced a second thing. `last_remote` was doing two
jobs: the id GOAWAY names, and the "have I seen this id" watermark that decides
whether a late frame is tolerated or a connection error. Those stopped being the
same quantity the moment a stream could be *reported but declined* — refused
past the limit, or arriving after our own GOAWAY.

RFC 9113 §6.8 lets the peer retry every stream above `lastStreamID` on a new
connection. Naming a declined stream therefore tells the peer that request was
handled when it was not, and it is silently dropped rather than retried — in
exactly the graceful-close path this fix exists for. The field is now two:

* `last_remote` — highest remote id accepted **for processing**; GOAWAY names it.
* `highest_remote` — highest remote id **seen**; the monotonicity check and
  `seen()` use it, so frames the peer had in flight for a declined stream are
  still tolerated.

Both halves are pinned by `h2_goaway_names_the_last_processed_stream_not_the_last_seen`:
collapsing them one way makes GOAWAY over-claim, and the other way makes late
frames for a declined stream fatal again.

### What is *not* copied from Node, and why

REFUSED_STREAM's real trigger is `maxConcurrentStreams`, and this lane measured
both of Node's regimes with the same raw peer:

| when the limit is exceeded | Node 26.5.1 sends |
|---|---|
| before the peer ACKs our SETTINGS | `RST_STREAM stream=5 code=7` (REFUSED_STREAM); the other streams live |
| after the peer ACKs our SETTINGS | `RST_STREAM code=2` then `GOAWAY code=2`, connection closed |

turnloop implements the first and **not** the second. RFC 9113 §5.1.2 says a
HEADERS that causes the advertised limit to be exceeded *MUST* be treated as a
stream error of type STREAM_CLOSED or REFUSED_STREAM; Node's second regime is a
connection error instead. h2spec §5.1.2 exercises this case, so the RFC-shaped
answer is also the one the required gate checks. A host that wants Node's
stricter policy has `settings_awaiting_ack`-equivalent knowledge available to it
and can escalate; the core does not escalate on its behalf.

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
- **HEADERS for a stream *this endpoint itself opened* and has retired**: the
  response a client aborted while it was already on the wire. The block is
  decoded (HPACK is connection state) and discarded, no event and no second
  RST_STREAM. This one was fatal before the change too — `finish_headers`
  answered `STREAM_CLOSED` on a connection error — so it is a defect of the same
  family rather than a consequence, and a client abort is the most ordinary
  operation there is. Only a locally opened id is unambiguous here, which is why
  the rule is not `seen(id)`: a *remote* id at or below `last_remote` may never
  have existed, and h2spec §5.1.1 requires a HEADERS on one of those to stay a
  connection error.

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
  unchanged. Opaque data over the peer's `SETTINGS_MAX_FRAME_SIZE` is refused
  here rather than sent for the peer to answer with `FRAME_SIZE_ERROR`.
- `Event::Headers` gains `kind: HeadersKind::{Head, Informational, Trailers}`.
  The connection already enforced the distinction; the host had to keep its own
  `received_head` to recover it. **Breaking** for an exhaustive pattern without
  `..`; two in-crate sites and one test were updated.
- **Breaking, and worth calling out separately:** `Event` gained a variant, so an
  exhaustive `match` on it needs one more arm. Perry's committed contract probe
  stops compiling until it gets one — a one-line change, and a deliberate signal
  rather than a silent behaviour shift.
- `Event::Unprocessed { stream }` — a stream the peer opened after a graceful
  GOAWAY, for which **nothing has been sent**. The host decides whether to
  answer.
- `Event::Reset` may now name a stream the host never saw opened — a
  concurrent-stream-limit refusal is decided before any `Event::Headers` for
  that stream exists. Documented on the variant.
- `Connection::reset` now sends for a stream with no record left (already reset,
  or refused before one existed) rather than treating it as a no-op, so a host
  can answer an `Event::Unprocessed`. An id this connection never used is still
  an error.
- `send_headers`/`send_data` for a terminated stream report `STREAM_CLOSED`
  rather than the idle/unknown protocol error.

## Verification

| check | result |
|---|---|
| `cargo test --workspace --all-features -- --test-threads=1` | 437 passed, 0 failed, 20 ignored (fixture-backed) |
| `python3 scripts/ci/h2spec.py` | **147 tests, 147 passed, 0 failed, 0 skipped** |
| sabotage: revert each fix in turn | each named test fails; see below |
| `cargo fmt --all --check`, clippy (default + all-features, all-targets) | clean |
| `RUSTDOCFLAGS=-D warnings cargo doc --all-features` | clean |
| `cargo +stable check`, `cargo +1.97.1 check` (MSRV) | clean |
| `check-paths.py`, `feature_modes.py`, `no-tokio.sh` | pass |

A green test proves nothing if the code under it never ran, so each fix was
reverted in place and the suite re-run. Every row was re-measured against the
final tree:

| reverted | tests that fail |
|---|---|
| `reset` returns no credit, frees no slot | `h2_reset_with_unreleased_data_keeps_its_table_slot`, `h2_reset_returns_the_connection_window`, `h2_release_after_termination_is_a_no_op`, `h2_late_frames_for_a_locally_reset_stream_are_ignored`, `h2_response_in_flight_when_the_client_aborts_is_ignored` |
| post-GOAWAY stream is a connection error | `h2_stream_after_graceful_goaway_is_unprocessed_and_unanswered` |
| post-GOAWAY stream answers RST_STREAM (the refuted premise) | `h2_stream_after_graceful_goaway_is_unprocessed_and_unanswered` |
| `reset` will not answer for a stream with no record | `h2_stream_after_graceful_goaway_is_unprocessed_and_unanswered` |
| GOAWAY names the last stream *seen* rather than processed | `h2_goaway_names_the_last_processed_stream_not_the_last_seen` |
| declined ids drop out of `seen()` | `h2_goaway_names_…`, `h2_stream_after_graceful_goaway_…` |
| stream limit is a connection error | `h2_stream_limit_refuses_one_stream_not_the_connection` |
| late DATA for a gone stream is fatal | `h2_late_frames_for_a_locally_reset_stream_are_ignored`, `h2_response_in_flight_when_the_client_aborts_is_ignored` |
| a client's in-flight response after its own reset is fatal | `h2_response_in_flight_when_the_client_aborts_is_ignored` |
| GOAWAY opaque data is not size-checked | `h2_goaway_carries_code_last_stream_and_opaque_data` |

The first row is why `h2_reset_with_unreleased_data_keeps_its_table_slot`
asserts the *events* each stream produced and not merely that the connection
survived: with only the refusal fix in place, a burnt slot answers RST_STREAM
and the connection lives, so "6 connections survived" passes while "6 streams
were accepted" does not.

And the artefact that specified the work — Perry's committed probe, not one line
of it changed — re-run against the fixed crate:

```
== gap 1: a reset stream's table slot ==
-- server resets WITHOUT releasing capacity --
   streams the server accepted: 6            (was 2)
-- server releases capacity, THEN resets --
   streams the server accepted: 6
== gap 3: a stream opened after a graceful GOAWAY ==
  server after late stream: ["Unprocessed s=1"]    (was CONNECTION ERROR PROTOCOL_ERROR)
                                                   (and no "server emitted frame" line at all)
```

The probe prints its `server emitted frame kind=` line only when the server has
at least a frame header of output. Its absence is the probe independently
confirming that nothing was sent — which is what Node does.

## One unrelated Windows defect, surfaced by CI

`test-native (windows-2025, all-features)` failed on
`turnloop-http/interop::curl_and_node_h2_hundred_streams_against_native_server`:

```
thread '<unnamed>' panicked at protocols\turnloop-http\tests\interop.rs:374:
peer did not finish shutdown: An established connection was aborted by the
software in your host machine. (os error 10053)
```

Pre-existing, and not this branch's, established three ways rather than asserted:

1. The server's wire bytes for that exact 100-stream exchange are **identical to
   `main`'s** — 2955 bytes, same FNV-1a — replayed against both trees.
2. The panic is in the socket drain *after* `count == total` is asserted and the
   Node client has printed `100 verified`.
3. Windows **default** and **executor** ran the same test in the same run, curl
   leg included, and passed 9/9. Same code, different outcome — a race.

The drain breaks on EOF and `ConnectionReset` but not `ConnectionAborted`, and
Windows reports the local end of a departed peer as WSAECONNABORTED. The crate
never sees this because it folds the two together everywhere —
`backend/iocp/socket.rs` maps `ERROR_CONNECTION_ABORTED` to `ConnectionReset`,
and `types.rs` and both WASI backends do the same. This test reads a raw
`std::net::TcpStream`, so it is the one place that gets the un-normalised kind.
Folded it the same way; re-running until the race went the other way would have
left the next lane to find it again.

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

## What this cost, and the lesson that is not about HTTP/2

The premise that Node answers `RST_STREAM(REFUSED_STREAM)` after a graceful
GOAWAY travelled through three briefs and was implemented once before anyone put
a socket against Node. It is the same failure mode as the h2spec blind spot in
§0, seen from the other side: h2spec passed with four defects present because it
only drives from the peer side, and a stated behaviour survived three hand-offs
because nobody drove the peer at all. Both are cases of a claim nobody could
falsify from where they were standing.

The cheap guard is the one this lane used twice by accident and once on purpose:
**put a raw socket against the reference implementation, and include a control
that distinguishes "it did nothing" from "it stopped listening".** The PING after
the late stream is what turns "no frames" from a guess into a measurement.

## Not done

- Server push: `turnloop_http::http2` rejects PUSH_PROMISE outright and Perry
  does not implement `createPushResponse`; unchanged.
- **Trailers pipelined onto a stream the server refused.** A peer that sends
  HEADERS + DATA + trailer HEADERS in one burst onto a stream refused for the
  GOAWAY race or the stream limit still fails the connection on the trailer
  block, because a *remote* id at or below `last_remote` cannot be distinguished
  from one that was never opened, and h2spec §5.1.1 depends on that staying an
  error. The DATA in the same burst is tolerated, so the behaviour is
  inconsistent; making it precise needs the refused ids remembered, which is
  state this change deliberately does not add. Unchanged from before.
- Rapid-reset (CVE-2023-44487) accounting. A refused stream costs an HPACK
  decode and one RST_STREAM, the same as before for an accepted-then-reset
  stream, so this change neither adds nor removes that exposure — but the crate
  has no reset-rate bound, and now that a refusal is survivable it is worth one.
- Nothing ran on Linux, Windows or WASI here, and nothing was benchmarked.
