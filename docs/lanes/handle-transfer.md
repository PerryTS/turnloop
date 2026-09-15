# handle-transfer — giving a transport's descriptor back to the host (issue #35)

Base: `909e92f`. Branch `lane/handle-transfer`, clone
`/Users/amlug/projects/perry/windlass-lanes/handle-transfer`. Linux (epoll)
coverage ran in `/root/claude-handle-transfer` on the shared build box, under an
`OWNER.txt` naming this lane.

A turnloop socket owned its descriptor and exposed no way to get it back. Perry's
`node:net` migration therefore had to leave outbound TCP clients on tokio, because
`socket.upgradeToTLS` hands a live, **already-connected** socket to a TLS layer
mid-stream — PostgreSQL's `SSLRequest` is exactly that, and Perry has a gap test
for it. Transport was effectively fixed at creation: any socket that *might* be
upgraded later could not start on turnloop at all. `socket._handle.fd`, which Node
exposes, was also unimplementable.

## API

```rust
impl Loop {
    // existing, now also the way out of turnloop entirely
    pub fn detach(&mut self, h: Handle) -> Result<Detached>;

    // new: borrowed, reporting only — Node's socket._handle.fd
    pub fn raw_transport(&self, h: Handle) -> Result<RawTransport>;
}

pub enum RawTransport {
    Fd(i32),        // Unix file descriptor
    Socket(usize),  // Windows SOCKET
    Handle(usize),  // Windows HANDLE: named-pipe instance, console, adopted stream
}

#[cfg(unix)]
impl Detached {
    pub fn into_fd(self) -> OwnedFd;                    // infallible
    pub fn raw_transport(&self) -> RawTransport;
}
#[cfg(windows)]
impl Detached {
    pub fn into_socket(self) -> Result<OwnedSocket>;    // InvalidInput if not a socket
    pub fn into_handle(self) -> Result<OwnedHandle>;    // InvalidInput for a socket,
                                                        // Unsupported for a pipe listener
    pub fn raw_transport(&self) -> Result<RawTransport>;
}

// internal contract (turnloop::backend::Backend), default Unsupported
fn raw_transport(&self, handle: Handle) -> Result<RawTransport>;
```

Decisions, all written into the rustdoc, DESIGN §5a/§7.6 and
`docs/BACKEND_REVISION_2.md` so they cannot drift:

- **Ownership leaves through `detach`, and only through `detach`.** The issue
  offered `Detached::into_fd()` as option 1 and a borrowed `as_raw_fd` as option 2;
  both are implemented, but they are deliberately *different* things. `detach`
  already cancels in-flight work, waits for the terminal completions the host is
  owed, unregisters the resource and removes the handle. Converting the transport
  it returns therefore needs no new guarantee: there is nothing left to cancel, no
  buffer to release, no registration to remove and no completion that can still be
  produced. `raw_transport` adds no guarantee at all — it is an integer the core
  hands through and never acts on.
- **A refusal, never a silent cancellation.** A handle with an outstanding
  operation is `WouldBlock` (the existing `detach` semantics), a closing handle is
  `InvalidInput`, and a handle that is closed, already handed off, or never existed
  is `NotFound`. Nothing is cancelled behind the host's back: the refusal is
  repeatable, the terminal completion still arrives, and only then does `detach`
  succeed.
- **The descriptor is handed over in the mode the loop held it.** Loop-created
  sockets were created non-blocking (with `FD_CLOEXEC` on Unix), so that is what
  the host receives; a blocking TLS handshake is one `set_nonblocking(false)` away.
  What the backend *changed on adoption* is restored first, exactly as on close:
  Unix status flags and termios, Windows console mode. Restoring the mode a
  loop-created socket never had would be an invention.
- **`raw_transport` is narrow on purpose.** It reports; it does not lend. The
  documented rules are: valid until the handle is closed or detached, usable for
  printing, comparing and read-only queries (`getsockname`-class), and not for
  I/O, closing, mode changes, registration with another poller or completion port,
  or anything taking ownership. Those rules are what makes it sound: the function
  is safe, returns a `Copy` integer, and every way to break the loop's contracts
  with it requires the host to make an OS call turnloop cannot see. The narrow
  door for actually taking the descriptor is `detach` + `into_fd`.
- **Non-transport handles say `Unsupported`, not a number.** Timers have no
  descriptor. Process, signal and filesystem-watch handles have internal ones
  (pidfd, signalfd, inotify, `RegisterWaitForSingleObject`) that are turnloop's
  implementation and not the host's resource; reporting them would invite exactly
  the misuse the rules forbid.
- **One documented exception to "no raw fd crosses the backend boundary."** The
  `Backend` rustdoc said no raw fd, OVERLAPPED pointer, pollable or browser object
  crosses the trait. `raw_transport` is now the single exception, and it is stated
  as one: it crosses outwards only, the core never acts on the value, and ownership
  still travels through `detach`/`Detached`. It is an optional method defaulting to
  `Unsupported`, so WASI 0.2, WASI 0.3, web and the driver's own test backend get
  the right answer without implementing anything.

## Per-backend behaviour

| Backend | Owning handoff | Reporting (`raw_transport`) | Notes |
| --- | --- | --- | --- |
| Linux (epoll) | `Detached::into_fd` | `RawTransport::Fd` | `detach` deregisters from epoll first; closing the last descriptor would too, but the loop no longer owns it |
| macOS/BSD (kqueue) | `Detached::into_fd` | `RawTransport::Fd` | identical; termios restored for an adopted terminal |
| Windows (IOCP), socket | `Detached::into_socket` | `RawTransport::Socket` | IOCP association is permanent — see below |
| Windows (IOCP), named-pipe instance | `Detached::into_handle` | `RawTransport::Handle` | keeps `FILE_FLAG_OVERLAPPED`; association is permanent and cannot be duplicated away |
| Windows (IOCP), pipe listener / connecting pipe | refused by `detach` (`Unsupported`) | `Unsupported` (no instance of its own) | unchanged from before this lane |
| WASI 0.2 | `Unsupported` | `Unsupported` | a `wasi:sockets` socket is a component-model resource handle in the component's own table, not a descriptor; no interface hands one to the embedder |
| WASI 0.3 | `Unsupported` | `Unsupported` | same reason |
| Web | `Unsupported` | `Unsupported` | a `WebSocket`/`fetch` resource is a host JS object with no descriptor identity |

### The IOCP association answer

Windows cannot dissociate a handle from a completion port. `CreateIoCompletionPort`
on an already-associated handle fails with `ERROR_INVALID_PARAMETER`, and the
association lives on the *file object*, so `DuplicateHandle` shares it. The
association therefore travels with every socket and pipe instance turnloop hands
over, for the life of that handle.

What saves this is quiescence: `detach` refuses until every operation has
terminated, so **no completion packet can ever be posted to the source loop's port
for that handle by turnloop**. The association is inert. The receiving host has
three ways to work with it:

1. **Synchronous or non-blocking Winsock calls** — `recv`/`send`/`select`, or
   `WSARecv`/`WSASend` with no `OVERLAPPED`. These never involve a completion port.
   This is the supported mode, and it is what a mid-stream TLS upgrade needs.
2. **Overlapped calls with a tagged event** — set the low-order bit of
   `OVERLAPPED.hEvent` (`hEvent | 1`). Windows then does not queue the completion
   packet, and the host waits on its own event and calls `GetOverlappedResult`.
   This is the *only* way to drive a handed-over **named pipe**, because a pipe
   instance keeps `FILE_FLAG_OVERLAPPED` (every `ReadFile`/`WriteFile` needs an
   `OVERLAPPED`) and cannot duplicate out of its association. turnloop's own
   `pipes::Connect` already relies on the inverse of this rule.
3. **Duplicating out of it — sockets only** — `WSADuplicateSocketW` into a
   `WSAPROTOCOL_INFOW`, then `WSASocketW` with `FROM_PROTOCOL_INFO`. The result is
   a *new*, unassociated socket for the same connection, which the host may put on
   its own completion port; the original is then closed.

The one thing a host must not do is an **untagged overlapped call**. Its completion
packet would arrive on the source loop's port carrying an `OVERLAPPED` that loop
does not own. That is not memory-unsafe — `Iocp::entry` already refuses to
dereference a pointer outside its own kernel slab and returns `InvalidInput` — but
the source loop's next `turn` fails, which is not something the host wants.
`into_socket`/`into_handle` say all of this in their rustdoc.

## Files

- `crates/turnloop/src/types.rs` — `RawTransport`.
- `crates/turnloop/src/driver.rs` — `Driver::raw_transport`, and the expanded
  `detach` rustdoc (what the loop guarantees; where ownership goes; WASI/web).
- `crates/turnloop/src/backend/mod.rs` — `Backend::raw_transport` (default
  `Unsupported`), the amended "no raw fd crosses here" statement, and the transfer
  section's new paragraph on ownership leaving through `Detached`.
- `crates/turnloop/src/backend/unix.rs` — `Detached::into_fd`, `raw_transport`,
  `restore()` factored out of `Drop`, and the backend's `raw_transport`.
- `crates/turnloop/src/backend/iocp/mod.rs` — `Detached::into_socket`,
  `into_handle`, `raw_transport`, `take_native`/`restore`, and the backend's
  `raw_transport`.
- `crates/turnloop-contract/src/handoff.rs` — the loop-side contract functions.
- `crates/turnloop-contract/tests/handoff.rs` — the native scenarios, including the
  Windows IOCP and named-pipe probes.
- `crates/turnloop-contract/tests/allocations.rs` — `steady_handoff_allocates_nothing`.
- `crates/turnloop-contract/tests/wasi.rs`, `tests/web/web_contract.rs` — the
  `Unsupported` assertions on the platforms that have no descriptor.
- `protocols/turnloop-tls/tests/upgrade.rs` (+ `Cargo.toml` test target and
  `integration-tests` metadata) — the upgrade scenario end to end.
- `DESIGN.md` §5a (handle transfer), §6 (API sketch), §7.6 (two new matrix rows);
  `docs/BACKEND_REVISION_2.md` (boundary table row and a "Descriptor handoff"
  section).

Only one dependency line changed: `turnloop-contract`'s existing `windows-sys`
gained the `Win32_Networking_WinSock` feature, for the Winsock probes in the test.
No new crates.

## Tests — and how each one proves its subject

Every one of these goes around turnloop for the part that matters: it uses the
descriptor from the test process with `libc`/Winsock or `std::net`, so a backend
that kept any claim on the transport could not pass.

**`a_handed_off_socket_carries_bytes_after_its_loop_is_dropped`** — the headline.
The peer is a plain `TcpStream` the test owns, so it outlives the loop. Bytes are
exchanged *through the loop* first (the connection is provably live and
mid-stream), the socket is handed over, the loop is asserted quiet and `!alive()`,
**the whole `Loop` is dropped**, and only then do bytes flow both ways over the
returned descriptor. Nothing turnloop owned can be keeping that connection open.

**`a_handed_off_socket_is_driven_by_the_bare_descriptor`** — the same, with no std
wrapper at all: `libc::send`/`libc::poll`/`libc::recv` (Winsock `send`/`recv` on
Windows) straight on the number the loop returned. It also asserts the returned
descriptor equals what `raw_transport` reported before the handoff, so a backend
handing back a *different* (for example duplicated) descriptor would fail.

**`a_handed_off_listener_accepts_in_the_host`** — a listener is handed over and
becomes a `std::net::TcpListener`; the loop then connects *to it as a client*, the
test accepts, and a loop-side write is read on the host-accepted socket.

**`listeners_and_accepted_sockets_are_handed_off`** — both ends of an accept, and
the liveness assertion that the loop's accounting really dropped them.

**`pending_operations_refuse_handoff`** — a provided-buffer read is outstanding;
`detach` is `WouldBlock`, twice (the refusal does not cancel behind the host's
back); the turn delivers exactly one `Cancelled`; the caller's buffer is
byte-for-byte untouched; `detach` then succeeds; a second `detach` and
`raw_transport` are `NotFound`; submitting is refused; and 60 ms of turning
produces **zero** completions.

**`closing_and_closed_handles_refuse_handoff`** — `InvalidInput` while closing,
`NotFound` after `Closed`.

**`raw_transport_reports_live_transports`** — the value is stable across calls,
different for two live transports, unchanged after the socket is used, and the
socket still works after being reported (reporting is read-only). A timer reports
`Unsupported` from `raw_transport` and `InvalidInput` from `detach`.

**`a_handed_off_socket_keeps_its_association_and_duplicates_out_of_it`**
(Windows) — `CreateIoCompletionPort` on the handed-over socket fails with
`ERROR_INVALID_PARAMETER`; synchronous Winsock I/O then carries bytes both ways;
the loop stays quiet; and `WSADuplicateSocketW` + `WSASocketW` produces a socket
that **does** associate with the test's own port. The documented escape hatch is
executed, not asserted in prose.

**`a_handed_off_named_pipe_is_driven_with_a_tagged_event`** (Windows) — a
connected named-pipe instance is handed over; association is proved permanent the
same way; then `WriteFile`/`ReadFile` with `OVERLAPPED.hEvent | 1` carry bytes both
ways while the loop still owns the other end, and the loop is asserted quiet
afterwards — i.e. the tagged event really did keep the packet off the loop's port.

**`a_plaintext_socket_is_handed_off_mid_stream_for_a_real_tls_handshake`**
(`turnloop-tls`) — Perry's actual pattern. The client sends PostgreSQL's 8-byte
`SSLRequest` through the loop and reads the server's `S` through the loop; the
socket is detached, converted, the loop dropped; and a **real rustls handshake**
runs on that descriptor, with ALPN `h2`, `HandshakeKind::Full` asserted (so it
cannot be a resumption of some other connection) and encrypted `ping`/`pong`
exchanged. No part of the handshake goes through turnloop.

**`transports_have_no_descriptor_to_hand_out`** (WASI 0.2 and 0.3) — `detach` and
`raw_transport` are `Unsupported` for a listener, a client and an accepted socket,
and the connection still works afterwards, so the refusal is inert.
`capability_errors_and_oversize_response_are_terminal` (web) gained the same
`raw_transport` assertion next to the existing `detach` one.

**Allocation gate**: `steady_handoff_allocates_nothing` runs 100 measured cycles of
`raw_transport` → `detach` → `into_fd`/`into_socket` → re-adopt → `attach`,
asserting `0` allocations, that the identity handed over matches the one reported
on every cycle, and — after the hundred round trips — that the socket still carries
bytes. It cannot pass having done nothing.

## Verification

macOS (kqueue) is this machine, `aarch64-apple-darwin`, pinned
`nightly-2026-08-20`. Linux (epoll) is `x86_64-unknown-linux-gnu` on the shared
build box.

| Command | Result |
| --- | --- |
| `cargo +nightly-2026-08-20 fmt --all --check` | PASS |
| `python3 scripts/ci/check-paths.py` | PASS — 1505 tracked files, 266 references |
| `python3 scripts/ci/feature_modes.py` | PASS — 6 public features, 18 required arms |
| `cargo +nightly-2026-08-20 clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS (macOS) |
| same, `--target x86_64-unknown-linux-gnu`, `-p turnloop -p turnloop-contract -p turnloop-io` | PASS |
| same, `--target x86_64-pc-windows-msvc` | PASS |
| same, `--target wasm32-wasip2` | PASS |
| same, `--target wasm32-unknown-unknown` | PASS |
| `cargo +nightly-2026-09-07 clippy … --target wasm32-wasip3 --all-features` | PASS |
| `RUSTDOCFLAGS='-D warnings' cargo +nightly-2026-08-20 doc --locked --workspace --all-features --no-deps` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test --locked --workspace --no-fail-fast -- --test-threads=1` (macOS) | PASS — exit 0, 66 `test result: ok` groups, 0 FAILED |
| `cargo test -p turnloop-contract --test handoff -- --test-threads=1` (macOS) | PASS — 7/7 |
| `cargo test -p turnloop-tls --features turnloop --test upgrade -- --test-threads=1` (macOS) | PASS — 1/1 |
| Linux six required modes (`default`, `executor`, `epoll-timerfd`, `process-sigchld`, `fallbacks`, `all-features`), each `cargo +nightly-2026-08-20 test --locked --workspace --no-fail-fast … -- --test-threads=1 --skip permission_denied_is_reported` | PASS — all six exit 0, 413 `test result: ok` lines, 0 FAILED; the handoff suite and the allocation gate each ran 6×, the TLS upgrade test once (its `turnloop` feature is only on in `all-features`) |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` (Wasmtime 46.0.0) | PASS — 43 contract (incl. `transports_have_no_descriptor_to_hand_out`) + 13 allocation tests |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` (nightly-2026-09-07) | PASS — 43 contract + 14 allocation tests |
| `python3 scripts/ci/run-tests.py node` (web backend under Node 26.5.1) | PASS — 16/16 |
| `bash scripts/ci/no-tokio.sh` | PASS — 5 graphs, zero runtime crates |
| `python3 scripts/ci/soak.py` | PASS — 251 locked versions, 1 pre-existing rustls exception |
| `cargo +nightly-2026-08-20 deny --locked check` | PASS — advisories, bans, licenses, sources |
| `python3 scripts/ci/lint-workflows.py` | PASS — no workflow files were changed |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py'` | PASS — 98 tests |
| `python3 scripts/ci/run-tests.py web` (headless Chromium/Firefox) | UNRUN locally (no browsers on this machine); covered by CI |
| Windows `cargo test` (the IOCP and named-pipe probes) | UNRUN locally; cross-compiled clean and covered by the three `windows-2025` CI arms |

### Pre-existing failure seen on the build box (not this lane)

`turnloop-contract --test filesystem permission_denied_is_reported` fails there
because the box runs as **root**, and root ignores a read-only file's mode. It is
the same failure the sockopts lane recorded on its own base commit, in filesystem
code this lane does not touch; CI runs unprivileged. The first Linux run
(`run-tests.py native --mode default`) reproduced exactly that one failure and
nothing else, which is why the six-mode matrix skips it by name.

### CI

| Run | SHA | Result |
| --- | --- | --- |
| _(filled in below)_ | | |

## Follow-ups this lane deliberately did not take

- **TLS on turnloop as the answer for the upgrade case** (the issue's option 3).
  `turnloop-tls` exists and Perry P5 uses it; this lane makes the *general* handoff
  work, which is what unblocks P1 now and what `socket._handle.fd` needs anyway.
- **No automatic duplication on Windows.** `into_socket` could have duplicated out
  of the IOCP association for the host, but a pipe cannot (the association is on
  the file object), so it would be an inconsistency dressed as a convenience — and
  it would silently change the socket the host asked for. The recipe is documented
  and tested instead.
- **No `Detached::into_fd` for WASI.** A `wasi:sockets` resource could in principle
  be handed to another component, but there is no descriptor and no interface for
  it; inventing one would be a `wasi:sockets` proposal, not a turnloop change.
- **No cross-process handoff surface.** `send_handle`/`recv_handle` already move
  transports between processes; this lane is about leaving turnloop entirely.
