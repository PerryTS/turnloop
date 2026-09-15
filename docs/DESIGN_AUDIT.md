# DESIGN audit at a0fbb1b

Audit date: 2026-09-15. Authority: [DESIGN.md](../DESIGN.md), draft 0.3 including
§10 rule 4a. Source references below are `file:line` in commit
`a0fbb1bf9a100bb3215219f54301b9ee82ffe71f`; they can be resolved beneath
[this immutable tree](https://github.com/PerryTS/turnloop/tree/a0fbb1bf9a100bb3215219f54301b9ee82ffe71f).
CI evidence keys and job/test counts are defined in the
[integration report](INTEGRATION_REPORT.md#1-ci-evidence-actually-read).
**N** means the required Linux x86_64/arm64, macOS arm64 and Windows x86_64 native
jobs, including the relevant executor/all-feature arms. A test cited by its
shared helper also names the actual wrapper where relevant.

Classification legend:

- **V — IMPLEMENTED and VERIFIED IN CI**, within the exact subject/platform scope stated.
- **U — IMPLEMENTED but not verified** by the reviewed CI; local evidence is stated separately.
- **P — PARTIAL**, including implemented behavior with an incomplete required gate.
- **M — MISSING**, no implementation or required verification facility found.

Each row is a requirement or closely related group; qualifications are part of
its classification. Historical motivation, non-goals and a non-normative spelling
are not treated as missing implementation. Green tests are sampled behavioral
evidence, not a proof of all executions of unsafe code.

## §§1–4: purpose, goals and architecture

| Requirement | State | Source evidence; executed subject or gap |
|---|---|---|
| §1 host turns an embeddable completion driver | V | `crates/turnloop/src/driver.rs:1000`; N `native::bounded` / Windows `bounded_turn`; W2/W3 bounded-turn subjects; WEB `now_only_and_unsupported` |
| §1 cross-platform replacement for Perry's runtime | P | Six production modules exported at `crates/turnloop/src/backend/mod.rs:306`; capability omissions below; Perry migration has no implementation in this repository (§12) |
| §2 motivation/old Perry instruction experiments | U | `DESIGN.md:41` records other repositories and older Perry snapshots. Those measurements were not rerun or attributed to current turnloop; current PERF covers only its own committed cases |
| §3.1 bounded turn, host primitive, exact deadline, lazy services | P | `driver.rs:339,906,1000`, `blocking.rs:94`, `backend/iocp/port.rs:1`; N external-waiter/timer contracts, WEB schedule tests. p3 strict bound and teardown qualifications below |
| §3.2 equal first-release targets; BSD/mobile paths | P | `crates/turnloop/build.rs:8`, `.github/workflows/ci.yml:48,106,193,232`; six main backends execute, p3 experimental; FreeBSD nightly and mobile CI absent. See full platform matrix in integration report |
| §3.3 predictable steady-state cost | P | Core allocation subjects pass N/W2/W3/WEB; generic jobs allocate (`driver.rs:878`, `blocking.rs:267`); queued-post poll and compressed WASI MongoDB failures reproduced locally; §10 below |
| §3.4 deterministic lifetimes | V | `driver.rs:719,778,998`, `backend/mod.rs:1`; N cancellation, stale IDs, drop/lease subjects; W2/W3 cancellation and release allocation tests; WEB fetch abort/close ordering |
| §3.5 portable native services/files/DNS/host jobs | P | `driver.rs:435–876` public operation surface (individual anchors below). Native services and DNS work; typed filesystem API, WASI files/p3 DNS, some transfer capabilities missing |
| §3.6 multithreading | P | Native loop ownership/routing/shared services verified (§5a); complete Perry/Web Worker model absent |
| §3.7 zero-tokio Perry | P | Repository graph gate V: `scripts/ci/no-tokio.sh:1`, DEP. Perry graph and package replacements are outside this repository; no end-to-end removal evidence |
| §3.8 optional futures integration | V | `crates/turnloop/Cargo.toml:14`, `executor.rs:223`, `crates/turnloop-io/src/lib.rs:1`; N executor, PW2/PW3 streams/TLS/HTTP and WEB executor subjects |
| §3.9 no JS/GC/Node types in core; portable kind + OS code | V | `types.rs:8,65`, `driver.rs:64`; N `bounded_capacity_and_stale_ids`, `connection_error`; host-specific web imports stay in `backend/web.rs:1` |
| §3 non-goals: tokio API clone, protocols inside core, io_uring in 0.1, embedded/no_std | V (scope) | `DESIGN.md:79`, `executor.rs:223`, `backend/mod.rs:306`. Their absence is intentional; no runtime proof is claimed for a non-goal |
| §4 backend/core/optional executor/sibling protocol separation | V | `crates/turnloop/Cargo.toml:14`, root `Cargo.toml:1`, `protocols/turnloop-http/Cargo.toml:1`; LINT default/all-features builds and DEP target graph audit |

Unless prefixed otherwise, `driver.rs`, `types.rs`, `blocking.rs`, `buffer.rs`,
`executor.rs`, `notifier.rs`, `timer.rs`, `external_wait.rs` and `backend/...`
in the tables mean files under `crates/turnloop/src/`.

## §5 D1–D9 and current Backend trait

| Requirement | State | Source evidence; job and test |
|---|---|---|
| D1 opaque token, caller-owned completion buffer, no application callbacks in turn | V | `types.rs:8`, `driver.rs:1000`; N `bounded`, `many_loops_post`, `timer_backlog_io_and_post_progress`; WEB scheduling import is host scheduling, not JS application dispatch |
| D2 completion-shaped TCP/UDP/read/write/accept, Unix cached edge readiness | V | `driver.rs:621,663,671,693,701,711`, `backend/unix.rs:560`, `backend/epoll.rs:1`, `backend/kqueue.rs:1`; N echo/vectored/UDP/connection-error subjects; W2/W3 TCP/UDP subjects |
| D2 IOCP overlapped sockets and named pipes | V | `backend/iocp/mod.rs:1132,1245`, `backend/iocp/pipes.rs:1`; N-W `accepted_pipe_uses_no_bridge_until_moved_to_another_port`, `unassociated_import_uses_direct_iocp_without_a_bridge`, Windows pipe lifetime suites |
| D3 provided read/write ownership; owned writes; pooled leases | V | `buffer.rs:6,40,74,82,127,157`; N `retained_pool_lease`, `steady_read_write_timer_and_accept_allocate_nothing`; executor moved/shrinking-buffer subjects (`crates/turnloop-contract/src/executor_contract.rs:163`); W2/W3 allocation gates |
| D3 Windows idle pooled TCP reads do not reserve a buffer per socket | V | `backend/iocp/mod.rs:594` zero-byte receive path; `crates/turnloop-contract/tests/windows.rs:1` pooled lease/backpressure contracts; N-W. Lease lifetime is explicit release/drop (`buffer.rs:177`), not next-turn invalidation |
| D4 exactly one terminal result, cancel before Closed, multishot stop | V | `driver.rs:719,723,778`, `backend/mod.rs:1`; N `cancellation_close`, `cancellation_reserves_survive_a_full_event_backlog`; W2/W3 contracts; WEB `fetch_bytes_abort_and_close_ordering`. Does not imply Drop is always nonblocking |
| D5 parked/running/notified handshake, bounded lock-free posts, per-loop routing | V | `notifier.rs:33,115`, `queue.rs:1`; N `parked_notify`, `running_notify`, `many_loops_post`; MODEL notifier/queue/pool models. OS syscall tracing requirement remains missing (§10) |
| D6 indexed 4-ary timer heap; immediate cancellation, next deadline | V | `timer.rs:10`, `timer/heap.rs:1`, `driver.rs:339,385,406`; MODEL `cancel_and_expire_match_sorted_reference`; N timer bounds/allocation/no-spin, PERF timer_cancel. Heap/BTree comparison executed in native CI (`ci.yml:161`) |
| D7 exact min deadline and one backend poll per turn | P | `driver.rs:1009–1024`, `backend/unix.rs:560`, `backend/iocp/mod.rs:1354`; ordinary N/W2/W3 contracts pass. Queued posts plus pending native I/O can still cause a zero-time OS poll (§10.3); p3 strict host bound unproved |
| D7 Unix Fd integration and Windows opt-in Event helper | V | `driver.rs:906`, `backend/iocp/port.rs:1`; N Unix `external_waiter`; N-W external Event/lifetime tests; `crates/turnloop-contract/src/integration.rs:1` |
| D7 RuntimeOwned WASI / HostCallback web | V for exposed integration | `backend/wasi_p2.rs:699`, `backend/wasi_p3.rs:634`, `backend/web.rs:1`; W2/W3 revision-two contracts, WEB `websocket_bytes_and_schedule_coalescing`, `external_wait_deadlines_schedule_without_spin` |
| D8 bounded lazy configurable pool, four default workers; native DNS/jobs | V for process-wide pool | `blocking.rs:14,94,267,271`, `driver.rs:853,861`; N `shared_pool`; `crates/turnloop-io/tests/streams.rs:169` worker/DNS validation; MODEL pool completion |
| D8 per-loop versus process-wide configuration | M for per-loop option | `blocking.rs:94` native singleton and `types.rs:136` Config expose no per-loop service choice/lifecycle |
| D8 best-effort job cancellation and per-owner completion | V | `driver.rs:876,1073`, `blocking.rs:1`; N `shared_pool`, MODEL pool completion. Arbitrary closure + cancellation flag allocations remain outside zero-allocation core I/O gates |
| D8 files via pool on every platform | P | Unix adopted-file worker path `backend/files.rs:1,379`; N `regular_file_jobs_reuse_pool_storage`. Windows uses per-handle synchronous I/O workers (`backend/iocp/sync_io.rs:1`), not shared file pool. No typed file-open/stat/etc API; WASI files absent |
| D9 !Send local executor, Sleep, futures-io handles, drop cancellation | V | `executor.rs:223,598,823`, `crates/turnloop-io/src/lib.rs:1`; N executor modes `executor_contract.rs:13,98,163,206,296,330`; W2/W3 executor subjects; WEB `executor_websocket_sleep_and_cancel`, `executor_fetch_bytes_and_deadline_abort` |
| D9 rustls unbuffered adapter, no sidecar | V | `protocols/turnloop-tls/src/lib.rs:1`, `src/asynchronous.rs:1`; N TLS `async_tls_alpn_fragmented_plaintext_and_close_notify`; PW2/PW3 TLS; DEP |
| Backend contract revision and surface | V implementation; P documentation freeze | `backend/mod.rs:213` unsafe trait plus rev-2 process/signal/TTY/IPC/deadline methods and additive `resolve:280`; `docs/BACKEND_REVISION_2.md:1`. N/W2/W3/WEB shared contracts run. `trait-v1` is historical, not today's complete surface; revision document should include additive resolver/error/clock contract |

## §5a: multithreading / perry-thread model

| Requirement | State | Evidence / verification |
|---|---|---|
| 1–2 one !Send loop per agent/thread; no main-thread assumption; owning-loop routing | V native primitive; M Perry adoption | `driver.rs:64,150,898,902`; N `many_loops_post` runs four threads × 1,000 posts per peer with payload/owner assertions (`crates/turnloop-contract/src/lib.rs:577`) |
| 3 shared pool, signal fanout, child-exit dispatcher, one external waiter service | V native primitives | `blocking.rs:94`, `backend/signals.rs:1`, `backend/services.rs:1`, `external_wait.rs:75`; N `signals_reach_four_loops_on_four_threads`, `concurrent_256_children_exit_once`, `shared_external_wait_service_routes_and_cancels`; Windows console/process equivalents |
| 3 external waiter fairness under load | P | `crates/turnloop-contract/src/native_surface.rs:273` checks routing/cancel/deadline completion; no sustained starvation/fairness bound per loop. Fixed-capacity helper is implemented, not one OS thread per wait |
| 4 replace Perry ad-hoc threads (stdio, dgram, pty, fs-watch, IPC, signals, N-API, acks) | P | Core primitives above; no fs-watch operation (`types.rs:208`, `driver.rs:435` surface) and no Perry rewiring. Native pty/stdio can be adopted; spawning/configuring a full pty facade remains host work |
| 5 sockets/pipes/servers detach → cancel/drain → attach | P | `driver.rs:800,821`, `backend/unix.rs:624`, `backend/iocp/mod.rs:1417`; N `transfer_inflight`, accept handoff. Windows named-pipe listener and busy connect detach rejected; WASI/web detach rejected (`wasi_p2.rs:693`, `wasi_p3.rs:628`, `web.rs:401`) |
| 5 process handle passing | P | Unix `backend/ipc.rs:125`, Windows `backend/iocp/mod.rs:1265` only transfer sockets. N IPC/child tests prove socket ownership and bytes. General fd/pipe/file/Windows DuplicateHandle transfer promised by the broader wording is absent |
| 6 kernel reuse-port or host accept/handoff | V Linux/macOS/Windows mechanism; U FreeBSD | `driver.rs:435`, `backend/iocp/mod.rs:1167`, `crates/turnloop-contract/src/extended.rs:170,208`; N reuse_port_distribution / accept_handoff_distribution. FreeBSD compiles locally, no distribution run |
| 7 per-agent timers | V primitive; M Perry timer migration | Per-Driver heap `driver.rs:64,385`; N multi-loop/executor tests; P3 host work below |
| 8 loop per Web Worker; postMessage without isolation; optional SAB | P | `backend/web/host.js:133`, `driver.rs:1465`; WEB worker ring/condition tests. `crates/turnloop-contract/tests/web/helpers.js:8,41` workers are JS producers. No non-isolated postMessage Poster or test instantiating a Rust Loop in each worker |
| 9 required multithread tests | P | Native ownership/post/transfer/distribution/signal tests execute; fairness bound and real mobile/off-main lifecycle tests missing; no worker-loop ownership stress under separate wasm memories |

## §5b: protocol crates and adapters

All eight protocol families have production code and adapters; “host adapters not
implemented” is stale. **V** below is protocol/adapter functionality, not complete
npm/Node/Perry compatibility. Real database tests run in Linux **P**, with a WASI
0.2 repeat. Windows/macOS execute portable workspace subjects; they do not run
the same real database services. Browser raw database TCP is unavailable by design.

| Surface | State | Source evidence; executed test and remaining boundary |
|---|---|---|
| Own HTTP/1.1 and HTTP/2 server, HPACK/flow control | V | `protocols/turnloop-http/src/http1.rs:1`, `http2.rs:1`, `asynchronous/mod.rs:1`; H2 strict 147/147; N/P `curl_against_async_http1_server`, `node_https_and_http2_against_async_tls_server`; PW2/PW3 scripted streaming server tests |
| HTTP client pool, redirects, proxy, decompression | V bounded API; P full Fetch/axios/undici parity | `protocols/turnloop-http/src/client.rs:1`, `asynchronous/client.rs:1`, `compression.rs:1`; N/P `node_async_client_redirect_decompression_and_h2`, `node_https_via_authenticated_connect_proxy`, `cancelled_pooled_request_never_reuses_partial_response`; PW2/PW3 codecs/allocations. Full Fetch validation/diagnostics, automatic stacked coding and broader HTTP/2 pool orchestration remain documented gaps (`docs/lanes/proto-http.md:14`) |
| TLS | V | `protocols/turnloop-tls/src/lib.rs:1`, `asynchronous.rs:1`; N/P CA/SNI/ALPN/resumption and invalid-cert subjects; PW2/PW3 portable TLS + channel binding. Existing rustls plaintext allocations are explicitly measured (`README.md:36`) |
| WebSocket | V base protocol; P ws extension parity | `protocols/turnloop-websocket/src/lib.rs:1`, `asynchronous.rs:1`; N/P `node_websocket_against_async_server`, PW2/PW3 `async_upgrade_echo_ping_and_clean_close`; WEB host WS. `README.md:8,32`: no permessage-deflate |
| PostgreSQL connection/pipeline/pool | V implemented subset; P pg facade | `protocols/turnloop-postgres/src/client.rs:65,197,261,314`, `async_pool.rs:1`; P `real_async_required_channel_binding_authenticates_over_ssl`, `real_async_tls_queries_copy_cancel_pool_and_connection_kill`; PW2/PW3 fragmented wire/async/pool and allocation tests. Type/options/error/JS facade parity is not established by these tests |
| MySQL connection/auth/statement/binary/text/pool | V implemented subset; P mysql2 facade | `protocols/turnloop-mysql/src/client.rs:32,170,182,201`, `async_pool.rs:1`; P `auth_prepared_transactions_compression_and_infile`, `common_column_types_and_binary_null_bitmap`, `real_async_tls_rsa_text_binary_multi_results_transactions_pool`; PW2/PW3 async tests. MySQL 9.6 does not verify real mysql_native_password auth; automatic SQL-key statement LRU and full charset/error policy remain (`docs/lanes/proto-sql.md:45`) |
| Redis pipeline/pubsub/reconnect; separately scoped cluster/Sentinel | V implemented subset; P ioredis facade | `protocols/turnloop-redis/src/client.rs:63,104,228`, `cluster.rs:1`; P `real_cluster_slots_shards_moved_ask_and_sentinel`, `real_async_tls_pipeline_pubsub_deadlines_reconnect_cluster_sentinel`; PW2/PW3 async. Host MOVED/ASK routing is implemented; streaming RESP3 remains rejected (`resp.rs:154,183`), full failover/subscription/options parity remains |
| MongoDB OP_MSG, BSON, SCRAM, TLS, SDAM, pool, retry/cursor | V implemented subset; P full driver conformance | `protocols/turnloop-mongodb/src/client.rs:97,394,601`, `connection.rs:1`, `auth.rs:1`; P `real_async_auth_tls_sdam_pool_cursor_retry_and_primary_stepdown`; N SDAM/selection/staleness fixtures, PW2/PW3 async. Full CMAP/retry/transaction/change-stream unified runners absent (`docs/lanes/proto-mongo.md:99`); high-level stream lifecycle absent (`README.md:148`); SRV real DNS unverified in CI |
| SMTP and MIME | V implemented subset; P nodemailer facade | `protocols/turnloop-smtp/src/transport.rs:43,68,129`; P `installed_postfix_smtp_sink_records_message`, `real_async_postfix_delivery`; PW2/PW3 scripted auth/send tests. Owned message/results and no complete nodemailer pooling/stream facade; own MIME builder is an implementation choice instead of the sketch's lettre builder |
| DNS | P overall | Native pool A/AAAA `blocking.rs:271`, p2 `backend/wasi_p2.rs:555` verified by N/PW2 `resolve_and_cancel_reuse_executor_slots`. p3 Unsupported explicitly tested. SRV/TXT `crates/turnloop-io/src/dns.rs:57,119` exists; its only real lookup test `tests/streams.rs:215` is ignored without a CI runner |
| child_process/container, unused cron dependency removal | V turnloop process primitive; M Perry substitutions | `driver.rs:522`; N process tests. Perry container wrappers and removal of `tokio-cron-scheduler` are P2/P8 work, not core omissions |
| Linux tray/MPRIS/zbus runtime replacement | M in this repository | `DESIGN.md:290` proposes an async-io alternative, but §5b zero-runtime decision and binding project rule forbid it. Requires turnloop-driven D-Bus path or an explicitly scoped surface decision, not a banned sidecar |
| Zero tokio/hyper/h2/other runtime dependencies | V repository; M Perry proof | `scripts/ci/no-tokio.sh:1`, `deny.toml:1`; DEP audits default/all graphs, normal/build/dev, target matrix plus all-target graph. No exemption was added |

## §§6–9: surface, platforms, liveness and host boundary

| Requirement | State | Evidence / verification |
|---|---|---|
| §6 API sketch | P as a capability checklist | Non-normative names differ: Config/Driver/OpResult replace some sketch names; `types.rs:136,208`, `driver.rs:64,435`, `backend/mod.rs:100,213`. Timers/net/native services/executor exist; FsRequest/FsResult-style filesystem surface absent. Do not require literal sketch signatures |
| §7.1 Linux ns wait, eventfd, fallback, pidfd/SIGCHLD, files/DNS pool | V core mechanisms; P full files | `backend/epoll.rs:117,126,174`, `backend/files.rs:1`, `blocking.rs:271`; N-L/N-A all six modes, native surface + no-spin/alloc tests. No tested old-kernel fleet beyond forced fallbacks |
| §7.2 kqueue EV_CLEAR/EVFILT_USER/PROC/SIGNAL; macOS embedding | V macOS; U other selected BSD/Apple OSes | `backend/kqueue.rs:23,78,175,199`, `backend/signals.rs:74`; N-M `pending_signal_cannot_outlive_kqueue_unsubscribe`, `external_waiter`; local FreeBSD/mobile compile only |
| §7.3 IOCP ownership/cancellation, pipes, processes, console, Event | V on Windows 11 | `backend/iocp/mod.rs:1`, `process.rs:1`, `sync_io.rs:1`, `signals.rs:1`; N-W `windows_lifetimes`, `windows_console`, allocation suites. Arbitrary overlapped regular files rejected at `sync_io.rs:389`; rejected `.bat`/`.cmd` spawn is intentional policy, not missing CreateProcess |
| §7.3 high-resolution timer / minimum version / VM precision | P | `backend/iocp/timer.rs:27,58`; N-W timer bounds and no-spin pass. NT packet choice documented `DESIGN.md:569`; minimum-version exports/VM distribution/cycle attribution not verified |
| §7.3 guaranteed bounded lifecycle | P | `docs/BACKEND_REVISION_2.md:181`, `backend/iocp/README.md:43`: after exit-watch cancellation, release/drop can synchronously await process teardown. Pending watch close semantics are tested; a strict bounded-turn interpretation needs an explicit policy and test |
| §7.4 p2 single poll, TCP/UDP, timers, stdio | V ordinary contracts | `backend/wasi_p2.rs:604,703`; W2/PW2/P. Host precision, not a millisecond guest floor; no wasi:http outgoing adapter found (socket-based HTTP exists, satisfying matrix's alternative) |
| §7.4 p3 futures/streams/waitable-set multiplexing | P | `backend/wasi_p3.rs:539`, `wasi_p3/return_storage.rs:7`; W3/PW3 ordinary tests pass. `docs/upstream/wasi-p3-wait.md:25` retains four unresolved host-bound/portability issues. `yield_blocking` before Now polling and synchronous subtask cancel are not proven bounded |
| §7.4 generic blocking jobs inline or host async; filesystem | M | `blocking.rs:259` rejects wasm jobs, no host async-job ABI; no filesystem operations in p2/p3 Backend. Inline arbitrary jobs would itself violate a bounded turn; requires design/implementation resolution |
| §7.4/§7.6 WASI DNS and TTY size | P DNS; M size | p2 resolver V, p3 default Unsupported (`backend/mod.rs:280` + no p3 override). TTY defaults `backend/mod.rs:249,253` reject both; current bindings do not provide the claimed portable terminal size method |
| §7.5 web Now-only scheduling/timer/fetch/WS | V bounded exposed API; P complete host interface | `backend/web.rs:208,236`, `backend/web/host.js:1`; WEB 13 subjects. Fetch is whole-body GET bytes without general status/header/body-stream options; `host.js:61` materializes arrayBuffer before size rejection |
| §7.5 ReadableStream/WritableStream imports; workers without isolation | M | No general stream open/operation or postMessage Poster; `types.rs:208`, `backend/web/host.js:133`; SAB condition/ring is tested, full per-worker loop not. All web turns are Now-only, including workers (safe stricter API than optional blocking-worker wording) |
| §7.6 explicit unavailable OS capabilities | V rejection semantics | WASI/web processes/signals/IPC/raw browser TCP/UDP return Unsupported as appropriate; W2/W3 revision-two rejection tests; WEB `revision_two_single_agent_contracts`, `capability_errors_and_oversize_response_are_terminal`. Optional host-mapped OPFS is absent, not a portable OS feature |
| §7.6 cost measurement matrix | P | Linux Callgrind gate exists; native counter harness `crates/turnloop-bench/src/counter.rs:1` exists. No required perf user/kernel, Windows cycle, WASI fuel or browser profiler regression gate; compile-only targets lack runtime evidence |
| §8 ref/unref and O(1) liveness | V | `driver.rs:331,355`, `crates/turnloop-contract/src/lib.rs:382,830`; N `liveness`, `terminal_delivery_liveness`; W2/W3 applicable shared contracts; WEB revision-two contracts. Does not replace Perry's own counters until P0/P3 |
| §9 JS values/GC roots, Node errors, phases, keep-alive, Send-only jobs | V turnloop boundary; M Perry implementation | Tokens/error types and Rust Send closures exist (`types.rs:8,65`, `driver.rs:853`). `DESIGN.md:455` assigns JS/GC/phase/error policy to Perry; no Perry source tree or GC-stress runs here |
| §9 P0 wait hooks and perry-ffi async ABI v2 | M here | `DESIGN.md:465,473`; no `js_register_wait_driver` or Perry perry-ffi async ABI implementation in tracked production source. Rust driver is not that C ABI |

## §§10–11: performance, contracts and CI

| Requirement | State | Evidence / verification |
|---|---|---|
| §10.1 no allocation per core read/write/timer/accept after warm-up | V tested native/p2/p3-release paths | `crates/turnloop-contract/tests/allocations.rs:169,243,306,878,1065,1427`; N/W2/W3 nonzero workloads and allocator calibration. WEB measures Rust operations only; JS host allocations are outside its allocator |
| §3.3 broader per-operation zero allocation, including protocols/jobs | P | Generic closure/Arc jobs allocate; rustls 400 allocations/100 bidirectional records (`protocols/turnloop-tls/README.md:36`), HTTP owned heads and SMTP owned results allocate. WASI MongoDB compressed borrowed-command zero gate actually fails (`tests/allocations.rs:139`); not selected by its `Cargo.toml:41` wasi-tests |
| §10.2 notifier zero wake syscall while running | P gate | `notifier.rs:33,49`, shared `notify_running` at contract `src/lib.rs:47`; N counter assertions and MODEL pass. Counter measures wake paths, not an independent strace/ktrace/ETW syscall-counting CI harness; none found in workflows/scripts |
| §10.3 ≤1 OS wait; none with queued completions | P, reproduced violation of second clause | `driver.rs:1013–1024`, `backend/unix.rs:586`; local queued-post + idle-UDP probe gives **os_waits=1**, completions=1. Native progress test intentionally polls amid timer/post backlog. No change to requirement adopted here |
| §10.4 no fixed ticks / no wait floor | V for ordinary backend waits | `backend/epoll.rs:117`, `kqueue.rs:199`, `iocp/timer.rs:1`, `wasi_p2.rs:655`, `wasi_p3.rs:559`; N/W2/W3 no-spin and timer bounds; WEB timer scheduling. Host scheduling clamps remain platform limits |
| §10.4a idle socket 0.5/2/10 ms: ≤2 turns, ≤1 zero-event wait | P overall; V existing subjects | Shared `crates/turnloop-contract/src/lib.rs:958`; N/W2/W3 no-spin, WEB idle-WebSocket timer test; native service + HTTP quiet tests. WASI counters count private deadline events as native work (`wasi_p2.rs:684`, `wasi_p3.rs:619`), contrary to `backend/mod.rs:204`. p3 host bound remains unproved; no actual spin claimed from this counter defect |
| §10.5 all operation instruction budgets against bridge/hand epoll | P | `crates/turnloop-bench/benches/instructions.rs:1`, `benchmarks/instructions.json:1`, `scripts/ci/instructions.py:1`; PERF four cases (one control), cgu=1/interleaved rounds. Read/write/accept/blocking, kernel counters, bridge attribution and numeric X% target absent |
| §10 methodology: subject executes, controls, fresh interleaved rounds | V within current gate | `scripts/ci/instructions.py:1`; PERF control 527 and all three workload cases nonzero; `scripts/ci/run-tests.py:1` rejects empty required suites; WEB real fixture counters |
| §11 required PR OS/WASI/browser jobs | V main targets; M FreeBSD nightly | `.github/workflows/ci.yml:2,48,106,193,232,508`; 37 required successful jobs; no `schedule` trigger/FreeBSD arm |
| §11 bounded, notify, cancel/close/error, liveness, timer, integration, buffer contracts | V sampled native and supported wasm subsets | N/W2/W3/WEB named subjects above; `crates/turnloop-contract/src/lib.rs:13,25,47,270,339,382,413,767`, `src/integration.rs:1`. Explicit target exclusions are in marker inventory, not counted as passes |
| §11 Loom and Miri | V enumerated models; P broader pure-Rust coverage | `crates/turnloop/Cargo.toml:46` models + two Miri filters; MODEL six models/two tests. Remaining executor/queue/buffer pure-Rust paths do not all run under Miri |
| §11 fault injection | P | Native cancellation/backpressure/EINTR/error tests and IOCP lifecycle injection exist (`backend/iocp/process.rs:562`, `backend/unix.rs:892`, contract `tests/allocations.rs:243`). No exhaustive per-backend EAGAIN/reset/exhaustion/signal-storm campaign or protocol fuzz CI |
| §11 fd/handle/pool-thread counts before/after every test | P | Native `tests/lifetimes.rs:24`, Windows `tests/windows_lifetimes.rs:413` and specific console/file cases measure resource lifetimes. Not every test measures fd/handle/thread deltas; singleton pools intentionally survive loops |
| §11 overnight echo/timer/process churn per OS | M | No schedule trigger at `ci.yml:2`; existing stress tests are bounded PR tests, not long-running nightly soak. Dependency seven-day soak is a separate, active policy |
| §11 every feature/config has a CI arm | P | `scripts/ci/feature_modes.py:1` checks registered features and required modes; default/all/native fallback/executor/worker/experimental-p3/pure-Rust-zstd are exercised. Not every target/combination is run; wasm suite allowlists omit MongoDB compressed allocations and native-only cfg tests cannot establish wasm behavior |

## §§12–15 and Appendix A

| Requirement | State | Evidence / verification |
|---|---|---|
| §12 P0 exact wait hooks + O(1) Perry liveness | M here | `DESIGN.md:526`; no Perry implementation/test. Core hooks/counters ready; queued-poll/no-spin issues still qualify integration |
| §12 P1 sockets + Windows pipe IPC | M here | `DESIGN.md:527`; core N-tested handles ready, Perry bundling/ext-net/rooting absent |
| §12 P2 processes/pty/stdin/dgram/signals | M here | `DESIGN.md:528`; native primitives tested, Perry thread removal/JS parity absent |
| §12 P3 per-agent JS timers / Node phases | M here | `DESIGN.md:529`; no JS scheduler changes in turnloop (correct ownership boundary) |
| §12 P4 crypto/image/compression blocking work / ABI v2 | M here | `DESIGN.md:530,473`; process-wide Rust pool ready; JS-safe result delivery and ABI not implemented here |
| §12 P5 own HTTP server/TLS/WS | M Perry wiring; V Rust prerequisites | `DESIGN.md:531`; N/P/H2/PW2/PW3 protocol evidence above |
| §12 P6 fetch/axios/undici/SMTP | M Perry wiring; P parity prerequisites | `DESIGN.md:532`; Rust transport clients tested, JS/browser semantic facade still separate |
| §12 P7 four DB clients | M Perry wiring; P conformance prerequisites | `DESIGN.md:533`; real Rust native/p2 server suites pass; complete npm-facing compatibility not established |
| §12 P8 remove runtime from every Perry target/crate | M here | `DESIGN.md:534`; turnloop DEP gate cannot certify Perry's transitive graph or unified stdlib paths |
| §12 every phase gap suite, GC stress during I/O, cgu=1 A/B, Windows arm | M here | `DESIGN.md:518`; no compiled-Perry gap/GC/protection/seed/A-B jobs in `.github/workflows/ci.yml:23` |
| §13 standalone MIT repository / 0.x release artifacts | V repository; U archive provenance | Root `Cargo.toml:1`, `LICENSE:1`; alpha.2 GitHub release exists. Its statement that 12 crates were published is release evidence, not independently checked archive checksums for all 12 in this audit |
| §13 SECURITY.md | M | `DESIGN.md:540`; tracked-file inventory contains no root or .github SECURITY.md |
| §13 Trusted Publishing/OIDC and release automation | P | `.github/workflows/release.yml:1,63,109`, `scripts/ci/release.py:1`; automation unit tests in LINT. Exact-main release verify succeeds, release-pr fails empty app/client ID, publish skipped (run 34919821444); successful OIDC exchange unverified |
| §13 breaking-minor policy; Perry exact pin + one-time age/checksum record | P process/tooling; M Perry adoption proof | `RELEASING.md:1`, `scripts/ci/verify-crate.py:1`, `DESIGN.md:541`; no Perry bump/checksum/source-commit record audited; don't infer from a GitHub tag |
| §13 seven-day dependency soak, no forbidden runtimes, target deps, optional layers | V | `.cargo/config.toml:1`, `scripts/ci/soak.py:1`, `scripts/ci/policy.toml:17`, `crates/turnloop/Cargo.toml:14`; DEP + LINT. Existing rustls 0.23.45 security exception has `expires = 2026-09-21`; policy is not disabled |
| §14 M0 review/sign-off + six-platform CI skeleton | P | Design and CI exist; no auditable explicit sign-off on every D1–D9/§5a/§5b decision found |
| §14 M1 core/all backends/spike decisions + baseline | P | Required runtime jobs and PERF exist; p3 bounds, compio comparison with instruction numbers and Windows minimum-version evidence incomplete |
| §14 M2 I/O/multiloop/alloc/syscall gates | P | I/O/transfer/alloc contracts V; independent syscall traces and broad transfer gaps remain |
| §14 M3 native services/files/DNS/jobs | P | Native services V; typed/WASI files, p3 DNS and pool option remain |
| §14 M4 executor/TLS/WS/HTTP interop | V bounded Rust scope | N executor/interop, H2, PW2/PW3, WEB; complete Node-facing parity belongs to later milestones |
| §14 M5 Perry P0/P1 | M here | No Perry gap/GC/A-B evidence |
| §14 M6 HTTP/SMTP and fetch/axios parity | P | Rust clients V; fetch/axios parity suite absent |
| §14 M7 DB clients and conformance | P | Real-server suites V; complete driver compatibility/conformance not established |
| §14 M8 Perry P2–P8/tokio removed | M here | Repository DEP success is not Perry completion |
| §15 Q1 name / Q9 zero tokio | V | `Cargo.toml:1`, DEP, published alpha release |
| §15 Q2 sys layer / Q3 Windows timer | P resolution | Own backends + NT timer chosen; `spikes/iocp/EVALUATION.md:1`, `DESIGN.md:569`. Compio comparison instruction proof and minimum-version/VM evidence remain |
| §15 Q4 lease sizing/lifetime | V code; P design wording | `buffer.rs:127,157,177`, Config sizes; N retained lease test. Document explicit release lifetime; no next-turn expiry |
| §15 Q5 io_uring / Q7 advanced DNS | U future choices | `DESIGN.md:571,573`; intentionally deferred optimization/DoH, not required missing 0.x backends. Basic p3 DNS is still required |
| §15 Q6 mobile CI | M | No simulator/emulator arms, despite local cross-clippy results |
| §15 Q8 protocol location | V implementation choice | Sibling `protocols/` workspace members at root `Cargo.toml:1`; N/P/PW2/PW3 build and execute |
| Appendix A references | U external context | Reference list at `DESIGN.md:580`; external papers/other repositories not re-audited as implementation proof |

## Proposed clarifications (not adopted)

Keep DESIGN authoritative and its binding gates intact. Resolve the tension
between §10.3 “none” and the current fairness-driven zero-time poll with queued
posts; implement the rule or explicitly review a specification change with
starvation/no-spin evidence. Normalize WASI private timeout accounting before
calling the numerical rule complete. Define a bounded p3 host contract rather
than hiding host yield/cancel latency in a one-call counter.

Clarify explicit BufLease release, current trait revision including resolve,
shared versus per-loop pools, Windows synchronous-worker/file and transfer limits,
WASI job/filesystem/TTY capabilities, non-isolated worker routing, and the scope
of zero allocation (Rust core, codecs, owned outputs and JS host separately).
The §9 claim that P0 needs no other Perry changes conflicts with §12 P0 requiring
precise deadlines and O(1) liveness in the same change; retain the stronger §12
requirements. The tray/MPRIS suggestion to use async-io conflicts with the binding zero-runtime
rule and must not become an implementation exception. None of these observations
changes the seven-day soak or the no-spin/zero-tokio requirements.
