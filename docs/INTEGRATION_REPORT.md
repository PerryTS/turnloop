# Integration report — audited main, 2026-09-15

**Snapshot:** `a0fbb1bf9a100bb3215219f54301b9ee82ffe71f` (main / PR #13 merged).
This is the alpha.2 codebase plus the Windows follow-ups, not a claim that those
follow-ups are in the published alpha.2 archives. This report supersedes the
accumulated integration history. Local failures in historical lane reports do
not describe current required CI coverage.

**Conclusion:** all required CI jobs are green on this exact commit. Production
epoll, kqueue, IOCP, WASI 0.2, experimental WASI 0.3 and web backends, the executor,
and protocol adapters exist and execute tests. The complete DESIGN is **not**
implemented: this audit found reproducible gaps outside those gates, missing
capabilities, and unfinished Perry integration.

- [Section-by-section DESIGN audit](DESIGN_AUDIT.md): classifications, source lines,
  named tests and verification limits.
- [Remaining work](REMAINING_WORK.md): concrete internal and Perry implementation lanes.
- [Evidence and reproduction](AUDIT_EVIDENCE.md): commands, local failures and CI provenance.
- [Open-marker inventory](AUDIT_MARKERS.md): every matched marker and cfg selection
  at the audited snapshot, with current disposition.

## 1. CI evidence actually read

Job metadata was read with `gh run view <id> --json jobs` (plus SHA/status fields),
and every successful job log with
`gh api repos/PerryTS/turnloop/actions/jobs/<job>/logs`.
The [job inventory](audit/ci-jobs.tsv) records IDs, URLs, conclusions, log hashes
and runner counts: **111 successful job logs across three runs**.

| Run | Commit | Result and relationship |
|---|---|---|
| [34919345014](https://github.com/PerryTS/turnloop/actions/runs/34919345014) | `a0fbb1b` | Exact audited main: 37 successful jobs; optional `self-hosted-windows` skipped |
| [34918732035](https://github.com/PerryTS/turnloop/actions/runs/34918732035) | `163fd0c` | PR #13 head: same tree as audited main; 37 successful jobs, optional self-hosted job skipped |
| [34916140575](https://github.com/PerryTS/turnloop/actions/runs/34916140575) | `7f37e7e` | Earlier main: 37 successful jobs, optional self-hosted job skipped |

The following citations refer to exact main. A successful workspace invocation
does not turn ignored or cfg-excluded bodies into passing tests. Repeated
workspace/member runs are not unique test totals.

| Evidence key / job | Execution observed |
|---|---|
| **N-L**: `test-native (ubuntu-24.04, default)` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223733081) | 252 workspace tests; independent core 16 / contract 49. Both Linux architectures run default, executor, epoll-timerfd, process-sigchld, combined fallbacks and all-features: 12 required arms |
| **N-A**: `test-native (ubuntu-24.04-arm, default)` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732844) | Same default counts; actual arm64 runtime |
| **N-M**: `test-native (macos-15, default)` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732823) | 251 workspace; independent core 14 / contract 50. Executor and all-features are separate required arms |
| **N-W**: `test-native (windows-2025, default)` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732855) | 267 workspace; independent core 19 / contract 62. Executor [arm](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732852): 274 / 20 / 69; all-features [arm](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732837): 323 / 20 / 69 |
| **W2**: `wasi (wasm32-wasip2, nightly-2026-08-20)` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732694) | Wasmtime 46: 7 core, 22 debug contracts, 22 release contracts, 9 release allocation tests |
| **W3**: `wasi (wasm32-wasip3, nightly-2026-09-07)` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732792) | Wasmtime 46: 9 core, 23 debug contracts, 23 release contracts, 10 release allocation tests; experimental feature enabled |
| **WEB**: `web` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732711) | 13 tests **each** in Chrome, Firefox and Node. Each fixture reports 5 fetches, 2 aborts, 6 WebSockets and 7,937 echoed bytes |
| **P**: `protocol` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732707) | 18 native protocol suites, plus WASI 0.2 portable and real-server suites. Actual PostgreSQL 16, MySQL 9.6, Redis cluster/Sentinel, MongoDB replica set and Postfix transactions; TLS probes and fixture assertions execute |
| **PW2**, **PW3**: `protocol-wasi` [p2](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732739), [p3](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732879) | 19 selected suites each: stream/TLS/HTTP/SQL/Redis/SMTP/MongoDB/WebSocket and allocation cases. These jobs use scripted peers; the additional real-server run is p2 in **P**, not p3 |
| **H2**: `h2spec` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732723) | **147 strict tests, 147 passed, 0 skipped**, against the async turnloop server |
| **MODEL**: `loom` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732726), `miri` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732992) | Six production Loom models; two Miri filters (`timer::tests`, `table::tests`), not an all-core Miri run |
| **PERF**: `instructions` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732853) | Interleaved Callgrind gate: control 527; idle 111,569 ≤ 191,486; notify 108,089 ≤ 188,006; timer_cancel 321,820 ≤ 414,171. Identical min/max across three rounds. No TCP or blocking-job instruction budget yet |
| **DEP**: `dependencies` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732684) | Zero-runtime dependency graph audit, cargo-deny, seven-day soak: 251 registry versions and one existing reviewed rustls security exception. Seven-day policy remains active |
| **LINT**: `lint-native`, `lint-wasm`, `workflow-lint` [automation log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104223732559) | Native fmt/strict Clippy/docs/stable/MSRV; wasm target Clippy; 97 automation tests. Mock runner outputs inside gate unit tests are not backend execution evidence |
| **GATE**: `ci-gate` [log](https://github.com/PerryTS/turnloop/actions/runs/34919345014/job/104225153295) | Required dependency fan-in succeeded; only the explicitly optional self-hosted Windows arm was skipped |

Every native mode also executes eight HTTP/TLS/WebSocket interop suites (38 tests).
The intentional curl-unavailable regression proves the Node 100-stream fallback;
its logged UNRUN curl leg is expected. The normal Windows curl path now uses a
checksum-pinned build with HTTP/2. Neither case means HTTP/2 interop is unrun.

### Named subjects superseding stale UNRUN claims

- Native: `native::idle_socket_timers_do_not_spin`, `native::transfer_inflight`,
  `native::shared_pool`, `steady_read_write_timer_and_accept_allocate_nothing`.
  Windows additionally executes backlog/deadline, direct versus foreign-port
  routing, console, child teardown and handle-count subjects in
  `crates/turnloop-contract/tests/windows_lifetimes.rs:507` and
  `crates/turnloop/src/backend/iocp/pipes.rs:333`.
- WASI: no-spin/timer/executor/stdio subjects in
  `crates/turnloop-contract/tests/wasi.rs:67` and `tests/allocations.rs:1065`.
  WASI 0.2 `resolve_and_cancel_reuse_executor_slots` runs; p3's
  `unsupported_wasi_dns_releases_reserved_slots` proves rejection, not DNS support.
- Browser/Node: `fetch_bytes_abort_and_close_ordering`,
  `timer_no_spin_with_idle_websocket`, `worker_mpsc_backpressure_and_no_lost_wake`,
  `steady_rust_websocket_posts_and_timers_allocate_nothing`.
- Real SQL: `real_async_required_channel_binding_authenticates_over_ssl`,
  `real_async_tls_queries_copy_cancel_pool_and_connection_kill`,
  `real_async_tls_rsa_text_binary_multi_results_transactions_pool`.
- Real Redis/MongoDB/SMTP:
  `real_async_tls_pipeline_pubsub_deadlines_reconnect_cluster_sentinel`,
  `real_async_auth_tls_sdam_pool_cursor_retry_and_primary_stepdown`,
  `real_async_postfix_delivery`. All execute natively and under WASI 0.2 in **P**.

## 2. Platform claims versus verified scope

Backend selection is `crates/turnloop/build.rs:8`; CI matrices are
`.github/workflows/ci.yml:48`, `:106`, `:193`, `:232`, `:378`.

| Platform | Current evidence | Limits relative to DESIGN §3 / §7.6 / §11 |
|---|---|---|
| Linux x86_64 / arm64, GNU | Required runtime + lint, all six native modes | One hosted Ubuntu/kernel family; no overnight churn or syscall-trace gates |
| macOS arm64 | Required runtime + lint, three modes | Does not verify Apple mobile or macOS x86_64 |
| Windows x86_64 | Required Windows 11 runtime + lint, three modes | Minimum Windows 10 version, other VMs and cycle/ETW attribution unverified; optional private runner skipped |
| WASI 0.2 | Required core/contract/adapter runtime; real servers | No filesystem API or TTY size API; detach and generic blocking jobs unsupported |
| WASI 0.3 | Required experimental core/contracts and scripted adapter runtime | No DNS/filesystem; strict host-progress/cancellation bound remains unproved; no real DB service suite or second runtime |
| Web | Required Chrome + Firefox + Node, live fetch/WS fixtures | No general Web Streams adapter, no non-isolated postMessage Poster; worker producers are JS, not a demonstrated Rust loop per worker |
| FreeBSD x86_64 | **Local cross-clippy PASS** for core/contracts | No CI job, including the DESIGN-required nightly; runtime **UNRUN** |
| Android aarch64 | Backend selected; local all-target cross-clippy **FAIL** | `portable_tests.rs:13` triggers `missing_const_for_thread_local` despite const initializers; no emulator/device CI; runtime **UNRUN** |
| iOS device + simulator aarch64 | **Local cross-clippy PASS** for core/contracts | No simulator/device CI, linking/package/lifecycle tests; runtime **UNRUN** |
| tvOS / visionOS / watchOS aarch64 | **Local cross-clippy PASS** for core/contracts | No CI or runtime evidence; OS capabilities/entitlements are not established by type checking |
| Linux x86_64 musl | Backend selected; local cross-clippy **FAIL** | Deprecated `libc::time_t` at `backend/poller.rs:25`; no CI; runtime **UNRUN** |
| OpenBSD / NetBSD / DragonFly | kqueue selected | No compile or runtime evidence in this audit; not all contract cfgs include them |
| Windows arm64, macOS x86_64, other architectures | No runtime evidence in reviewed jobs | Do not infer support from the x86_64 Windows / arm64 macOS arms |
| WASI preview 1 / `wasip1-threads`, other OSes | Dependency audit may resolve their graphs | No production backend selected for preview 1; graph coverage is not backend support |

## 3. Remaining blockers and qualification of green CI

1. **Reproduced outside current gates:** WASI 0.2 compressed MongoDB commands
   allocate once per command. (The queued post + idle UDP OS poll was resolved by
   the tl-i01b amendment of DESIGN §10 rule 3: queued turns never block, may make one
   zero-timeout discovery poll only with native operations pending, and make no OS
   call otherwise; `discovery_polls` is counted separately from blocking waits.)
   See [local evidence](AUDIT_EVIDENCE.md#local-failures-and-probes).
2. **No-spin assurance is partial:** ordinary native/WASI/web subjects pass;
   WASI private timer events undercount `zero_event_waits`, and p3 still relies on
   host cooperative yield and synchronous cancellation with no proven latency bound.
3. **Missing capability/API work:** WASI filesystem and p3 DNS, native typed file
   operations/fs-watch, per-loop blocking-pool option, generic Windows handle
   passing/named-pipe listener transfer, web streams/non-isolated worker routing.
4. **Verification work:** FreeBSD nightly/mobile CI, broader fault/leak/long soak,
   syscall traces, full operation instruction budgets, controlled SRV/TXT fixture,
   broader protocol conformance and target allocation gates.
5. **Release/security:** `SECURITY.md` is absent. The
   [exact-main release workflow](https://github.com/PerryTS/turnloop/actions/runs/34919821444)
   verified CI but failed creating its GitHub App token because the app/client ID
   was empty; publish was skipped. The
   [alpha.2 release](https://github.com/PerryTS/turnloop/releases/tag/v0.1.0-alpha.2)
   exists, but reviewed workflows do not prove successful OIDC publication.
6. **Perry P0–P8:** turnloop supplies building blocks, not the Perry hook/ABI/GC/JS
   migration. This repository contains no implementation or end-to-end evidence
   for those phases. No claim is made about changes in another Perry checkout.

## 4. Local verification of this audit

PASS: `cargo fmt --check`; strict workspace Clippy, default and all-features;
`cargo +stable check --locked --workspace --all-targets --all-features`;
`cargo test --workspace` with default parallelism; zero-tokio, soak and feature-mode
gates. Cross-check results and additional failing probes are listed above and in
the [complete command ledger](../LANE_REPORT.md).

Local Linux/Windows runtime, SQL servers, browsers and Docker were **UNRUN in this
lane**. Their cited hosted CI runs are verified separately. This audit changes
documentation only; it does not repair or relax any failing behavior or gate.
