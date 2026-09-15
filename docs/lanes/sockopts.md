# sockopts — socket options on live handles (issue #34)

Base: `09d205f`. Branch `lane/sockopts` (PR #36), clone
`/Users/amlug/projects/perry/windlass-lanes/sockopts`. Linux coverage ran in
`/root/claude-sockopts` on the shared build box. Implementation and verification
complete: CI run 34991952873 is green on every platform arm.

Perry's `node:net` migration could not implement `socket.setNoDelay()`,
`setKeepAlive()` or `dgram.setTTL()`: turnloop took `nodelay` only in `ConnectOpts`
at creation, `ListenOpts` had no such field, and there was no way to change an
option on a live handle at all — so an **accepted** socket, which is what a server
configures, was unconfigurable.

## API

```rust
impl Loop {
    pub fn set_option(&mut self, h: Handle, option: SocketOption) -> Result<()>;
    pub fn get_option(&self, h: Handle, kind: SocketOptionKind) -> Result<SocketOption>;
}

pub enum SocketOption {
    NoDelay(bool),                      // TCP_NODELAY
    KeepAlive(Option<KeepAlive>),       // SO_KEEPALIVE + schedule; None disables
    Linger(Option<Duration>),           // SO_LINGER; Some(ZERO) resets on close
    RecvBufferSize(u32),                // SO_RCVBUF
    SendBufferSize(u32),                // SO_SNDBUF
    Ttl(u32),                           // IP_TTL / IPV6_UNICAST_HOPS
    Ipv6Only(bool),                     // IPV6_V6ONLY (bind-time; see below)
    Broadcast(bool),                    // SO_BROADCAST
    MulticastTtl(u32),                  // IP_MULTICAST_TTL / IPV6_MULTICAST_HOPS
    MulticastLoop(bool),                // IP_MULTICAST_LOOP / IPV6_MULTICAST_LOOP
    MulticastJoin(MulticastGroup),      // IP_ADD_MEMBERSHIP / IPV6_JOIN_GROUP
    MulticastLeave(MulticastGroup),     // IP_DROP_MEMBERSHIP / IPV6_LEAVE_GROUP
}
pub struct KeepAlive { idle: Option<Duration>, interval: Option<Duration>, count: Option<u32> }
pub struct MulticastGroup { group: IpAddr, interface: u32 }

pub enum SocketOptionKind { NoDelay, KeepAlive, Linger, RecvBufferSize,
                            SendBufferSize, Ttl, Ipv6Only, Broadcast,
                            MulticastTtl, MulticastLoop }

pub struct ListenOpts { reuse_port: bool, backlog: u32, accept_defaults: AcceptDefaults }
pub struct AcceptDefaults { nodelay: bool, keep_alive: Option<KeepAlive> }
```

Decisions, all of them written into the rustdoc and DESIGN §7.7 so they cannot
drift:

- **Synchronous, not an operation.** Both calls run inside the call: no Request is
  accepted, no completion is produced, nothing is queued, nothing is allocated.
  Same shape as `tty_set_mode`/`local_addr`.
- **`get_option` never answers from a cache.** Every call is a `getsockopt` (or the
  matching `wasi:sockets` import), because the OS is entitled to round, clamp or
  double a request — `SO_RCVBUF` on Linux doubles it. Reporting the *request* back
  would be a lie the host acts on.
- **A getter key, not a value, for reads.** `MulticastJoin`/`MulticastLeave` have no
  `SocketOptionKind`: no OS exposes a per-socket membership query, and a kind that
  answered `Unsupported` everywhere would be worse than its absence.
- **Bind-time options stay in the opts structs.** `SO_REUSEPORT` is
  `ListenOpts::reuse_port`/`UdpOpts::reuse_port`, and `SO_REUSEADDR` is applied by
  the backend to every TCP listener it binds. Neither can be changed on a bound
  socket, so neither is an option. `Ipv6Only` is the borderline case: it is kept in
  the enum so it can be *read* on a live socket, and setting it after bind is
  refused by the OS (asserted, not just documented).
- **`accept_defaults` applies before the host sees the connection.** The accepting
  backend applies them to the accepted socket after the OS accept and before the
  `Accepted` completion is produced. A backend that cannot apply a requested
  default rejects the **listener** when it is created rather than ignoring the
  request once per connection; an OS failure while applying one fails that accept
  and drops the socket instead of handing up a half-configured connection. Each
  field is at most one `setsockopt` — that is what "cheaply" is allowed to mean.
- **Granularity.** Native keep-alive and linger schedules are whole seconds, so a
  `Duration` is rounded **up** and a zero keep-alive interval is `InvalidInput`
  rather than silently becoming "immediately". WASI takes the duration unrounded.
- **Buffer sizes are kernel-chosen; the contract is a floor, not an equality.**
  Measured on an accepted loopback socket: macOS defaults to **408300** bytes and
  keeps a request exactly; Linux defaults to **87380**, doubles the request and
  clamps it to `net.core.rmem_max`; Windows rounds up to its own granularity and
  refused to shrink an auto-tuned window from 131072 to a 49152 request. There is
  therefore no portable exact expectation, and no portable *direction* either, so
  the API documents "at least what you asked for, read it back" and the tests
  assert that. Recorded on `SocketOption::RecvBufferSize`.
- **Partial failure is reported, not hidden.** Keep-alive is a switch plus up to
  three separate kernel settings. Every value is validated before any is written,
  but if the OS still rejects one after the switch was set, the error is returned
  and the socket keeps whatever the OS left — documented on the variant, because a
  silent rollback that itself failed would be worse.
- **Handle rules.** A timer handle is `InvalidInput`; a closing handle is
  `InvalidInput`; a released handle is `NotFound`; stdio/file/TTY, process, signal
  and fs-watch handles are `Unsupported`.

### One behaviour change outside the strict ask

`Open::Tcp` on WASI 0.2/0.3 previously **dropped `TcpOpts::nodelay` on the floor**
(`Open::Tcp { addr, .. }`), because `wasi:sockets` has no Nagle control. Leaving
that while making `SocketOption::NoDelay` and `AcceptDefaults::nodelay` report
`Unsupported` on the same backend would have been indefensible, so the
creation-time hint is now reported too, and the shared `pair()` fixture in
`turnloop-contract` stops requesting `nodelay: true` (native coverage of that path
moved into `nodelay_round_trip_and_accept_default`, which asserts it through the
OS instead of assuming it). Revert this hunk if the spec owner prefers the hint.

## Per-backend support matrix

`Unsupported` in this table means the call is **reported**, never accepted and
ignored.

| Option | Linux (epoll) | macOS / BSD (kqueue) | Windows (IOCP) | WASI 0.2 | WASI 0.3 | Web |
|---|---|---|---|---|---|---|
| `NoDelay` | ✓ `TCP_NODELAY` | ✓ | ✓ | **Unsupported** | **Unsupported** | **Unsupported** |
| `KeepAlive` | ✓ `SO_KEEPALIVE` + `TCP_KEEPIDLE`/`_KEEPINTVL`/`_KEEPCNT` | ✓ (`TCP_KEEPALIVE` is Apple's idle name) | ✓ (schedule needs Win10 1709+; older Winsock returns its own error) | ✓ `keep-alive-{enabled,idle-time,interval,count}` | ✓ (`get_`-prefixed getters) | **Unsupported** |
| `Linger` | ✓ `SO_LINGER` | ✓ | ✓ (`LINGER`, `u16` fields) | **Unsupported** | **Unsupported** | **Unsupported** |
| `RecvBufferSize` / `SendBufferSize` | ✓ (kernel doubles) | ✓ | ✓ | ✓ TCP and UDP | ✓ | **Unsupported** |
| `Ttl` | ✓ `IP_TTL` / `IPV6_UNICAST_HOPS` | ✓ | ✓ | ✓ `hop-limit` / `unicast-hop-limit` | ✓ | **Unsupported** |
| `Ipv6Only` | read ✓; set refused after bind by the OS | same | same | **Unsupported** | **Unsupported** | **Unsupported** |
| `Broadcast` | ✓ `SO_BROADCAST` | ✓ | ✓ | **Unsupported** | **Unsupported** | **Unsupported** |
| `MulticastTtl` / `MulticastLoop` | ✓ (`int`) | ✓ (`u_char` for IPv4, `int` for IPv6 — written and read at the right width, not left to byte order) | ✓ (`DWORD`) | **Unsupported** | **Unsupported** | **Unsupported** |
| `MulticastJoin` / `MulticastLeave`, IPv6 | ✓ `ipv6_mreq`, any interface index | ✓ | ✓ | **Unsupported** | **Unsupported** | **Unsupported** |
| `MulticastJoin` / `MulticastLeave`, IPv4 | ✓ `ip_mreq` (index 0) or `ip_mreqn` (index *n*) | ✓ index 0; **Unsupported** for a nonzero index | ✓ index 0; **Unsupported** for a nonzero index | **Unsupported** | **Unsupported** | **Unsupported** |
| `AcceptDefaults::nodelay` | ✓ | ✓ | ✓ | listener rejected at creation | listener rejected at creation | no listeners at all |
| `AcceptDefaults::keep_alive` | ✓ | ✓ | ✓ | ✓ | ✓ | no listeners at all |

Why each `Unsupported` is real, not laziness:

- **WASI 0.2/0.3.** `wasi:sockets@0.2.9` / `0.3.0` expose exactly keep-alive
  (enabled + idle + interval + count), send/receive buffer size and the hop limit
  on `tcp-socket`, and unicast hop limit + buffer sizes on `udp-socket`. There is no
  interface for Nagle, linger, IPv6-only, broadcast or group membership; there is
  nothing to call.
- **Web.** The backend's handles are a host `fetch` and a host `WebSocket`; there is
  no socket underneath to configure, and listening sockets are themselves
  unsupported (DESIGN §7.5). It inherits the `Backend` trait's `Unsupported`
  defaults and adds no code.
- **IPv4 membership by interface index off Linux.** BSD and Winsock name the
  interface by *address* in `ip_mreq`, not by index. Applying an index-keyed request
  to the default interface would be exactly the silent lie this API exists to
  prevent, so it is reported.

## Files

| File | What |
|---|---|
| `crates/turnloop/src/types.rs` | `SocketOption`, `SocketOptionKind`, `KeepAlive`, `MulticastGroup`, `AcceptDefaults`; `ListenOpts::accept_defaults` |
| `crates/turnloop/src/driver.rs` | `Loop::set_option` / `Loop::get_option`, handle validation |
| `crates/turnloop/src/backend/mod.rs` | `Backend::set_option` / `get_option` (default `Unsupported`) and the rules in the module docs |
| `crates/turnloop/src/backend/sockopt.rs` | new; epoll + kqueue implementation |
| `crates/turnloop/src/backend/unix.rs` | wiring; accept applies the listener's defaults; `socket_fd` rejects non-sockets |
| `crates/turnloop/src/backend/iocp/sockopt.rs` | new; Winsock implementation |
| `crates/turnloop/src/backend/iocp/mod.rs` | wiring; defaults applied after `SO_UPDATE_ACCEPT_CONTEXT` |
| `crates/turnloop/src/backend/wasi_p2/sockopt.rs`, `wasi_p3/sockopt.rs` | new; `wasi:sockets` implementation |
| `crates/turnloop/src/backend/wasi_p2.rs`, `wasi_p3.rs` | wiring; `nodelay` reported instead of dropped |
| `crates/turnloop/src/backend/web.rs` | doc note only; trait defaults already report `Unsupported` |
| `crates/turnloop-contract/src/sockopts.rs` | new; backend-generic contracts |
| `crates/turnloop-contract/tests/sockopts.rs` | new; native arm + the independent `getsockopt` probes |
| `crates/turnloop-contract/tests/wasi.rs`, `tests/web/web_contract.rs` | WASI and web arms |
| `crates/turnloop-contract/tests/allocations.rs` | steady-state allocation gate |
| `DESIGN.md` | §7.7 and a §7.6 matrix row |
| `docs/wasm.md` | WASI/web contract-family rows |

## Tests — and how each one proves its subject

Every contract assertion reads the value back **through the OS**, because
`get_option` is a `getsockopt`. Three tests go further and do not trust turnloop's
own getter at all.

**Independent OS probes** (`crates/turnloop-contract/tests/sockopts.rs`):

- `accepted_socket_options_are_visible_to_getsockopt` — finds the descriptor
  turnloop accepted *without asking turnloop for it*: it walks `/proc/self/fd`
  (Linux) or `/dev/fd` (macOS/BSD) and matches the one whose `getsockname` is the
  listener's address and whose `getpeername` is the client's, which the listener
  itself cannot satisfy. It then calls `libc::getsockopt` from the test process and
  asserts `TCP_NODELAY`, `SO_KEEPALIVE` and the idle time the *listener default*
  asked for, and that a later `set_option(NoDelay(false))`/`KeepAlive(None)` is
  visible there too.
- `adopted_socket_options_reach_the_shared_socket` — `dup`s a socket the test owns,
  adopts one reference through `Detached::from_fd` / `from_socket`, and asserts
  through `getsockopt` on the reference turnloop never saw. This is the portable
  probe and the one that runs on Windows. Its subject proof is carried by the two
  options no kernel rounds — `TCP_NODELAY` (false → true) and `IP_TTL` (its
  default → exactly 7) — because the buffer sizes cannot carry it: see the
  kernel-chosen note above. `SO_RCVBUF` is still checked, as a floor.
- `linger_zero_resets_the_connection` — the observable-behaviour test. Both arms
  run in one test: with `Linger(Some(ZERO))` the peer's pending read completes with
  `ConnectionReset`; without it the same close delivers `Eof`. The only difference
  between the arms is the option, so neither verdict is vacuous.
- `multicast_membership_is_tracked` — membership has no getter, so the proof is the
  kernel's own bookkeeping: leaving a group that was never joined **must fail**,
  leaving the group that was joined must succeed, and leaving it a second time must
  fail again. A backend that dropped the join cannot produce that sequence.

**Coverage by endpoint**: connected client, **accepted connection**, listener
(through `accept_defaults` and `ipv4_membership_by_interface_index`), UDP socket,
adopted/attached socket, a listener moved between loops
(`accept_defaults_survive_transfer` — the defaults live on the transport, so they
travel through `detach`/`attach` and still configure connections on the new loop),
unconnected TCP socket (where address-family detection for IP-level options is
non-obvious), and non-socket handles.

**`Unsupported` paths**: `socket_options_without_a_wasi_interface_are_unsupported`
(8 options + 6 getter kinds on WASI), `socket_options_are_unsupported_on_a_host_stream`
(10 options + 10 getter kinds on a live web WebSocket handle),
`non_socket_handles_report_unsupported` (stderr),
`local_listener_refuses_tcp_accept_defaults`,
`nodelay_accept_default_rejects_the_listener` and `nodelay_connect_hint_is_rejected`
(WASI), and `ipv4_membership_by_interface_index` (non-Linux).

**Allocation gate**: `steady_socket_options_allocate_nothing` in
`tests/allocations.rs` runs 100 measured iterations of
`set_option`×3 + `get_option`×2 on UDP and 100 more of the three-syscall keep-alive
path on a connected socket, asserting `0` allocations *and* that the OS kept each
value — a gate that cannot pass having done nothing. It runs on native and WASI.

**Sabotage checks (not committed).** With `apply_accept_defaults` neutered and
`set_linger` turned into `Ok(())`, exactly the three tests that should fail did:
`accepted_socket_options_are_visible_to_getsockopt`,
`nodelay_round_trip_and_accept_default` and `linger_zero_resets_the_connection`.
The other tests stayed green, which is the right blast radius. After the buffer
assertions were relaxed to a floor, both relaxed sites were re-sabotaged: stubbing
`Ttl` fails the adopted probe, and stubbing the two buffer setters fails
`buffer_sizes_round_trip` on its growth assertion. Neither relaxation made a test
unable to fail.

## Verification

macOS (kqueue) is this machine, `aarch64-apple-darwin`. Linux (epoll) is
`x86_64-unknown-linux-gnu` on the shared build box, clone `/root/claude-sockopts`
with an `OWNER` file naming this lane.

| Command | Result |
| --- | --- |
| `cargo +nightly-2026-08-20 fmt --all --check` | PASS |
| `python3 scripts/ci/check-paths.py` | PASS — 1501 tracked files, 264 references |
| `python3 scripts/ci/feature_modes.py` | PASS — 6 public features, 18 required arms |
| `cargo +nightly-2026-08-20 clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS (macOS) |
| same, `--target x86_64-unknown-linux-gnu`, `-p turnloop -p turnloop-contract -p turnloop-io` | PASS |
| same, `--target x86_64-pc-windows-msvc` | PASS |
| same, `--target wasm32-wasip2 --all-features` | PASS |
| same, `--target wasm32-unknown-unknown --all-features` | PASS |
| `cargo +nightly-2026-09-07 clippy … --target wasm32-wasip3 --all-features` | PASS |
| `RUSTDOCFLAGS='-D warnings' cargo +nightly-2026-08-20 doc -p turnloop --all-features --no-deps` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test --workspace --no-fail-fast -- --test-threads=1` (macOS) | PASS — 65 test binaries ok, 0 failed |
| `cargo test -p turnloop-contract --test sockopts -- --test-threads=1` (macOS) | PASS — 17/17 |
| `cargo test -p turnloop-contract --test sockopts -- --test-threads=1` (Linux, epoll) | PASS — 17/17 |
| Linux six required modes: `default`, `epoll-timerfd`, `process-sigchld`, `fallbacks`, `executor`, `all-features`, each `cargo +nightly-2026-08-20 test --locked --workspace --no-fail-fast … -- --test-threads=1 --skip permission_denied_is_reported` | PASS — all six modes, 406 `test result: ok` lines, 0 FAILED; the `getsockopt`, multicast, transfer and allocation tests each ran once per mode |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` (Wasmtime 46.0.0) | PASS — 42 contract + 13 allocation tests |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` (nightly-2026-09-07) | PASS — 42 contract + 14 allocation tests |
| `python3 scripts/ci/run-tests.py node` (web backend under Node 26.5.1) | PASS — 16/16, including `socket_options_are_unsupported_on_a_host_stream` |
| `bash scripts/ci/no-tokio.sh` | PASS — 5 graphs, zero runtime crates |
| `python3 scripts/ci/soak.py` | PASS — 251 locked versions, 1 pre-existing rustls exception |
| `cargo +nightly-2026-08-20 deny --locked check` | PASS — advisories, bans, licenses, sources |
| `python3 scripts/ci/lint-workflows.py` (actionlint + zizmor + shellcheck) | PASS — no workflow files were changed |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py'` | PASS — 98 tests |
| `python3 scripts/ci/run-tests.py web` (headless Chromium/Firefox) | PASS **in CI** (run 34991952873); UNRUN locally, no browsers on this machine |
| Windows `cargo test` | PASS **in CI** on all three `windows-2025` modes (run 34991952873) |

### CI

| Run | SHA | Result |
| --- | --- | --- |
| [34990033482](https://github.com/PerryTS/turnloop/actions/runs/34990033482) | `7d97b2a` | FAIL — every job green except the three `windows-2025` arms, each failing only `adopted_socket_options_reach_the_shared_socket` (15/16), all with `the OS reports 131072 bytes for a 49152-byte request` |
| [34991952873](https://github.com/PerryTS/turnloop/actions/runs/34991952873) | `b87bae6` | **PASS — every job, `ci-gate` green.** Zero `test result: FAILED` lines in the whole log; the previously failing probe now passes six times on `windows-2025` (three modes × workspace and per-member runs) |

That first run is the evidence for the buffer-size decision above: it is also the
first execution of `backend/iocp/sockopt.rs` anywhere, and everything else in it
passed on Windows first time — the `LINGER` layout, the Win10-1709 keep-alive
schedule, the accept default applied after `SO_UPDATE_ACCEPT_CONTEXT`, and the
`linger 0` → `ConnectionReset` behavioural test.

### Pre-existing failures seen on the build box (not this lane)

- `turnloop-contract --test filesystem permission_denied_is_reported` fails there
  because the box runs as **root**, and root ignores a read-only file's mode. It
  fails identically on the base commit `09d205f`, which was checked before skipping
  it in the mode matrix. CI runs unprivileged.
- `cargo check -p turnloop --target aarch64-linux-android` fails on the base commit
  too (two errors in the inotify watch code). Android is not in the CI matrix.

## What still needs CI

Nothing. Every platform arm has executed: Linux x86_64 and arm64 (six modes each),
macOS, Windows (three modes), WASI 0.2 and 0.3, and the headless-browser web arm.

## Follow-ups this lane deliberately did not take

- `turnloop-http`'s server still configures nothing per connection. Now that
  `ListenOpts::accept_defaults` exists, `nodelay: true` is the obvious default for
  an HTTP/1.1 and HTTP/2 server; that is a protocol-crate decision, not a core one.
- `TcpOpts` gained no new fields. Anything a host wants to change after connect
  goes through `set_option`, so the connect-time struct stays minimal.
- No `SocketOption` variant was added for `SO_REUSEADDR`, `SO_REUSEPORT`,
  `IP_MULTICAST_IF` or `SO_BINDTODEVICE`: the first two are bind-time (already in
  the opts structs) and the last two have no portable shape worth guessing at
  before a host asks for them.
