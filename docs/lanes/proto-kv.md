# proto-kv lane report

2026-09-14. Both protocol crates and their blocking test drivers are implemented.
Private Redis 8.4, three-master Cluster, Sentinel, rustls TLS, scripted SMTP and
installed Postfix smtp-sink tests pass. Final nightly/stable, lint, allocation, WASI and web compilation checks pass.
All 25 tests passed in the full private-server run; all private servers stopped.
No Perry or core-loop files changed. The integrator owns commits; this lane has
not committed/pushed/published. Read DESIGN.md §§1–5b and §13 and LANES.md fully.

## Implemented surface versus Node

| Surface | Implementation and boundary |
|---|---|
| Redis HELLO 3 / RESP2 fallback | HELLO with ACL auth; fallback only for unknown HELLO/NOPROTO; password-only or username/password AUTH; SELECT and CLIENT SETNAME; real ACL/TLS tests |
| RESP2/RESP3 replies | Bounded binary-safe codec: strings/errors/integers/null/arrays/maps/sets/pushes/booleans/doubles/big numbers/verbatim/attributes. Streaming lengths explicitly unsupported |
| Arbitrary Redis commands and pipeline | Binary argument slices, reusable encoding/replay storage, ordered token replies; all Perry commands available through this entry point |
| MULTI/EXEC/DISCARD/WATCH | Ordinary ordered commands; real commit/discard/WATCH-conflict tests. Host creates ioredis `[error,result]` arrays and pins Cluster transaction slot |
| Pub/sub | SUBSCRIBE/PSUBSCRIBE and both unsubscribe forms; multi-ack ordering, binary message/pmessage routing, ioredis subscriber restrictions, reconnect restoration; RESP2/3 tests |
| Blocking commands/timeouts | Real BLPOP timeout and cross-connection unblock; caller-supplied absolute deadlines via next_timeout/handle_timeout; timed-out sent commands retain ordering tombstones |
| Reconnect/offline queue | Explicit retry decision/wait/connect states; re-auth/select/name/subscribe before replay; max-retry queue failures; queue/resubscribe/replay options. Host evaluates retryStrategy; no RNG or clock in core |
| Error shapes | Redis ReplyError preserves wire message. Other local errors retain Error name; JS command/args/previousErrors metadata and exact maxRetries text remain adapter work |
| Cluster | SLOTS/SHARDS maps; hash tags/CRC16; MOVED map update and ASK stable-map preservation; bounded redirect tracker; common multi-key layouts with CROSSSLOT validation; unknown layouts require explicit keys. Real three-master MOVED and migrating/importing ASK tests |
| Sentinel | Seed iteration, get-master-addr-by-name, candidate ROLE validation, retry actions/deadlines. Real Sentinel discovery + ROLE + PING; stale role and seed retry tests |
| SMTP greeting/EHLO/HELO | Fragmented/multiline response parsing, capabilities, fallback, greeting/socket deadlines, protocol limits |
| SMTP TLS | Opportunistic/required STARTTLS, re-EHLO, implicit TLS; explicit UpgradeTls transition and host completion; rustls test CA trust, no verification bypass |
| SMTP auth | PLAIN, LOGIN, XOAUTH2; mechanism-specific errors, including server response in message; auth success/failure socket tests. CRAM-MD5 optional and not implemented |
| SMTP PIPELINING/envelope | MAIL plus RCPT pipeline, drains failure responses; SIZE/8BITMIME/SMTPUTF8 checks; partial recipients/all rejected; DATA/dot-stuffing/CRLF normalization; RSET/QUIT |
| SMTP result/error fields | response, responseCode, command, code, accepted, rejected records, envelope, messageId; failed events retain envelope/recipient outcomes. Host maps rejected records to rejected/rejectedErrors JS arrays |
| MIME builder | lettre builder only: text/html alternatives, attachments, cc/bcc, custom headers, transfer encodings. Explicit Date, Message-ID and boundary seed avoid hidden clock/RNG defaults |
| nodemailer verify | Ready after greeting/TLS/auth. No test delivery attempted, matching verify's transport purpose |

Perry evidence: `perry-ext-ioredis/src/lib.rs`, `perry-stdlib/src/ioredis.rs` and
`perry-ext-nodemailer/src/lib.rs` in the supplied read-only worktree. Redis exposes
SET/SETEX/GET/DEL/EXISTS/INCR/DECR/EXPIRE/PING/HGET/HSET/HGETALL/HDEL/HLEN and lifecycle.
The constructor ignores config and uses REDIS_* env vars today; host adaptation
must fix that separately. SMTP exposes host/port/secure/user/pass and
from/to/subject/text/html, sendMail/verify. No list convenience binding was found.

## JS conversions and adapter contract

Each crate README documents the pull API and conversion boundary. Bulk bytes
become a Buffer or UTF-8 replacement-decoded string; integers become JS numbers
(with the usual >2^53 precision issue); nil becomes null; arrays preserve order.
RESP3 maps/sets remain explicit typed values for host conversion. HGETALL becomes
an object from RESP2 pairs or RESP3 map entries. Perry currently converts
EXISTS/EXPIRE to boolean, while ioredis exposes numeric replies. No GC values,
callbacks, sockets, threads, clocks or timers are owned by either protocol core.

Hosts flush output with partial-write acknowledgements, feed decrypted input,
drain events, and supply deadlines/time. An accepted command/send token gets one
terminal result; timeout/close prevents duplicate completion. Replaying a Redis
write after its reply was lost can execute it twice, as with ioredis. In-flight
blocking timeout does not pretend to cancel server execution: its wire slot stays
reserved until the late response or transport close. Close the transport when a
stalled blocking command must be abandoned.

Cluster connection maps/pooling, logical request retention across redirects,
transaction connection affinity, topology-refresh scheduling and Sentinel
transport/auth options belong to the host. These are routing/discovery primitives,
not a completed drop-in ioredis Cluster object. Sentinel replica selection,
ongoing failover notifications, NAT mapping, read scaling and cluster pub/sub
coordination are not implemented. Auto-pipeline scheduling, ready INFO checks,
key prefixing, custom transformers/scripts, URL/env parsing and JS callback APIs
also remain in the later adapter. Redis streamed RESP3 is explicitly rejected;
Redis 8.4 command replies tested here do not use it.

SMTP lacks streaming MIME inputs (message bytes are buffered), DSN/LMTP, DKIM,
transport plugins, service/proxy presets, pooling and OAuth acquisition/refresh.
Exact nodemailer error text for every unusual server reply is not claimed. The
primary auth/envelope/DATA error shapes are covered. Perry's old generic lettre
error prefixes differ from nodemailer; adapter must choose intended JS behavior.

## Allocation profile and dependencies

Production code is safe Rust. Both libs use `#![deny(unsafe_op_in_unsafe_fn)]`.
The only unsafe code is forwarding to System in test counting allocators.

| Path | Measured allocation profile |
|---|---|
| 1,000 warmed Redis INCR encodes/replies | 0 allocations; command/replay/TX/RX/event queues reused |
| Incomplete RESP aggregate fragments | 0 allocations; bounded validation before materialization |
| Array of two bulk strings | 3 allocations: owned array and its two strings |
| Fragmented Redis pub/sub message | 2 allocations: returned channel and payload; borrowed fast path avoids temporary tag/array allocation |
| Warmed SMTP successful send | 3 allocations: returned accepted vector, recipient string, response string; caller envelope/ID ownership moved |

Capacity grows at high-water marks; subscription changes and discovery/topology
updates allocate retained state. MIME building allocates owned headers/parts/body
and lettre's formatting intermediates; its allocation is outside the warmed
transport path and has not been optimized or claimed zero-allocation. No closures
or task nodes are allocated per operation. Owned reply errors/aggregate results
naturally allocate. Codec max defaults: 16 MiB/frame, 1M values, depth 64; SMTP
responses 64 KiB. Large fragmented RESP frames rescan incomplete data and have a
quadratic worst case; an incremental parser is a follow-up performance task.

Dependencies (resolved with existing seven-day soak; no override):
- Redis production crate: **none**. redis-protocol 6.0.0 `codec` feature enables
  tokio-util. Its standalone decoders are sans-I/O, but use nom count/Vec range
  trees and temporary map containers. The hand-written bounded validator and
  materializer avoids these temporary/fragment allocations and runtime features.
- SMTP: **lettre 0.11.23**, default features disabled, **only builder**; **base64
  0.22.1** for SASL. lettre pulls a separate base64 0.23.1 via email-encoding.
  Its transitive email_address/idna/ICU, MIME/encoding, httpdate and fastrand
  dependencies support message building, not an async transport.
- Dev only: **rustls 0.23.44**, std/ring/tls12; ring/webpki for real blocking TLS.
  Test CA/leaf and public private key fixtures generated with installed OpenSSL.
- `scripts/check-dependencies.py` verifies all 68 resolved packages contain no
  tokio/tokio-util/async-std/smol/async-io/async-executor/futures-executor and
  asserts lettre has exactly `builder`. Cargo.lock records the soaked graph.

## Verification

Host: macOS arm64; nightly-2026-08-20 and stable Rust 1.97.1.

| Command | Result |
|---|---|
| `python3 scripts/servers.py test` | PASS final run: all 25 tests, zero ignored; includes private Redis/TLS/Cluster/Sentinel and Postfix |
| `cargo test --workspace --test allocation` | PASS; Redis scalar/aggregate and SMTP representation budgets |
| `cargo test -p turnloop-redis --test allocation` | PASS after pub/sub fast path: 2 allocation tests |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS final tree |
| `cargo +stable test --workspace --locked` | PASS final tree; 22 passed, 3 external tests explicitly ignored in this invocation (all ran in full nightly suite) |
| `cargo check --workspace --target wasm32-wasip2 --locked` | PASS final tree, both cores (no TLS dev dependencies built) |
| `cargo check --workspace --target wasm32-unknown-unknown --locked` | PASS final tree, both cores |
| `cargo fmt --all -- --check` | PASS final tree |
| `python3 scripts/check-dependencies.py` | PASS, 68 packages and lettre builder-only |
| `cargo tree --workspace -e features` | PASS; inspected graph, saved `.tools/dependency-tree.txt` |
| `cargo tree --workspace -i tokio` | Expected exit 101: package ID tokio does not match any package (absence); positive audit script above passes |
| `cargo test -p turnloop-smtp --test smtp installed_postfix -- --ignored` | PASS installed Postfix 3.2.2 smtp-sink, saved subject/body bytes asserted |

Initial failures (fixed, not weakened):
- Real tests invoked against a separately launched server session: ConnectionRefused
  because that process session ended. Lifecycle now supports detached child starts
  and runs tests with servers in the same parent command; successful full run above.
- First stop script attempted sandbox-unavailable `ps`: Operation not permitted.
  It now checks Redis INFO's exact private config_file before SHUTDOWN NOSAVE.
- SMTP SIZE test incorrectly expected 32 instead of 31 normalized bytes; changed
  to compare the length of exact expected normalized content, preserving byte test.
- Clippy manual_range_patterns: changed 250|251|252 to 250..=252.
- Stronger auth error-message test exposed missing server-response suffix; fixed
  implementation to append response and use mechanism-specific command.

UNRUN: Linux/Windows execution, WASI/web execution under runtime/browser, Miri,
long-running/fuzz/benchmark soaks, real Sentinel failover and replica selection.
Commands for future CI: `cargo test --workspace` on each native platform;
`cargo +nightly miri test -p turnloop-redis --test protocol`; WASI/browser runners
require test-driver adaptation. No claim of end-to-end Perry compatibility gates.

## Private server lifecycle

`scripts/servers.py start|stop|test`: .tools data/config/logs, high random loopback
ports, single Redis with ACL and TLS, three-master cluster (cluster-cli create),
one Sentinel monitoring the private master. Refuses ambiguous state and checks
config identity before stopping. `test` stops in finally on failure/interrupt.
Postfix test owns a child RAII guard and also stops it on failure. No default-port
or system service changes. Final cleanup confirmed: `.tools/redis/instances.json` is absent and the recorded
private Redis TCP ports refuse connections. Postfix child is waited after kill.

## Deviations / proposed DESIGN.md clarifications

1. Distinguish exactly-once *completion* from at-least-once Redis replay execution.
2. Protocol cores expose connection/TLS/discovery transitions without depending
   on the executor; adapters can stay thin once the executor lands.
3. Specify maximum frame/response/message budgets and the treatment of streamed
   RESP3. Current implementation rejects streamed RESP3, rather than panicking or
   allowing unbounded input; MIME body buffering remains an explicit limitation.
4. Node compatibility belongs partly in host conversion: JS HGETALL objects,
   numeric precision, pipeline/EXEC result aggregation and transport error wrapping.

## Open questions and next steps

- Confirm which ioredis major/version Perry intends to match. Current docs are v6;
  retryStrategy defaults changed since v5, so core requests host policy explicitly.
- Integrate token/event API with turnloop executor and Perry GC/promise ownership.
- Add JS conformance tests for conversions/error metadata, choose host config and
  timeout defaults (Perry uses 10s today), and implement connection pooling/topology
  policies above before claiming complete Node library parity.
- Add native Linux/Windows and runtime WASI/web CI, incremental RESP parsing,
  streamed RESP3 if required, streaming MIME, and sustained reconnect/failover soaks.

## Source references

- [ioredis documentation](https://github.com/redis/ioredis): binary replies,
  pipelines, transactions, pub/sub, reconnection and Cluster/Sentinel behavior.
- [RESP specification](https://redis.io/docs/latest/develop/reference/protocol-spec/).
- [Nodemailer SMTP](https://nodemailer.com/smtp),
  [envelope](https://nodemailer.com/smtp/envelope),
  [OAuth2](https://nodemailer.com/smtp/oauth2).
- [Nodemailer v7.0.6 SMTP source](https://github.com/nodemailer/nodemailer/blob/v7.0.6/lib/smtp-connection/index.js):
  `_formatError`, auth, MAIL/RCPT/DATA error shapes and response suffix.
- [redis-protocol 6.0.0](https://docs.rs/redis-protocol/6.0.0/redis_protocol/) and
  downloaded source `resp3/decode.rs` (nom count, Vec range frames, map temporaries).
- [lettre 0.11.23](https://docs.rs/lettre/0.11.23/lettre/) and downloaded message
  builder source: supplied Date prevents date_now, explicit ContentType boundaries
  avoid multipart random generation.
- [Postfix smtp-sink source](https://github.com/vdukhovni/postfix/blob/master/postfix/src/smtpstone/smtp-sink.c):
  private listener and message dump options used by the installed binary test.

## Integrator verification

- 2026-09-14, integrator, outside the Codex sandbox (macOS arm64): `python3 scripts/servers.py test` — **PASS, 25 tests** against private Redis 8.4 (single, 3-master cluster, sentinel, TLS) and Postfix smtp-sink. `cargo tree -i tokio` is empty for x86_64-unknown-linux-gnu, wasm32-wasip2 and aarch64-apple-darwin.
