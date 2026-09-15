# linger-close — issue #21

Base `f6fc128`, branch `lane/linger-close`, macOS 26 arm64 (Darwin 25.5), Node
26.5.1, 2026-09-15. Implementation, tests and local verification complete.
Linux, Windows and WASI 0.3 **runtime UNRUN here** (integrator CI).

## Root cause

`server::http2` closed the socket as soon as `core.is_drained()` held, and
`server::http1` right after its final response (and dropped idle keep-alive
connections outright at shutdown). A peer's last bytes could still be unread in
the server's receive buffer: HTTP/2 SETTINGS/WINDOW_UPDATE acknowledgements, a
PING, a pipelined HTTP/1 request. `close()` on a TCP socket with unread receive
data sends RST instead of FIN. The peer then reads `ECONNRESET` instead of EOF,
and a reset can discard data it has not read yet (probe on this Mac: a 1 MB
response closed with unread peer bytes delivered 875,452 bytes before
`ECONNRESET`; small responses arrived intact but ended in `ECONNRESET`, which is
what Node's `ClientHttp2Stream` reported in PR #20's macOS run). The same close was
used by `WebSocketStream::close`.

## API

**turnloop (`executor.rs`)**, `AsyncIo<B>`:

- `poll_shutdown(&mut self, cx) -> Poll<io::Result<()>>`: flush accepted writes,
  then submit the backend's `Operation::Shutdown` (queued after earlier writes)
  **without closing the handle**. Reads continue until the peer's EOF. Idempotent
  after completion; writes after (or during) a half-close fail with `BrokenPipe`;
  a pending shutdown is owned by the adapter and abandoned on drop. UDP adapters
  return `io::ErrorKind::Unsupported` without submitting.
- `executor() -> ExecutorHandle<B>` (Rc clone, no allocation) and `is_write_shut()`.
- `poll_close` is unchanged: it still fully closes the handle.

**turnloop-io**:

- `trait HalfClose: Stream { type Backend; fn executor(&self) -> Option<ExecutorHandle<_>>; fn poll_shutdown(...) }`,
  implemented for `AsyncIo`, `turnloop_tls::asynchronous::TlsStream` and `Transport`.
- `shutdown(&mut stream)` next to the unchanged `close(&mut stream)`.
- `linger_close(&mut stream, &mut scratch, Option<Instant>) -> LingerClose` (future
  with `set_deadline`), output `io::Result<Lingered { end, reads, discarded }>`,
  `LingerEnd::{Eof, Deadline, Unsupported, Failed(kind)}`. Phases: half-close →
  read-and-discard into caller scratch until EOF/error/deadline → `poll_close`.
  The deadline bounds the half-close too. `Err` only for empty scratch, a half-close
  that cannot finish by the deadline (`TimedOut`: peer stopped reading) or a failed
  final close; the caller then drops the stream. It waits on the stream's read and
  one executor timer, re-arming the timer only when the deadline moves.

**turnloop-tls**: `TlsStream::poll_shutdown` = drive close_notify, flush it, then
the transport's `poll_shutdown`. Decryption continues until the peer's
close_notify/EOF. `poll_write` now refuses once close_notify was sent (previously
it encrypted a record the transport then rejected, which blocked later reads).

**turnloop-http** (`asynchronous::server`):

- `Options { linger_timeout: Duration }`, default **5 s** (nginx
  `lingering_timeout`); `Server::bind_with(executor, addr, options)`,
  `Shutdown::with_options`, `Shutdown::options`.
- `Shutdown::stop_by(deadline)`: stop and close every lingering connection no
  later than `deadline` (earlier wins; connections already lingering are woken and
  re-arm). `Shutdown::deadline()`. In-flight requests still finish; dropping the
  `Server` cancels them, as before.
- `server::http1`/`http2` now require `S: HalfClose` (all in-tree transports
  implement it).

**turnloop-websocket**: `WebSocketStream::close` (now on `S: HalfClose`) runs the
closing handshake under its deadline, then a lingering close under the same deadline.

## Server behaviour

- HTTP/1: after a final response that ends the connection (`connection: close`,
  non-reusable request, or shutdown), and for a connection idle between requests
  when shutdown arrives: half-close, discard until EOF or
  `min(now + linger_timeout, stop_by deadline)`, close. The idle wait now reads
  outside the codec, so a shutdown no longer cancels a codec read (which dropped the
  socket); a stop that interrupts a partially received head still releases the
  transport immediately, as before. A buffered pipelined request is not served
  after stop (unchanged) and is discarded.
- HTTP/2: when drained (GOAWAY sent, streams done, output flushed): flush, then the
  same lingering close. Protocol-error closes are unchanged (see follow-ups).
- A completed exchange never becomes an error during lingering: half-close/read
  failures end lingering (`Failed`), a failed final close drops the transport.
- Discard uses the connection's retained input buffer (`Vec` resized within its
  capacity), so no allocation per read; the linger future waits on one read and one
  timer (no spin, DESIGN §10 rule 4a). A `stop_by` deadline change costs one
  nonblocking turn that delivers the replaced timer's `Cancelled`/`Closed`
  (rule 3: at most one zero-timeout discovery poll beside the pending read).

## Per platform

| Backend | `AsyncIo::poll_shutdown` | Notes |
|---|---|---|
| epoll / kqueue (`unix.rs`) | TCP and Unix-domain stream sockets: `shutdown(SHUT_WR)` | ttys, FIFOs and regular files (`Kind::Stream`/`File`) now fail at submit with `Unsupported` (was an async `ENOTSOCK` completion / `Unsupported` from the file pool) |
| IOCP | TCP: `shutdown(SD_SEND)` | named pipes, sync/console handles: `Unsupported` at submit (existing validation, now documented) |
| WASI 0.2 | TCP: `tcp-socket.shutdown(send)`; stdout/stderr close their output stream | stdin/UDP/listeners: `InvalidInput` (existing) |
| WASI 0.3 | TCP: closes the send stream and awaits its result | as 0.2 |
| web | host WebSocket: starts its close; others `Unsupported` | no listeners on the web backend |

UDP `AsyncIo` adapters return `Unsupported` before submitting. When the transport
cannot half-close, `linger_close` closes immediately (`LingerEnd::Unsupported`).

## Tests

All deterministic: the server transport (`Gated` test wrapper) holds its first
`poll_close`/`poll_shutdown` until the peer has written late bytes after the
server's last read, and the peer reads nothing until the server's close/half-close
completed. Unread bytes are therefore in the server's receive buffer at close time
(or arrive at a closed socket, which also resets).

| Test | Proves |
|---|---|
| `turnloop-http/asynchronous::http1_lingering_close_delivers_response_before_pipelined_request` (+ `https1_…`) | 32 KiB response then clean EOF with a pipelined request unread; server discarded exactly the request |
| `…::http2_lingering_close_delivers_response_despite_unread_frames` (+ `https2_…`) | client sends only preface+request, then SETTINGS ACK + WINDOW_UPDATE + PING after the server drained; full 32 KiB DATA, END_STREAM, GOAWAY(0), clean EOF |
| `…::http1_idle_keepalive_lingers_at_shutdown` | idle keep-alive connection at `stop()` half-closes and drains a request racing the stop |
| `…::http_linger_timeout_closes_silent_peer_without_spinning` | peer never closes: server closes at `sent + 500 ms`, ≤ 1 turn and ≤ 1 zero-event wait for the expiry |
| `…::http_stop_by_deadline_ends_lingering_connection` | 60 s linger ended by `stop_by(now + 100 ms)`; `run_ready()` shows exactly the lingering task woken; ≤ 2 turns, ≤ 2 zero-event waits |
| `turnloop-http/server_allocations` (new harness=false gate, `integration-tests`) | HTTP/1 and HTTP/2 discard phase (half-close done → close): 0 allocations over ~256 separate reads, 1,024 bytes |
| `turnloop-websocket/asynchronous::server_close_lingers_until_client_eof` | server `close()` after the closing handshake drains late client bytes; client reads EOF |
| `turnloop-io/streams::half_close_keeps_reading_until_peer_eof` | EOF delivered, idempotent shutdown, writes rejected, reads continue |
| `…::lingering_close_discards_peer_input_until_eof` | `Lingered { end: Eof, discarded: 3072 }` |
| `…::lingering_close_deadline_closes_silent_peer_without_spinning` | `end: Deadline` at the deadline, ≤ 1 turn / ≤ 1 zero-event wait |
| `…::lingering_close_deadline_bounds_a_stalled_half_close` | a peer that never reads: `TimedOut` at the deadline instead of hanging |
| `…::half_close_is_unsupported_on_datagram_adapters` | UDP: `Unsupported`, `LingerEnd::Unsupported` |
| `turnloop-io/allocations::lingering_close_discards_without_allocating` | whole `linger_close` (half-close, 238–256 separate reads, timer, close) after a warm-up connection: 0 allocations |
| `turnloop-tls/asynchronous::tls_half_close_sends_close_notify_and_keeps_decrypting` | close_notify reaches the peer as EOF, server still decrypts the peer's later data |

Existing Node/curl interop tests are unchanged and pass.

### Proof the tests fail without the fix

Temporarily restoring the old `turnloop_io::close` in `server.rs` (both
post-response sites), 3 runs each, every run failed:

- `http1_…`, `http2_…` (plain): `reset before EOF after 32855 bytes: Connection reset by peer (os error 54)`.
- `https1_…`, `https2_…`: client's close_notify on the reset socket: `Broken pipe (os error 32)`.
- Old idle path (`shutdown.until(conn.head())` → return): `http1_idle_keepalive_lingers_at_shutdown` hangs (no graceful close ever) → `lingering exchange hung` after 10 s.
- Old `WebSocketStream::close`: `server_close_lingers_until_client_eof` → `ConnectionReset (os error 54)`.

Gate mutations (then restored, checksums verified):

- `cx.waker().wake_by_ref()` before returning Pending from the discard step: io and
  both HTTP deadline tests fail with `lingering close spun`.
- `Box::new(n)` per discard read: io gate reports 256 allocations; one `Box` per
  HTTP linger poll: server gate reports 257.

## Verification

Toolchain: pinned `nightly-2026-08-20`, `CARGO_BUILD_JOBS=4`, one heavy command at
a time. WASI/web C for `ring`: Homebrew LLVM clang/llvm-ar via `CC_<target>`/`AR_<target>`
(CI uses the pinned wasi-sdk). Wasmtime 46.0.0 installed by `scripts/ci/install-wasmtime.sh`.

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **PASS** |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS** |
| same with `--all-features` | **PASS** |
| `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-io --all-targets --target x86_64-unknown-linux-gnu [--all-features] -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS** (default and all-features) |
| same for `--target x86_64-pc-windows-msvc` | **PASS** (default and all-features) |
| same for `--target wasm32-wasip2 --all-features` (core crates) | **PASS** |
| `cargo clippy --locked --workspace --all-targets --target wasm32-wasip2 --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS** |
| `cargo clippy --locked --workspace --all-targets --target wasm32-unknown-unknown --all-features -- …` | **PASS** |
| `cargo test --workspace -- --test-threads=1` | **PASS**: 266 passed, 0 failed, 13 ignored |
| `cargo test --workspace --all-features --no-fail-fast -- --test-threads=1` | **PASS**: 333 passed, 0 failed, 20 ignored (all new tests included) |
| `python3 scripts/test-servers.py --services http run python3 scripts/ci/run-tests.py interop --mode default` | **PASS**: http interop 9, asynchronous 20 (incl. ignored Node fixture test), server_allocations 2, tls tls 5 / asynchronous 3 / async_allocations 1, websocket websocket 3 / asynchronous 3 / async_allocations 3 |
| same `--mode executor` | **PASS** (same 9 suites and counts) |
| same `--mode all-features` | **PASS** (same 9 suites and counts) |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2 --package turnloop-io --package turnloop-tls --package turnloop-http --package turnloop-websocket` | **PASS**: io streams 7, io allocations 2 (discard gate: 238 reads, 0 allocations), http asynchronous 16 (all 7 new), codecs 16, allocations 3, tls asynchronous 3, async_allocations 1, portable 3, channel_binding 3, websocket asynchronous 2, async_allocations 3 |
| `bash scripts/ci/no-tokio.sh` | **PASS** (every policy triple, default and all-features) |
| `python3 scripts/ci/soak.py` | **PASS**: 251 locked registry versions, 1 active security exception (inherited rustls 0.23.45) |
| `python3 scripts/ci/check-paths.py`, `python3 scripts/ci/feature_modes.py`, `python3 -m unittest discover -s scripts/ci -p 'test_*.py'` | **PASS** (98 unit tests) |
| No-fix and mutation probes above (run again on the final tree) | **FAIL as intended**, sources restored and checksummed |
| Linux / Windows / WASI 0.3 / web runtime, h2spec, instruction baselines | **UNRUN** (integrator CI) |

The linger-timeout tests were widened from 200 ms to 500 ms after the runs above
for slow-runner headroom (the settle turns must not overrun the deadline); the
affected io/http/tls/websocket suites were re-run afterwards: **PASS**.

## Follow-ups / not covered

- HTTP/2 protocol-error closes (`Http2::event` drops the transport after flushing
  GOAWAY) still close immediately; lingering there needs `Http2::event` to keep the
  transport on error, which changes client semantics too.
- `Shutdown::stop_by` bounds lingering only, not in-flight requests.
- `server_allocations` is registered as a native `integration-tests` suite only, not
  in `wasi-tests` (WASI 0.3 is unverified locally); the turnloop-io discard gate
  already runs on WASI.
- Linux, Windows (IOCP `SD_SEND`, named-pipe `Unsupported`), WASI 0.3 and web runtime
  behaviour is covered only by CI; the macOS RST reproduction is the local evidence.
  Without the fix the reset should surface on Linux and Windows too (as `ECONNRESET`
  or lost bytes), but that failure mode was only reproduced on macOS.
