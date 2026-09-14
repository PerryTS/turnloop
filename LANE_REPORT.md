# proto-kv lane report

Status: implementation in progress. No verification passes claimed yet.

Read DESIGN.md §§1–5b and §13, LANES.md, perry-ext-ioredis/src/lib.rs,
perry-stdlib/src/ioredis.rs, and perry-ext-nodemailer/src/lib.rs in the supplied
read-only Perry worktree. No files outside lane ownership will be changed.

## Surface and design

| Surface | Status |
|---|---|
| Redis RESP2/3, handshake, arbitrary commands, pipeline, transactions | In progress |
| Redis pub/sub, deadlines, reconnect, offline queue | Planned |
| Redis Cluster and Sentinel | Planned |
| SMTP greeting, TLS, auth, envelope, DATA, MIME | Planned |
| Blocking real-server and TLS integration drivers | Planned |

Perry exposes Redis SET/SETEX/GET/DEL/EXISTS/INCR/DECR/EXPIRE/PING,
HGET/HSET/HGETALL/HDEL/HLEN, connect/disconnect/quit. Its constructor currently
ignores per-instance config and reads environment variables; those belong in the
host adapter, not the protocol. SMTP binding exposes host/port/secure/auth and
from/to/subject/text/html, sendMail and verify. Full Node APIs extend beyond these.

## Dependencies and allocation plan

- Hand-written RESP codec: redis-protocol 6.0.0's feature named `codec` pulls
  tokio-util. Its standalone parsing features do avoid runtimes, but range-frame
  parsing builds temporary aggregate containers, including on incomplete input.
  This lane instead validates complete bounded frames without allocation and
  materializes only the owned result, avoiding temporary aggregate allocation.
  Command serialization and replay records use reusable buffers.
- lettre: defaults disabled, only `builder`; MIME representation allocation is
  inherent to constructing messages. Dates/IDs must be supplied by the caller;
  protocol state machines never read clocks.
- base64: runtime-agnostic SMTP SASL encoding.
- rustls: dev-only, std/ring/tls12; blocking socket TLS tests, no runtime.
- The existing seven-day registry soak remains enabled. Cargo.lock will be kept.

## Verification

All requested build, lint and tests currently UNRUN pending implementation.

## References

- https://github.com/redis/ioredis (reply conversions, pipeline, subscriptions,
  reconnection; current main is v6, explicit retry decisions remain host policy).
- https://nodemailer.com/smtp and https://nodemailer.com/smtp/envelope
- https://nodemailer.com/smtp/oauth2
- https://redis.io/docs/latest/develop/reference/protocol-spec/
- https://docs.rs/redis-protocol/6.0.0/redis_protocol/
- https://docs.rs/lettre/0.11.23/lettre/

## Deviations / proposed DESIGN.md changes

Specify that protocol request IDs have exactly-once terminal events, while
replaying an unacknowledged write after connection loss has at-least-once server
execution semantics (as in ioredis). Transport ownership remains entirely host-side.

## Open questions / next steps

Implement both crates, private-instance lifecycle scripts, real Redis/cluster/
Sentinel integration and scripted SMTP/TLS tests; run nightly/stable/lint/wasm
checks and allocation regression tests. Integration into Perry/turnloop is later-wave.
