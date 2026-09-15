# Remaining work — implementation lanes after a0fbb1b

Audited 2026-09-15 against DESIGN draft 0.3 plus §10.4a.
[Current CI](INTEGRATION_REPORT.md) is green; [the requirement audit](DESIGN_AUDIT.md)
and [marker inventory](AUDIT_MARKERS.md) explain why that is not full completion.
Priorities below express planning order, not permission to postpone Windows,
WASI or web. Items within each priority can be scheduled independently where
prerequisites permit. No production changes are made by this audit.

For **every** implementation item: preserve zero tokio/other banned runtimes and
the seven-day soak; add tests asserting their subject executed; retain exactly-once
terminal delivery, lifetime safety and no-spin; extend the affected allocation
gates. Compile checks do not replace runtime acceptance. Required fmt, strict
Clippy, stable check, workspace tests and affected-target jobs must pass.

## A. turnloop-internal items

### Priority 1 — reproducible failures and incomplete first-class contracts

#### I01. Honor the no-OS-wait rule with queued posts and pending I/O

- **DESIGN:** D7, §10.3, §10.4a; multithread fairness §5a.9.
- **Evidence:** `crates/turnloop/src/driver.rs:1013–1024` polls with zero timeout
  when posts are queued but native operations remain. `backend/unix.rs:586`
  still enters the OS. [Local probe](AUDIT_EVIDENCE.md#local-failures-and-probes)
  delivers one queued post with **os_waits=1** on an idle UDP receive.
- **Acceptance:** add the queued-post/idle-native case to shared contracts and
  assert zero OS waits, actual post delivery and later I/O progress. Include
  queued terminal completions, repeated timers, full output buffers and sustained
  producer traffic; preserve the existing progress and no-spin tests. If this
  conflicts with fairness, submit a concrete specification decision with evidence;
  this audit does not authorize weakening §10.3.
- **Verify:** Linux all applicable modes/fallbacks and both architectures, macOS, Windows, WASI
  0.2/0.3; callback-equivalent web case. Local runtime evidence currently macOS only.

#### I02. Normalize WASI deadline-event accounting

- **DESIGN:** §10.4a; `PollInfo` definition at `backend/mod.rs:204`.
- **Evidence:** p2 `backend/wasi_p2.rs:655,684` includes the private deadline
  pollable in the returned indices; p3 `backend/wasi_p3.rs:559,619` treats any
  wait-set event as work, including its private deadline. Both can report zero
  zero-event waits for a timer-only wake. Native IOCP explicitly normalizes this
  at `backend/iocp/mod.rs:1388`; epoll has equivalent timerfd handling.
- **Acceptance:** count private timeout-only wakes as zero-event OS waits and
  distinguish simultaneous real I/O/notifier events. Extend quiet-deadline
  accounting tests to both WASI backends; prove one real wait/expiry and identical
  idle/registered-idle results. Keep ≤2 turns and ≤1 zero-event wait at all three
  design delays, without a guest wait floor or allocation.
- **Verify:** Wasmtime 46 p2 and p3, debug semantics/release precision; native and
  browser contracts guard common semantics. This is an accounting defect, not
  evidence that ordinary WASI tests currently spin.

#### I03. Remove compressed MongoDB command allocations on WASI and gate them

- **DESIGN:** §3.3, §5b, §10.1, §11 every configuration.
- **Evidence:** existing `protocols/turnloop-mongodb/tests/allocations.rs:139`
  fails locally on p2: **1,000 allocations / 1,000 commands**, zlib=true,
  operation=false. Compressor reset at `src/connection.rs:178`; `Cargo.toml:41`
  selects only `asynchronous` for WASI. `docs/lanes/proto-fix1.md:137` records the
  same prior p2/p3 miniz reset problem; current p3 was not rerun in this audit.
- **Acceptance:** fix storage reuse without masking allocator counts; execute
  all four compressed/uncompressed × raw/coordinator modes, calibration and
  positive row/command counts on both WASI targets. Make this existing zero gate
  required in CI. Check MySQL compressed reset under its own target allocator
  before claiming it has or does not have the same issue. Preserve native gates
  and dependency soak; any fork must be packaged for downstream consumers.
- **Verify:** native three OSes; Wasmtime p2/p3 release. A portable compressed
  codec test may also run in Node/browser without raw database sockets.

#### I04. Establish a bounded WASI 0.3 host contract

- **DESIGN:** D7, §7.4, §10.3/4a, M1.
- **Evidence:** `backend/wasi_p3.rs:577–598` calls cooperative `yield_blocking`
  before a Now poll and synchronously cancels a deadline subtask.
  `docs/upstream/wasi-p3-wait.md:25` explicitly lists unproved host yield,
  deadline fairness, cancellation latency and raw context-0 portability.
  W3/PW3 prove ordinary execution, not these adversarial bounds.
- **Acceptance:** a supported bounded step/cancel mechanism or documented,
  tested runtime contract; tests with stalled host tasks, simultaneous imports,
  deadline storms, cancellation/drop and multiple component exports. Measure
  actual yields/waits/cancel acknowledgements, preserve provided-buffer lifetime,
  enforce no-spin and release allocation limits. Retain experimental status until
  the stated bound is justified; do not claim one ABI call implies bounded time.
- **Verify:** pinned Wasmtime 46 plus a supported newer version/second host where
  available. Add real-server p3 adapter execution after the bound and DNS work;
  p2 and web remain regression arms. Upstream API changes may be a dependency.

#### I05. Implement WASI 0.3 DNS resolution

- **DESIGN:** §3.5, §5b DNS, §7.6 DNS row.
- **Evidence:** `backend/mod.rs:280` defaults to Unsupported; p3 has no override.
  `crates/turnloop-io/tests/streams.rs:129` explicitly tests rejection. p2
  `backend/wasi_p2.rs:555` and its positive resolver tests are already implemented.
- **Acceptance:** A/AAAA stream results, hostname/permission errors, bounded result
  limits, exact cancellation/close/slot reuse and deadline behavior; positive
  hostname resolution/connect assertions on real fixtures. Preserve rejection
  for browser raw DNS and unsupported host capabilities.
- **Verify:** Wasmtime p3; p2/native regression. Required p3 protocol jobs should
  connect by controlled hostname rather than requiring caller-supplied IPs.

#### I06. Complete file operations and filesystem-watch support

- **DESIGN:** §3.5, D8, §5a.4, §6 filesystem sketch, §7.6 Files, M3.
- **Evidence:** public `types.rs:208` / `driver.rs:435` surface has no typed file
  requests or watch operation. Unix `backend/files.rs:379` reads/writes adopted
  descriptors through reusable workers. Windows `backend/iocp/sync_io.rs:1`
  uses a worker per synchronous handle; arbitrary overlapped files are rejected.
  Neither WASI backend exposes `wasi:filesystem` operations.
- **Acceptance:** define concrete open/read/write/metadata/close and watch scope
  needed by Perry; add bounded pool-backed native operations and capability-scoped
  WASI 0.2/0.3 implementations under the same ownership/cancellation contract.
  Prove actual file bytes/metadata, permission errors, watch changes, ordering,
  pool backpressure, cancellation and no buffer access after completion. Resolve
  D8's shared-file-pool wording versus Windows per-handle workers explicitly.
  Optional OPFS requires a host mapping; do not invent raw browser filesystem access.
- **Verify:** Linux/macOS/Windows filesystem fixtures; preopened-directory Wasmtime
  p2/p3 fixtures; browser OPFS only if included in the agreed scope.

#### I07. Complete web stream and non-isolated worker interfaces

- **DESIGN:** §5a.8, §7.5, §7.6.
- **Evidence:** `backend/web/host.js:61` buffers a whole fetch response before its
  size check; `:133` creates SAB-based workers. `types.rs:208` exposes fetch/WS,
  not general ReadableStream/WritableStream handles. Tests' workers
  (`crates/turnloop-contract/tests/web/helpers.js:8,41`) are JS ring producers.
- **Acceptance:** real ReadableStream/WritableStream adapters with bounded
  backpressure, cancellation and close; enforce fetch body limits while reading,
  not after materialization. Provide postMessage routing without isolation and
  tests with a separate wasm instance/Loop in each worker, owner-only delivery,
  timer/WS I/O and pending-drop cleanup. Keep SAB fast path under isolation and
  zero Rust allocation tests. Now-only worker turns are acceptable if the design
  documents callback scheduling; do not block the worker needed to run callbacks.
- **Verify:** Chrome, Firefox and Node worker_threads where APIs exist, both
  isolated and non-isolated browser fixtures; Rust and JS memory costs separate.

#### I08. Restore release automation and required security documentation

- **DESIGN:** §13, M0/release readiness.
- **Evidence:** no tracked `SECURITY.md`; `.github/workflows/release.yml:63`
  obtains a GitHub App token. Exact-main release
  [34919821444](https://github.com/PerryTS/turnloop/actions/runs/34919821444)
  fails there with an empty app/client ID; publish skipped. Alpha.2 release exists.
- **Acceptance:** configure the workflow's App ID in its intended context; verify
  credentials/configuration without exposing secrets. Add maintainer-approved
  security contact/reporting policy. Run release-PR preparation against exact
  successful CI; for the next intended release demonstrate OIDC token exchange,
  dependency-order publication, registry/archive checksums, source provenance and
  rerun/idempotence behavior. Do not publish merely as an audit test.
- **Verify:** GitHub Actions + crates.io; local packaging/dry-run and gate unit
  tests supplement but cannot prove remote Trusted Publishing.

### Priority 2 — capability boundaries and required verification

#### I09. Finish pool configuration, allocation scope and waiter fairness

- **DESIGN:** D8, §3.1/3.3, §5a.3/9, §7.4 jobs.
- **Evidence:** native singleton `blocking.rs:94`, per-job boxing `:267` and
  cancellation Arc `driver.rs:878`; no per-loop configuration. Wasm submit
  `blocking.rs:259` returns Unsupported. External wait routing tests
  `crates/turnloop-contract/src/native_surface.rs:273` lack sustained fairness bounds.
- **Acceptance:** per-loop/shared choice and lifecycle or an explicit design
  decision retaining a shared service; reusable job submission storage if jobs
  fall under §3.3's no-per-operation-allocation promise; host async-job path for
  WASI (arbitrary inline work cannot preserve bounded turn). Prove started/queued
  cancellation, capacity recovery, drop, thread counts and non-starvation among
  multiple continuously busy loops. Preserve JS exclusion from worker jobs.
- **Verify:** native all three OSes; WASI host integration; web executor/worker
  fallback only for capabilities actually exposed by the host.

#### I10. Complete transfer and Windows lifecycle boundaries

- **DESIGN:** §5a.5/6, D4/D7, §7.3, §15.3.
- **Evidence:** `backend/iocp/mod.rs:1265` sends sockets only; Unix
  `backend/ipc.rs:125` likewise classifies socket kinds. Windows named-pipe
  listeners/busy connects cannot detach (`backend/iocp/README.md:37`).
  After watch cancellation release/drop may synchronously await child teardown
  (`docs/BACKEND_REVISION_2.md:181`). `iocp/timer.rs:27` assumes high-res NT support.
- **Acceptance:** define transferable resource classes; support required
  pipe/file/server transfers with correct process-peer ownership and cancel/drain
  before reuse, or explicitly narrow the design. Address bounded-turn child release
  after watch cancellation without freeing live kernel buffers. Test minimum
  supported Windows version, timer-export absence, actual console shutdown
  semantics and VM timer distributions. Existing #13 backlog/direct/foreign-port,
  FIFO, argv/env/cwd and console tests are already green and must stay so.
- **Verify:** Windows hosted + minimum-version/VM hosts; Unix socket/fd transfer
  regression; WASI/web explicit single-agent capability tests. Do not assume
  Windows uid/gid or Unix-only signals can be implemented on that OS.

#### I11. Add a controlled DNS/SRV/TXT integration runner

- **DESIGN:** §5b DNS/MongoDB, §11 test execution.
- **Evidence:** the only ignored test without a runner is
  `crates/turnloop-io/tests/streams.rs:215`,
  `native_srv_txt_records_through_blocking_pool`; it uses public DNS. Native
  resolver implementations `src/dns.rs:57,119` exist; Windows DNSQuery path lacks
  equivalent real query coverage. Full mongodb+srv discovery is not CI-proven.
- **Acceptance:** private deterministic DNS fixture, native SRV/TXT resolution,
  priority/weight/TTL and malformed/permission/error handling, cancelled lookups,
  MongoDB SRV/TXT seed discovery and refresh; every fixture/test must assert query
  and connection counts. Select the ignored subject or replace its public-network
  dependence with the controlled test without reducing its assertions.
- **Verify:** Linux/macOS/Windows; WASI direct-IP/basic DNS boundaries remain
  explicit until the required query types are supported by the host.

#### I12. Make the promised platform matrix executable

- **DESIGN:** §3.2, §7.6, §11, §15.6.
- **Evidence:** `crates/turnloop/build.rs:8` selects more platforms than
  `.github/workflows/ci.yml:48,106`. Local core/contract cross-clippy passes
  FreeBSD and five Apple mobile target variants; Android test lint and musl
  deprecation fail ([evidence](AUDIT_EVIDENCE.md#local-failures-and-probes)).
- **Acceptance:** fix Android const-thread-local lint without weakening warnings
  and fix musl timespec conversion; add required compile arms and the prescribed
  FreeBSD nightly runtime. Run iOS simulator and Android emulator contracts on a
  non-main thread, then qualify tvOS/visionOS/watchOS capability/entitlement limits.
  Explicitly document macOS x86_64/Windows arm64 and additional BSD support tiers
  based on evidence; selecting kqueue in build.rs alone is insufficient.
- **Verify:** relevant native/emulator/simulator hosts; cross-clippy for targets
  lacking a runner stays compile-only. Include mobile wait, pipe, process/signal
  restrictions, allocation and no-spin rather than a zero-test crate build.

#### I13. Finish independent syscall and instruction gates

- **DESIGN:** §7.6 measurement, §10.2/5, §11.
- **Evidence:** `notifier.rs:49` reports wake-attempt counters; no strace/ktrace/ETW
  gate. `crates/turnloop-bench/benches/instructions.rs:1` and committed JSON cover
  control/idle/notify/timer_cancel only, using Callgrind rather than kernel perf.
- **Acceptance:** independently observe zero running-notify syscalls with positive
  parked-wake controls on each native OS; instrument read/write/accept and blocking
  round trips. cgu=1 fresh interleaved controls, operation counts, committed reviewed
  budgets and kernel/user attribution against the Perry bridge and hand-written
  epoll baseline. Establish Windows cycles, WASI fuel and browser-relative reporting
  where the matrix promises them. Keep controls exact and baselines reviewable.
- **Verify:** Linux perf/Valgrind, macOS tracing/rusage, Windows ETW/cycles, Wasmtime
  fuel and browser profiler. Tool permission failure is UNRUN, not a passing zero.

#### I14. Add long soak, broader faults, leak and pure-Rust coverage

- **DESIGN:** §11 fault/leak/model/soak clauses.
- **Evidence:** bounded stress and selected fd/handle leak tests exist; Miri only
  `timer::tests` and `table::tests` (`crates/turnloop/Cargo.toml:46`). No CI schedule.
- **Acceptance:** nightly long echo/timer/process churn per native OS; comparable
  WASI/web socket/timer churn within host capabilities. Inject EINTR, partial I/O,
  EAGAIN storms, resets/aborts, exhaustion, pre-registration child exit, signal
  storms and cancel/drop races; measure actual fault hits and stable resource/
  thread counts. Extend Miri to suitable executor/buffer/queue components and
  fuzz bounded protocol decoders with corpus/iteration counts and CPU/memory limits.
- **Verify:** native OS runners, Wasmtime p2/p3, Chrome/Firefox/Node; no invented
  process/signal tests on platforms that intentionally lack them.

### Priority 3 — close compatibility and documentation scope

#### I15. Define protocol compatibility targets and close their concrete gaps

- **DESIGN:** §5b, M4/M6/M7; Perry P5–P7.
- **Evidence:** [protocol audit](DESIGN_AUDIT.md#5b-protocol-crates-and-adapters).
  Examples: `turnloop-websocket/README.md:8` no permessage-deflate;
  `turnloop-redis/src/resp.rs:154,183` rejects streamed RESP3;
  `turnloop-mongodb/README.md:148` lacks full change-stream lifecycle;
  `docs/lanes/proto-mongo.md:99` full unified runners absent;
  `docs/lanes/proto-http.md:14` documents Fetch/error/stacked-coding limits.
- **Acceptance:** pin the Perry-exposed npm versions/options/error semantics and
  map each supported API to an asserted test. Implement required gaps: full HTTP
  client/Fetch policy and pool lifecycle, WS extensions if Perry exposes them,
  SQL statement/type/options parity, Redis reconnect/failover/subscription scope,
  MongoDB CMAP/retry/transaction/change-stream runners and lifecycle, SMTP streaming/
  pooling if exposed. Record explicit non-goals instead of promising every upstream
  driver's features. Define allocation budgets for owned results separately from
  core operations; never call a nonzero codec baseline zero-allocation.
- **Verify:** native and WASI 0.2 real services, WASI 0.3 real services after I04/I05,
  shared sans-IO fixtures on all capable targets, Node/browser host HTTP/WS.

#### I16. Resolve specification drift and release-consumer provenance

- **DESIGN:** D3/D8, §5b tray, §7.4–7.6, §13, §15.
- **Evidence:** `docs/BACKEND_REVISION_2.md:1` omits the later additive resolver in
  its trait handoff; root/core READMEs still call wasm production backends pending;
  HTTP README still requires IPs for all WASI clients; SQL READMEs accurately qualify browser-only runtime as unverified. [Marker inventory](AUDIT_MARKERS.md)
  preserves exact snapshot lines. Current bindings do not expose claimed WASI
  TTY size; `backend/mod.rs:249,253` defaults to Unsupported.
- **Acceptance:** reconcile explicit lease lifetime, shared pool/file worker
  topology, supported handles, p3 experimental bounds, TTY/files/DNS/worker matrix
  and Rust versus host allocations. Reconcile §9 “no other Perry change” with §12 P0
  precision/liveness requirements. Remove the async-io tray alternative, consistent
  with binding zero-runtime policy. Update active READMEs/contribution/handoff docs;
  retain historical lane ledgers as history. For Perry version bumps, verify every
  consumed archive checksum/source commit/time and record the one-time age override;
  remove the existing rustls exception once its reviewed age window completes.
- **Verify:** source/link/metadata checks and consumer build/package tests; remote
  release provenance as I08. No silent relaxation of DESIGN or soak is authorized.

#### I17. Harden the retained zstd encoder surface and track inherited TODOs

- **DESIGN:** §5b HTTP compression, §11 faults/coverage; contribution I/O-error rules.
- **Evidence:** all 27 TODO/todo!/unimplemented! matches are in the published
  `turnloop-zstd-decoder` fork. Two active encoder panic branches remain at
  `src/encoding/frame_compressor.rs:203` and `src/encoding/blocks/compressed.rs:349`.
  Public `encoding::compress` exposes the encoder; I/O reads/writes unwrap at
  `frame_compressor.rs:159,183,206`. These are inherited, not added in this audit,
  and are not evidence of a panic in the HTTP decompression path.
- **Acceptance:** decide whether encoding is part of the supported published API;
  make supported levels and I/O failures return explicit results, or constrain
  the advertised surface without pretending the encoder is complete. Prove
  malformed/oversize input and failing reader/writer paths execute. Resolve the
  decoder Failed-state TODO and retain the existing corpus/reuse gates. Track
  optimization-only TODOs separately; not all require a new feature.
- **Verify:** native unit/corpus tests; portable wasm decoder tests; encoder target
  tests if that API remains supported. Keep the two existing interop corpus files
  and 40 decode regression artifacts; broad upstream fuzz campaigns remain I14.

## B. Perry integration items — work in Perry, not this repository

This audit only inspects turnloop. **MISSING here** means no implementation or
verification in this repository; it does not assert the present state of an
uninspected Perry branch. `DESIGN.md:220` names an older Perry base. Re-audit the
actual integration checkout before deleting existing implementations.

**Acceptance shared by every phase (§12:518):** Perry fast and auto-optimize gap
suites; GC stress with `PERRY_GC_SCHEDULE_SEED` and
`PERRY_GC_PROTECT_FROMSPACE`, counters proving collections occurred during pending
I/O; cgu=1 instruction A/B with stable controls; required Windows arm. Add Linux,
macOS, WASI 0.2/0.3 and web capability-specific end-to-end runs. Buffer roots,
owner-thread promise settlement, exactly-once release, cancellation and no-spin
are integration assertions, not inferred from passing Rust codec tests.

| Item / order | DESIGN and evidence | Concrete acceptance | Platforms that verify |
|---|---|---|---|
| **P0: install precise wait driver and O(1) liveness** | §9:465; §12:526. turnloop `driver.rs:331,339,1000` exists; no Perry hook implementation here | Create loop per main agent; wire `js_register_wait_driver` hooks, destroy correctly; compute Instant deadlines in the same change; remove ms truncation/floors and keep-alive scans. Count turns/zero waits with idle socket and 0.5/2/10 ms JS timers; no polling-until-throttle behavior | Linux/macOS/Windows blocking hosts; WASI runtime turns; web schedule_turn host |
| **P1: replace net and IPC bindings** | §5a.5, §12:527; native TCP/UDP/pipe/transfer APIs and N contracts | Migrate bundled stdlib and perry-ext-net; root/pin buffers until terminal result, map Node errors, write backpressure/half-close/multishot accept, Windows named pipes, child.send socket ownership, multi-worker accept policy; remove tokio tasks and mpsc-per-write | Native three OSes; WASI TCP/UDP; browser explicit raw-socket rejection |
| **P2: remove process/stdio/dgram/signal helper threads** | §5a.4, §12:528; `driver.rs:503,522,581,621` | Move child_process/container, pty, stdin, dgram, signals and IPC onto loop handles; fs-watch on I06. Verify actual child/stdout/stderr bytes, signals, grandchildren cleanup and resource/thread deltas. Retain only OS-required Windows synchronous console/stdio helpers | Native three OSes; mobile restrictions; WASI stdio/UDP and explicit process/signal rejection; web rejection |
| **P3: per-agent JS timers and Node phase order** | §5a.7, §9:459, §12:529 | Move all timer classes to owning heap; implement timers → pending → poll → check → close with appropriate microtask/nextTick checkpoints, ref/unref and cancellation; prove interval drift/order, no global owner-tagged timer routing, no spin | Every target, main and worker agents; browser host clamp-aware assertions |
| **P4: pool jobs and perry-ffi ABI v2** | D8, §9:473, §12:530 | Replace spawn_blocking for bcrypt/argon2/sharp/zlib/crypto/fs/DNS/N-API; version `spawn_async`/`spawn_blocking`/`run_pending` on the owning loop, retain v1 shims where semantics carry over; Send-owned Rust payloads only on pool, JS conversion on owning thread. Count starts/completions/cancellations and GC roots; settle queue shutdown and async addon lifetime | Native three OSes; WASI/web host async/worker path where operations exist; cannot silently run arbitrary work inside bounded turn |
| **P5: server HTTP/TLS/WebSocket** | §5b:278/280/281, §12:531; N/P/H2 proof | Replace hyper/hyper-util, tokio-rustls/tungstenite bindings; preserve node:http/fastify/framework streaming, errors, TLS, WS extensions and close/abort semantics actually exposed. End-to-end JS server tests and allocation/instruction counters | Native three OSes, WASI sockets; browser host WS has client-only capability |
| **P6: client HTTP/SMTP** | §5b:279/286, §12:532; clients exist | Replace reqwest and lettre tokio transports for fetch/axios/undici/nodemailer; JS headers/body/redirect/proxy/decompression/abort/error parity. Connect Perry web host fetch/streams/WS imports; real SMTP delivery/STARTTLS/auth; avoid blocking-pool fetch dispatch | Native + WASI 0.2/0.3; Chrome/Firefox/Node for web HTTP; SMTP only raw-socket-capable targets |
| **P7: database bindings** | §5b:282–285, §12:533; P/PW2/PW3 clients | Replace sqlx, redis tokio-comp and official MongoDB driver; convert JS types/errors/results/options/cursors and pools. Gate prepared/pipeline/transaction/COPY/pubsub/reconnect/topology/retry behavior required by current Perry; require real authenticated effects and GC during pending rows | Native server fixtures + WASI 0.2/0.3; portable codec tests on web, explicit raw-DB Unsupported |
| **P8: remove runtimes and converge target paths** | §12:534; only turnloop DEP is proven | Remove async-runtime/tokio from perry-stdlib/perry-ffi/every ext crate, unused tokio-cron-scheduler; drive tray/MPRIS D-Bus without async-io or another banned runtime. Audit default/all/build/dev graphs for every Perry target. Use common stdlib paths through platform backends; pin released versions with timestamp/source/checksum record | All Perry targets and feature graphs; Linux desktop actual tray/MPRIS; fresh consumer builds |

### Cross-phase worker model and dependency order

From `DESIGN.md:244–270`: each worker_threads and async-enabled perry/thread agent
needs its own loop, poster, timer heap and GC-owned completion records. Eliminate
process-global wake flags/promise delivery queues; route signal, child, waitAsync,
message ack and blocking completions by owner. Test N agents cross-posting,
async work on non-main threads, handle transfer with I/O pending, worker shutdown
and surviving-loop progress. Web must cover both postMessage isolation-free
workers and the optional SAB path. This work spans P0–P4 rather than being solved
by installing one main-thread loop.

P0 precision/liveness and GC rooting are prerequisites for safe adoption. P1 and
P4 provide the transport/job foundation. DESIGN §12 permits P5–P7 to proceed
independently after those prerequisites. P8 is an end-to-end removal gate; it
cannot be marked complete from turnloop's dependency tree alone. Use I01–I17 as
explicit prerequisites where a phase requires a currently missing capability.
