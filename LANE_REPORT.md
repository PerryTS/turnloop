# proto-mongo lane report

Status: implementation and verification in progress. No commits (read-only `.git`).
Crate: `protocols/turnloop-mongodb`; root workspace created. No changes to sibling lanes.

## Surface and current status

| Surface | Status |
| --- | --- |
| URI, IPv6, escaping, core options, explicit SRV/TXT | Implemented, validation in progress; unsupported options reject |
| OP_MSG, sections, CRC32C, bounded incremental framing | Implemented; raw borrowed replies and reusable encoder |
| Legacy hello/hello, metadata, mechanism negotiation, speculative SCRAM | Implemented; real-server verification pending |
| SCRAM-SHA-1 / SHA-256 | Implemented; host nonce, SASLprep SHA-256, Mongo MD5 SHA-1 |
| CRUD / find / aggregate / counting / distinct / findAndModify / indexes / listings / runCommand | Raw command builders; wire option names; results and cursors |
| SDAM single/replica set/sharded, RTT, heartbeat deadlines, read preference | Implemented; official JSON runner being built |
| CMAP checkout/checkin, sizing, timeouts, clear | Implemented; host executes connect/close actions |
| Retryable reads and writes | Policy component; orchestration and implicit sessions still being verified |
| Sessions/transactions | Basic explicit decoration and state transitions; full spec conformance pending |
| Compression | zlib only; no snappy/zstd |
| Change streams | Resume state helper; full spec conformance pending |
| Perry bindings / turnloop adapter | Deferred as requested; Perry files read-only |

## References

Read DESIGN.md §§1–5b and 13 and LANES.md. Inspected both Perry MongoDB bindings.
Perry exposes JSON document/array strings, inserted-ID string for insertOne, numeric insertMany/update/delete/count results, null findOne, string rejections prefixed by operation, and constructor/connect/db/collection/list/close. Streaming, aggregation, and change streams are beyond its current binding surface.

Official specs snapshot: `9ecb35b1944ade6f7e711316f83c360bf045edfb` from https://github.com/mongodb/specifications . JSON fixtures are unmodified under tests/spec with upstream license. Source sections are cited in modules. Official Node API: https://mongodb.github.io/node-mongodb-native/ (no claim of full API parity).

## Verification

- `cargo check`: in-progress build failures during scaffolding; resolving API integration errors.
- Real MongoDB standalone, replica set, auth, TLS: UNRUN so far.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, stable build, WASI check: UNRUN so far.

## Allocation profile

Raw BSON command builder, OP_MSG encoder/decoder, borrowed rows and pool vectors retain capacity. Handshake/SCRAM, URI/DNS, topology updates and owned errors allocate. Document convenience conversions allocate inherently owned BSON. No allocation claim is yet measurement-verified. BSON dependency includes randomness/clock APIs; protocol never calls them. Host supplies cryptographic nonce, UUID, ObjectIds and `Instant`.

## Dependencies and supply chain

7-day global-min-publish-age configuration unchanged; lock resolution explicitly applied the soak.
- bson 3.1.0: runtime-agnostic BSON/raw views; defaults off; compat-3-0-0 required by BSON, serde and serde_json-1 support Perry JSON and spec Extended JSON.
- base64, sha1, sha2, hmac, pbkdf2, md-5, stringprep: Mongo SCRAM.
- flate2, pure Rust backend: OP_COMPRESSED zlib, no runtime.
- dev serde_json: official JSON fixtures. dev rustls (ring/std/tls12) and rustls-pemfile: TLS over std sockets, no async runtime.

## Deviations / proposed design clarifications

Define protocol performance budgets separately for initial connection/discovery versus warmed command/row paths. Carry host entropy and timestamps explicitly; do not generate BSON ObjectIds in the protocol. Basic transaction policy is not a claim of complete transactions specification support. Current spec snapshot adds newer backpressure requirements; these need independent implementation, not advertising unsupported capabilities.

## Open questions / next steps

Complete compile integration, real-server harness, official suites, failure tests, allocation measurement, stable/WASI/clippy checks. Record remaining gaps precisely after verification. Private servers must be stopped by scripts and checked after runs.
