# Open markers and exclusions at a0fbb1b

The complete, filterable inventory is [audit/markers.tsv](audit/markers.tsv).
It is tied to **the original audited commit**, including its old integration and
root lane reports; source links are immutable SHA links. This avoids losing or
misaddressing evidence when those two reports are rewritten in this lane.

## Collection and disposition rules

Scanned 986 UTF-8 tracked files from `git archive a0fbb1b`, including all 68 Markdown/
README documents, every report under `docs/lanes/`, production source, tests and
standalone spikes. Binary fixtures were not treated as text. The new audit files
are outputs, not inputs to their own inventory.

| Kind | Rows | Scope |
|---|---:|---|
| `open-code` | 27 | TODO, FIXME, todo!, unimplemented! across tracked text; includes example/comment matches |
| `doc-progress` | 339 | Case-insensitive TODO/FIXME/UNRUN/pending/not yet/not implemented in Markdown and README files |
| `unsupported` | 192 | All Rust/JS Unsupported/unsupported helper and streamed/offset rejection matches, including negative tests, comments and enums |
| `ignored-test` | 20 | Every Rust `#[ignore]` / `#[ignore = ...]`; runner disposition supplied |
| `cfg-selection` | 608 | Every Rust cfg/cfg_attr/cfg! and Cargo target cfg table; multiline predicates retained |
| `manual-doc-claim` | 4 | Additional stale adapter/CI/resolver claims that use different wording |
| **Total** | **1,190** | One row per location/kind; a source line matching two kinds intentionally appears twice |

Each row contains `file`, `line`, `kind`, the marker text, `status`, assessment,
nearby context and immutable `source` URL. **STILL TRUE** is qualified as behavior,
policy, historical local fact, selection or active limitation. **STALE** means the
claim no longer describes this main tree/current CI. **MIXED** keeps separate
live limits and superseded execution claims. “Historical local; stale as current
coverage” preserves both facts: a lane could not execute Windows locally, and
Windows production tests now execute in required CI.

`pending` often means an in-flight operation or queue state. Those rows are
explicitly marked **not open work**. A cfg predicate is not automatically a gap;
the assessment explains which target/job exercises it, which alternatives are
excluded, and what remains unverified. An Unsupported return is not automatically
wrong: the browser has no raw TCP, and WASI has no process/signal API.

Evidence keys N/W2/W3/WEB/P/PW2/PW3/MODEL/PERF/DEP/GATE refer to
[exact-main jobs](INTEGRATION_REPORT.md#1-ci-evidence-actually-read). I01–I17 refer
to [remaining work](REMAINING_WORK.md). Counts in this inventory are lexical
matches, not claims of that many defects or tests.

## 1. TODO / FIXME / panic markers

All 27 matches are inherited from `protocols/turnloop-zstd-decoder` (the published
ruzstd fork). There are **no FIXME matches**, and no TODO/todo!/unimplemented!
markers in the other production crates.

- **Two active unimplemented! branches:**
  `src/encoding/frame_compressor.rs:203` for unsupported compression levels, and
  `src/encoding/blocks/compressed.rs:349` for excessive literal count. These are
  encoder branches, not proof that HTTP decompression reaches a panic. The encoder
  is publicly reexported (`src/encoding/mod.rs:11,24`); I17 records its scope/error work.
- **Four todo! examples/comments:** `UPSTREAM-README.md:71,93`,
  `src/decoding/streaming_decoder.rs:39`, `src/bit_io/bit_writer.rs:365`.
  They are not executable production I/O placeholders.
- **Twenty-one TODO comments:** decoder failure state (`block_decoder.rs:27`),
  buffer/drain/ring handling, block/sequence validation, dictionary state, frame
  window/content size, checksums, offset/table reuse and encoding heuristics.
  The TSV retains each exact location. Optimization suggestions and missing
  error-state handling should not be given the same priority; see I14/I17.

## 2. Ignored tests and actual runners

**19 of 20 have a required CI runner.** Their ordinary `cargo test` ignored status
is still true; a broad “these service bodies have never run” claim is stale.

| Source / ignored locations | CI execution |
|---|---|
| `crates/turnloop-io/tests/streams.rs:215` | **No runner:** `native_srv_txt_records_through_blocking_pool` requires external DNS; not selected by the protocol metadata. I11 controlled fixture needed |
| HTTP `tests/interop.rs:154,440`, `tests/asynchronous.rs:350` | N interop and P, private Node/curl fixtures; included ignored subjects |
| MongoDB `tests/real_mongodb.rs:315`, `tests/async_server.rs:36` | P: native real MongoDB plus p2 async server suite (native bootstrap supplies state) |
| MySQL `tests/server.rs:162,300,348`, `tests/async_server.rs:59` | P: native real tests; p2 async server repeat. MySQL 9.6 fixture does not establish mysql_native_password auth |
| PostgreSQL `tests/server.rs:225,372,467`, `tests/async_server.rs:59,274` | P: real queries/types/COPY/cancel/pool and required channel binding; p2 async server repeat |
| Redis `tests/real.rs:136,354`, `tests/async_server.rs:76` | P: real TLS, commands, cluster/Sentinel, async reconnect; p2 async server repeat |
| SMTP `tests/smtp.rs:553`, `tests/async_server.rs:31` | P: actual smtp-sink delivery and async delivery; p2 async server repeat |

Protocol paths in this table are beneath `protocols/turnloop-<family>/`.
`--include-ignored` appears in the test runner's selected target invocations;
`.github/workflows/ci.yml:328,334` launches real native/p2 services. It is not
applied to all ignored tests in the workspace. In particular, it misses SRV/TXT.

## 3. Unsupported returns worth scheduling

The TSV also includes validation errors and mock providers so none are silently
omitted. These are the design-relevant groups:

| Group / source | Current disposition |
|---|---|
| `blocking.rs:259`, default `backend/mod.rs:280` resolver, no p3 override | **Live gaps:** WASI host jobs and p3 DNS; I05/I09. Native pool and p2 resolver already work |
| `backend/mod.rs:249,253` TTY defaults | Browser rejection intended; WASI terminal-size row is unsupported and needs a design/capability resolution (I16) |
| `backend/wasi_p2.rs:693`, `wasi_p3.rs:628`, `web.rs:401` | Detach unavailable on single-agent backends. Native transfer tests do not prove transfer between wasm instances |
| `backend/ipc.rs:125,285`, `iocp/mod.rs:1265,1424` | Socket-only IPC transfer; Windows listener/busy-pipe detach gap. I10 |
| `backend/iocp/sync_io.rs:389` | Arbitrary overlapped regular file adoption unsupported; synchronous/file worker surface and shared-pool design differ. I06 |
| `backend/web.rs:111,135,274` | Isolation/SAB required for worker API; multishot read/writev and general stream interface missing. I07 |
| `backend/web.rs:208,226,236,246,320`; WASI process/signal/reuse-port branches | Mostly **intended** platform/method restrictions. Now-only browser turns; raw TCP/process/signal/local IPC unavailable |
| Windows signal, uid/gid, reuse-port and resource-kind branches | OS limitations or invalid-resource preconditions; explicit rejection is correct. Windows timer export/version assumptions still need I10 evidence |
| Redis `src/resp.rs:154,183` | Streamed RESP3 unsupported; scope in I15 |
| Other protocol “unsupported” errors | Version/auth-plugin/SCRAM-extension/column/ALPN/URL/coding/TXT/offset validation, not automatically unfinished supported behavior. Compare to the pinned Perry compatibility contract (I15) |

Core paths above are under `crates/turnloop/src/`. No filesystem API is present to
return Unsupported; it is missing at the public surface and is separately I06.

## 4. cfg exclusions and platform implications

- **Native test cfgs:** Apple/Linux/Android/FreeBSD predicates exclude other BSDs
  even though `build.rs:8` selects kqueue for them. Native Windows bodies have
  their own test binaries. Linux/macOS test success never counts those empty
  Windows binaries as executed subjects.
- **Native fixtures/reference libraries:** whole SQL server/Redis/MongoDB/SMTP
  native test files, Node peer integration and C-reference zstd interop are
  excluded on wasm. Portable/async WASI tests are selected separately; real
  async-server tests run p2 in P, not p3. WEB does not run all protocol codec suites.
- **Allocation exclusions:** core WASI is gated in release; p3 return-storage
  allocation interception is disabled in debug. Native file/IPC/process/console
  allocation cases cannot be inferred from wasm counts. Quiet-deadline accounting
  at `crates/turnloop-contract/tests/allocations.rs:1019` excludes WASI (I02).
  MongoDB compressed allocations are omitted by the WASI manifest allowlist
  even though the allocation test itself compiles and fails (I03).
- **Examples:** SQL/Redis/SMTP/MongoDB `examples/turnloop.rs` have empty Windows/web
  mains. Windows production adapters still execute portable tests; compiling these
  examples does not demonstrate a successful example session.
- **Feature coverage:** required native default/executor/all-feature/fallback,
  web-worker, experimental-p3 and pure-Rust-zstd configurations are represented.
  `cfg(test)`, `cfg(loom)`, debug/release, docs-only and no-std/optional decoder
  branches are catalogued without claiming the entire combination space ran.
- **Outside matrix:** FreeBSD/mobile local cross-checks and Android/musl failures
  are in the integration matrix; no CI runtime is inferred. `wasip1-threads` in
  the dependency graph audit has no corresponding production backend.

## 5. Stale documentation versus live work

| Claim/location at original snapshot | Disposition |
|---|---|
| Root/core README platform tables (`README.md:20,21`, `crates/turnloop/README.md:19,20`) call WASI/web standalone spikes with production work pending | **STALE:** production W2/W3/WEB execute; p3 still experimental |
| Root README `:12`, core README `:10` say adapters still integrating | **STALE** as absence/completion prerequisite: all protocol async adapters exist; full Perry facade still outstanding |
| Old integration report `:382` pending gate section | Windows/WASI/browser/SQL/current instruction jobs **STALE as unrun**; long soak, broader perf and Perry migration still live |
| CONTRIBUTING `:282` pending production contracts and `:399` failing gates | **STALE:** required GATE succeeds; p3 deeper contract caveats remain |
| HTTP README `:53–55` says all WASI needs literal IPs | **STALE for p2**, still true of current p3's lack of a core resolver |
| SQL README browser execution `turnloop-mysql:160`, `turnloop-postgres:172` | **STILL TRUE:** wording refers to browser codec runtime, not WASI. Core browser tests do not prove SQL browser codecs or raw database access |
| proto-sql lane real SQL/auth/TLS/COPY/pool UNRUN rows | Historical sandbox records valid; **STALE as present CI coverage**. Real native/p2 effects and required PG channel binding execute |
| Old Windows/spike lane UNRUN records | Historical scope valid; **STALE as present production status**. Do not claim every old standalone spike was rerun. `spikes/iocp/WINDOWS_RESULTS.md:24` records individual spike runs, including APC timer/draft failures. NT minimum-version/VM, ETW/cycles and extended soak remain live |
| wasm2-wasm3 lane `:166` inherited ~33× idle regression | **STALE:** current PERF idle is below committed baseline; no rebaseline in this audit |
| proto-fix1 `:144–159` WASI compressed MongoDB allocation failure | **STILL TRUE:** p2 reproduced; old p3 failure not cleared by selected PW3 suites |
| proto-http fuzz/Node parity, proto-mongo full unified runners/change streams, BSD/mobile/runtime measurement limits | **STILL TRUE** within stated scope; I12–I15 |
| Local SQL shmget/initializer or Chrome startup failures | **STILL TRUE as historical local records**; no new local service/browser attempt in this lane and no global UNRUN claim |

The TSV provides individual verdicts for all lane/doc matches; this table only
summarizes high-impact groups. Active documentation can be reconciled in I16;
historical lane ledgers should not be rewritten to invent local executions.
