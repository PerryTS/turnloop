# proto-mongo — turnloop-mongodb

Implemented protocol components and integration harness, ready for integrator review.
This is a substantial stage-1 implementation with tested stage-2 functionality, **not a
claim of complete Node driver or MongoDB specification conformance**. Remaining gaps
are enumerated below. All source belongs to this clone; sibling lanes and Perry were
read only. No commits or publishing were performed by this agent; the integrator may
have committed intermediate snapshots.

Crate: `protocols/turnloop-mongodb`. Root Cargo workspace and locked dependencies are
present. API and adapter instructions: `protocols/turnloop-mongodb/README.md`.

## Design and behavioral sources

Read DESIGN.md §§1–5b and 13 completely, plus LANES.md. Inspected
`perry-ext-mongodb/src/lib.rs` and `perry-stdlib/src/mongodb.rs` in the supplied Perry
worktree. Their current exposed surface is constructor/connect/db/collection/close,
find/findOne, insertOne/Many, updateOne/Many, deleteOne/Many, countDocuments,
listDatabases and listCollections. Perry returns JSON document/array strings, an
inserted-ID string for insertOne, numeric counts for the other writes, null for a
missing findOne, and operation-prefixed string rejections. JS conversion, handles and
promise routing remain Perry's responsibility; these bindings have not been edited.

Behavioral references:
- [Official Node driver API](https://mongodb.github.io/node-mongodb-native/)
- [MongoDB specifications](https://github.com/mongodb/specifications/tree/9ecb35b1944ade6f7e711316f83c360bf045edfb/source)
- [SDAM](https://specifications.readthedocs.io/en/latest/server-discovery-and-monitoring/server-discovery-and-monitoring/)

Spec snapshot: **9ecb35b1944ade6f7e711316f83c360bf045edfb**. All 287 vendored JSON
fixtures are unmodified, under `tests/spec/`, with COMMIT and the upstream license.
Modules cite the applicable source documents and sections. No official MongoDB driver
is a dependency; replica-set initiation, user creation, failpoints and CRUD run through
this crate, without a Mongo shell.

## Implemented surface against the Node driver

| Surface | Implementation and evidence | Remaining differences |
| --- | --- | --- |
| URI/options | mongodb:// seed lists, bracketed IPv6, escaping, authSource/mechanism, TLS/SSL, directConnection, replicaSet, appName, timeout/pool/retry/read preference/tag/staleness/concern options | Unsupported options reject; not full Node connection-string parser parity. Unix sockets, proxies, TLS file/insecure flags, loadBalanced, srvMaxHosts, custom SRV service and zstd/snappy options absent |
| mongodb+srv | Explicit SRV and TXT requests; TLS default; parent-domain validation; TXT authSource/replicaSet; explicit URI overrides TXT | Host performs DNS and TTL refresh; no autonomous SRV polling or load-balancer TXT support |
| Wire | OP_MSG sections 0/1, fragmented input, bounded frames/BSON nesting, correlation, CRC32C generation/validation, unknown required flags/section rejection, borrowed sequences | No OP_QUERY/OP_REPLY compatibility, streaming/exhaust replies rejected; checksum tested by codec fixtures, not sent in real-server tests |
| Handshake/TLS | OP_MSG legacy isMaster with helloOk, client/app metadata, wire/size limits, SASL mechanisms, compression negotiation; explicit UpgradeTls event before Mongo bytes | No Stable API/loadBalanced handshake mode, full client metadata environment detection, or new backpressure-v2 support; those capabilities are not advertised |
| SCRAM | SHA-1 Mongo MD5 password digest; SHA-256 SASLprep; speculative auth; default mechanism negotiation; server proof and conversation-id validation; host nonce | No other authentication mechanisms, credential cache or reauthentication; iteration count limited to 4096..1,000,000, unlike Node's unbounded upper range |
| Inserts/updates/deletes | Raw command/sequence and write-model builders, upsert/multi options, ordered/unordered server results, write error/concern details, BulkBatcher size/count splitting, original error indices, unacknowledged send completion | Generic bulk accumulator exposes wire counts/errors. A Node-shaped mixed-operation bulkWrite facade and inserted/upserted-ID maps remain host work |
| ObjectId/types | Full bson re-export; raw BSON preserves types; deterministic ObjectIdGenerator takes host timestamp/entropy and wraps 24-bit counter | Caller adds missing _id before sending and retains inserted IDs; no JS ObjectId class or automatic input-document mutation |
| Find/cursors | find/findOne, getMore, killCursors, batchSize/limit, borrowed rows, namespace/server affinity and client-limit tracking | Host schedules cursor operations and kill on limit/close; negative limit normalization and high-level tailable/exhaust lifecycle absent |
| Read/modify/admin helpers | aggregate, countDocuments aggregation, estimated count, distinct, findOneAndUpdate/Replace/Delete via findAndModify, create/list/drop indexes, database/collection lists, runCommand | Options are wire names; no complete Node method/options/result facade. Arbitrary command options can be passed as raw BSON |
| Concern inheritance | Command::apply_client_options applies URI write/read concerns and read preference with tags/staleness; explicit command fields win | Database/collection inheritance is adapter-level; Connection does not rewrite arbitrary user commands |
| Error shapes | Server code, codeName, errmsg, labels, raw result/errInfo, bulk errors, pool/selection/network/operation timeout names | Exact server text preserved. Locally generated validation/error text is not asserted identical to every Node version; Perry operation prefixes remain at its FFI boundary |
| SDAM | Single, unknown, replica set with/without primary, sharded; primary hints; discovery/removal, setName/me, stale elections/topologyVersion/generation handling, application errors, heartbeat requests/deadlines, RTT EWMA, session timeout | Polling monitors only. Full monitoring-event equality/suppression, minimum-RTT sampling and all prose/unified suites remain |
| Selection | Read modes, tag sets, max staleness, latency window, deprioritized servers, power-of-two choice with host entropy and operation counts | Host maintains counts and drives selection waiting; no logging/APM listener facade |
| CMAP | Paused/ready/closed states; min/max/maxConnecting; FIFO checkout/checkin; wait/idle deadlines; generations; clear; stale checked-out connections close on return | Component tested directly; complete official CMAP JSON event/logging runner, maintenance throttling and all cancellation races not covered |
| Retry reads/writes | Operation coordinator: select/checkout/send/result/reselect; one retry; same lsid/txnNumber; fresh wire request id; NoWritesPerformed fallback; excludes multi updates/deletes, getMore, transactions, $out/$merge and generic runCommand | Host acquires session identities and applies topology/pool actions; no CSOT multi-retry/backpressure extension or complete official retry suites |
| Sessions/transactions | lsid/txnNumber; session identity reuse/expiry; dirty exclusion; cluster-time gossip and causal read decoration; start/commit/abort; bounded commit/abort retry decisions; majority commit retry preserving wtimeout; recovery tokens and explicit mongos pin/unpin | Snapshot sessions, endSessions batching, withTransaction replay controller, full transaction option validation and automatic cross-component pin enforcement remain |
| Compression | Negotiated zlib OP_COMPRESSED, bounded expansion, secret-bearing commands excluded, persistent compressor/decompressor | zstd, snappy and noop envelopes absent |
| Change streams | $changeStream through aggregate; resume token/startAfter/resumeAfter state, post-batch tokens, operation time, resumable-error classification, one resume attempt per failed getMore | Helpers plus real resume test; full automatic stream lifecycle, all option combinations and unified change-stream suite remain |
| turnloop / Perry adapter | Token/byte/event/time/host-request API with operation coordinator and README dispatch contract | Adapter intentionally deferred until executor lands; no Perry binding changes |

## Verification results

Environment: macOS arm64; MongoDB **8.2.6**; pinned nightly-2026-08-20; stable
**rustc/cargo 1.97.1**. No async runtime crates in normal, dev or target dependencies.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --all-targets --locked -- -D warnings` | PASS |
| `cargo test --all-targets --locked` | PASS: 13 focused protocol tests, allocation gate, SDAM runner and 2 selection runners. Real-server and cleanup tests explicitly ignored here; real-server test run separately below |
| `cargo +stable test --all-targets --locked` | PASS, same pure suites on stable 1.97.1 |
| `cargo test --test spec_sdam -- --nocapture` | PASS: 177 official fixtures, 395 phases; single/replica/sharded/error directories |
| `cargo test --test spec_selection -- --nocapture` | PASS: 78 selection + 32 max-staleness fixtures |
| `cargo test --test allocations -- --nocapture` | PASS: zero allocations across 4 × 1,000 measured commands and 8,000 borrowed rows; direct and Operation-coordinated paths, each with and without zlib |
| `cargo check --target wasm32-wasip2 --locked` | PASS |
| `cargo check --target wasm32-unknown-unknown --locked` | PASS; browser host timestamps are constructible without std::Instant::now |
| `scripts/verify-mongodb.sh` | PASS: repeats the above build/test checks and verifies all 109 metadata packages contain no forbidden runtime dependencies |
| `python3 scripts/mongodb.py run` | PASS: private standalone, three-member replica set and TLS standalone, authentication and behavior below; script stops children and verifies all five private ports closed |
| `python3 scripts/mongodb.py stop` | PASS for cleanup of the initial orphaned run, using authenticated protocol shutdown and closed-port assertions |

Real test assertions include:
- SHA-1, SHA-256 and default speculative negotiation; authenticatedUsers length;
  wrong-password rejection. Users created through this driver, replicated to all members.
- Full CRUD helper round trips over both plain/zlib and certificate-validated rustls
  transports; BSON field values, counters and database/collection/index names checked.
- Duplicate-key ordered stop and unordered continuation, error code 11000, original
  bulk index, and persisted counts. Cursor batches forced to one row, getMore exhausted,
  killCursors confirms its server-side effect.
- Replica initiation from runCommand; discovery of all three members and two readable
  secondaries; forced primary stepdown, election of another member, SDAM re-selection,
  and persisted data read back from the new primary.
- Transaction commit and abort visibility; repeated retryable update with identical
  session/transaction identity changes a field only once.
- failCommand forces exactly two sends for one retryable read and one retryable write;
  failpoints also force commit and abort retries; final row counts verified.
- Change-stream insert event observed, token recorded, cursor reopened with resumeAfter,
  a later distinct document observed, both cursors killed. Causal majority read succeeds.

Non-server fixture coverage is deliberately enumerated, not presented as the complete
spec repo. Full SDAM monitoring/logging/load-balanced/unified, CMAP, URI, retry,
transactions and change-stream unified runners are **not implemented / UNRUN**. There
is no runnable harness command for those additional suites yet. OS execution beyond
macOS, WASI runtime execution, browser runtime execution, and real mongos deployment
are **UNRUN**; cross compilation does not claim those runs.

Local evidence: `.tools/verification-final.log`, `.tools/real-final.log`,
`.tools/protocol-final3.log`, `.tools/alloc-staleness.log`. Logs/data are gitignored.

### Failures encountered and resolved

- Initial `cargo check` failed during module scaffolding/BSON API integration; final
  check, tests and clippy pass.
- `cargo test --test spec_sdam -- --nocapture` initially failed 5 fixtures. Fixed runner
  explicit directConnection/multi-seed initialization, primary hints and timeout handling;
  upstream fixtures and expected outcomes were not changed.
- `cargo test --test spec_selection -- --nocapture` initially failed deprioritization,
  empty primary tag set and max-staleness topology cases; code/runner corrected without
  changing fixture expectations.
- Allocation gate initially found **8 allocations/command** from BSON typed getters on
  absent/mismatched optional fields. Replaced those with borrowed optional lookups;
  the gate remains strict at zero, including compression and coordinator paths.
- Initial private-server startup failed because MongoDB TLS needs an explicit trust
  chain. Added a CA file. Sandbox denied ps/signals from a later tool call; protocol
  cleanup then created users/initiated the private replica set and shut it down.
  An intermediate cleanup test failed to detect the legacy ismaster primary field;
  fixed detection, then asserted socket closure. The final script keeps Popen handles
  and stops children in finally, with authenticated protocol fallback for separate stop.
- An integration run initially failed authenticating a secondary before its user had
  replicated. User creation now uses w=3 on the replica set; the successful reruns
  require all three members to authenticate. No authentication assertion was removed.

## Allocation and ownership profile

The strict counting allocator gate measures encoder, incremental decoder, borrowed row
iteration, optional result parsing, command builder and Operation coordinator together:
zero allocations after two warm-up iterations, 1,000 commands / 2,000 rows per mode,
four modes (direct/coordinator × uncompressed/zlib). This includes test reply encoding.
Core source contains no unsafe. The test allocator uses documented forwarding unsafe
blocks solely to count allocations.

Connection receive/transmit/expanded/compression buffers, BSON writers and operation
wire templates retain capacity. A borrowed result is valid until release_reply; close
preserves an already received reply until release. Commands do not retain caller raw
pointers. Host completion writes must retain Connection storage until consumption.

Connection setup/handshake/SCRAM, URI/DNS, discovery/topology events and new pool
capacity allocate. Owned result/error documents, write-error collections, session
cluster/recovery state and change-stream resume tokens allocate as retained result
representations. Vec/VecDeque pool/session buffers retain capacity; those state machines
have functional tests but no separate global-allocation gate. Workloads exceeding
previous buffer/batch/concurrency capacity may allocate. Warmed arbitrary BSON types,
all commands and every optional path are not exhaustively allocation-profiled.

## Dependencies and supply chain

The existing `.cargo/config.toml` 7-day minimum publish age was **not changed**. Initial
nightly resolution explicitly selected versions "as of 7 days ago"; all following
verification uses Cargo.lock. No one-time age override or new release exception used.

| Direct dependency | Justification/features |
| --- | --- |
| bson =3.1.0 | BSON/raw borrowing; default features disabled. compat-3-0-0 is its required compatibility flag. serde + serde_json-1 permit Perry JSON/Extended JSON representations and fixture conversion. Its normal graph has no async runtime |
| base64 0.22 | SCRAM binary-to-text fields |
| sha1 0.10, sha2 0.10, hmac 0.12, pbkdf2 0.12 | Runtime-agnostic RustCrypto SCRAM primitives |
| md-5 0.10 | RustCrypto MD5 implementation for MongoDB's required SHA-1 password preprocessing |
| stringprep 0.1 | Mongo SCRAM-SHA-256 SASLprep |
| flate2 1, defaults off / rust_backend | Pure-Rust zlib protocol compression using resettable state; no native zlib/runtime requirement |
| dev serde_json 1 | Official JSON fixtures and private server manifest |
| dev rustls 0.23, defaults off / ring,std,tls12 | TLS over the blocking std socket in tests; no async runtime or default aws-lc build |
| dev rustls-pemfile 2 | Test certificate/trust-chain parsing |

BSON transitively exposes ObjectId/random/time functionality; the protocol never calls
those generators. Host supplies nonce entropy, session UUIDs, ObjectId seconds/random
bytes and current monotonic timestamps. Dependency graph gate includes dev and target
packages, not only the native production tree.

## Deviations / proposed DESIGN.md clarifications

1. Distinguish warmed command/row allocation budgets from connection establishment,
   topology changes and persistent owned result state. The measured budget here is zero
   for the warmed wire/operation paths; full Node facade allocation is not claimed.
2. Protocol ownership and transport ownership should be separate in §5b: the core,
   operation coordinator, topology and pool contain policy, while adapters execute byte,
   TLS, DNS and connection lifecycle requests. No executor/runtime dependency is needed.
3. Explicit host time needs a browser-constructible representation. The core lane now
   has a HostInstant-like wrapper; this crate mirrors it on browser wasm and re-exports
   std Instant elsewhere. Agree on a shared public timestamp type during adapter work;
   no dependency on the still-renaming core crate was introduced.
4. Document the supported Node surface and official suite subsets as separate gates.
   Passing 287 selected fixtures does not establish complete driver conformance.
5. Current upstream specs add backpressure-v2 requirements beyond this brief. Do not
   advertise backpressure support until its state machine and tests exist.

## Open questions / next steps

- Integrator: decide the shared host timestamp/transport token types when the executor
  adapter lands. Preserve accepted-token completion and receive-buffer release contracts.
- Complete a Node/Perry-facing facade: option inheritance, ID insertion/result maps,
  cursor lifecycle, mixed bulk grouping and exact local error-text parity. Existing
  Perry bindings remain unchanged and cannot yet switch dependencies without adaptation.
- Add the remaining official CMAP/SDAM event/URI/retry/session/transaction/change-stream
  runners and multi-platform runtime CI. Extend the private harness to mongos if that
  stage is required for release.
- Finish higher-level change-stream lifecycle and snapshot sessions; decide support
  policy for backpressure, CSOT extensions, load balancers and other URI/auth features.
- Benchmark throughput/CRC costs and allocation coverage for more BSON types, bulk sizes,
  sessions, pooling and adversarial workloads. No throughput claim was made here.
