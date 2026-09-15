# gap-audit lane report

**Complete — documentation only.** Audited main
`a0fbb1bf9a100bb3215219f54301b9ee82ffe71f`, branch `lane/gap-audit`, 2026-09-15.
No production code, tests, CI, gates, dependencies, soak policy or Git metadata
changed. The integrator can commit this working tree.

## Implemented

- Rewrote [docs/INTEGRATION_REPORT.md](docs/INTEGRATION_REPORT.md) around current
  implementation and actual required CI results, removing stale global UNRUN claims.
- Added [docs/DESIGN_AUDIT.md](docs/DESIGN_AUDIT.md), covering every design section,
  D1–D9, §5a model, protocols, platform/cost matrix, all hard rules, CI clauses,
  release/security/soak, P0–P8, M0–M8 and open decisions with classifications and
  source lines. Context/non-goals are distinguished from missing requirements.
- Added [docs/REMAINING_WORK.md](docs/REMAINING_WORK.md): 17 concrete internal
  work items with priorities, acceptance criteria and verification platforms;
  separate Perry P0–P8 and cross-agent integration work derived from DESIGN/docs.
- Added [docs/AUDIT_MARKERS.md](docs/AUDIT_MARKERS.md) and
  [1,190 individual marker/cfg records](docs/audit/markers.tsv): 27 TODO/panic/example
  markers, 339 documentation-progress matches, 192 Unsupported matches, 608 cfg
  selections, 20 ignored tests, four additional stale claims. All have dispositions
  and immutable source links. Historical lane records remain historical facts.
- Added [docs/AUDIT_EVIDENCE.md](docs/AUDIT_EVIDENCE.md),
  [CI ledger](docs/audit/ci-jobs.tsv) and
  [local command/hash ledger](docs/audit/local-verification.tsv).
- Read job metadata and **111 successful job logs** across requested runs
  34918732035 / 34916140575 and exact-main run 34919345014. Each has 37 successful
  required jobs plus skipped optional self-hosted Windows. Exact main and PR #13
  head have identical trees. Named subjects, nonzero counts and source exclusions
  were checked; green job labels alone were not treated as full design proof.

## Main findings

Required CI really executes Windows production contracts, both WASI backends,
Chrome/Firefox/Node, real native + WASI 0.2 SQL/Redis/MongoDB/SMTP, 147/147 strict
h2spec cases, six Loom models, two Miri filters and the committed instruction gate.
Windows follow-up tests from #13 are green. Seven-day dependency soak stays active,
with one existing reviewed rustls security exception; zero-tokio passes.

Newly reproduced gaps outside those gates:

1. macOS queued post + idle pending UDP performs **one OS poll** despite §10.3's
   zero-wait-with-queued-completions requirement. Post delivery was asserted.
2. Existing WASI 0.2 MongoDB compressed allocation test fails: **1,000 allocations
   per 1,000 commands**. Calibration and rows prove execution. WASI CI omits this
   target; the older p3 failure is not cleared by current selected suites.
3. Extra target strict Clippy fails for **Android test thread-local initialization lint**
   (`missing_const_for_thread_local`) and **Linux musl time_t deprecation**.
   FreeBSD and five Apple mobile target variants pass core/contract cross-clippy;
   their runtime is UNRUN and absent from reviewed CI.

Other concrete gaps: WASI private-deadline wait accounting; p3 bounded host wait/
cancellation proof and DNS; filesystem/watch API and WASI files; per-loop pool
option/wasm host jobs; broader transfer and Windows lifecycle/version boundaries;
web streams and non-isolated worker routing; syscall/full-operation instruction
and long-soak/fault/leak gates; controlled SRV/TXT DNS runner; scoped protocol
compatibility. **One ignored test has no CI runner**, the real SRV/TXT lookup.
`SECURITY.md` is missing. Exact-main release automation fails on an empty GitHub
App/client ID; its CI verification succeeds and publishing is skipped. Alpha.2
exists, but these reviewed runs do not demonstrate successful OIDC publication.
Perry hook/ABI/GC/JS/tokio-removal phases have no implementation evidence here;
no claim is made about an uninspected Perry checkout.

## Verification commands

Native host: macOS arm64; pinned nightly-2026-08-20 and stable 1.97.1.
`CARGO_BUILD_JOBS=4` for the sequential quality/cross-target command batch.
All extra target Clippy commands check core/contracts, not complete protocol
C-library linking. Cross-clippy is compile-only. Raw temporary logs reside in
`/tmp/gap-audit-verification/`; persisted hashes and relevant failure text are in
[the evidence report](docs/AUDIT_EVIDENCE.md).

| Command | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test --workspace` | PASS: 251 passed, 13 ignored; default parallelism |
| `bash scripts/ci/no-tokio.sh` | PASS |
| `python3 scripts/ci/soak.py` | PASS |
| `python3 scripts/ci/feature_modes.py` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-unknown-freebsd -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-linux-android -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL: const thread-local test lint; runtime UNRUN |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-apple-ios -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-apple-ios-sim -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-apple-tvos -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-apple-visionos -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-apple-watchos -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-unknown-linux-musl -- -D warnings -D clippy::undocumented_unsafe_blocks` | FAIL: deprecated libc::time_t; runtime UNRUN |
| `cargo test --locked -p turnloop-mongodb --release --target wasm32-wasip2 --test allocations --config 'target.wasm32-wasip2.runner="wasmtime run"'` | FAIL: allocation workload traps at assertion; calibration PASS |
| Same command plus `-- --nocapture` | FAIL: exact 1,000/1,000 zlib allocation assertion; repeated once for diagnostic output, not to obtain a pass |
| `cargo run --offline --manifest-path /tmp/gap-audit-probe/Cargo.toml` | Initial scratch compile FAIL: nonexistent recv_from spelling, subject UNRUN. Corrected scratch to public recv; executable FAIL: delivered post, os_waits=1 instead of 0. Repository unchanged |
| `wasmtime --version` | PASS: installed 44.0.0 used for extra p2 allocation probe; CI uses pinned 46 |
| `gh run view <run> --json headSha,headBranch,status,conclusion,event,url,jobs` for 34918732035, 34916140575, 34919345014 | PASS: full job/SHA evidence; all 37 required jobs successful in each |
| `gh api repos/PerryTS/turnloop/actions/jobs/<job>/logs` for all successful jobs in those runs | PASS: 111 logs; all IDs/URLs/hashes in CI ledger |
| `git diff --stat 163fd0c HEAD` | PASS: empty, same PR-head/main tree |
| `gh run list --workflow release.yml`; `gh run view 34919821444 --json headSha,url,jobs`; release-job log API reads | Retrieval PASS; remote workflow FAIL: empty App/client ID. Verify job PASS, publish SKIPPED. Earlier release-pr job 104215916851 has same failure |
| `gh api repos/PerryTS/turnloop/releases` and repository main metadata | PASS: alpha.2 release exists; no archive-checksum/OIDC success claim |
| Snapshot extraction with `git archive a0fbb1b`, source/marker/metadata inventory | PASS: 986 UTF-8 files, 68 Markdown/README documents; 1,190 classified records |
| `python3 /tmp/gap-audit-check-docs.py` | PASS final: links, source lines, complete marker coverage and CI-record invariants. Initial checker flagged intentionally absent/external names; corrected its negative-reference handling and removed an invented ABI spelling from the draft |
| `python3 scripts/ci/check-paths.py` | PASS: 1,451 tracked files, 245 Rust/Cargo references |
| `git diff --check` and final documentation whitespace/link checks | PASS |
| `git status --short`, `git rev-parse HEAD`, documentation-only diff scope | PASS: audited HEAD unchanged; only Markdown/TSV report artifacts changed |

### Explicitly UNRUN in this lane

- Linux/Windows native runtime: no host; hosted CI evidence cited separately.
- FreeBSD/Android/iOS/tvOS/visionOS/watchOS/musl runtime: no suitable host/emulator.
- Local real SQL/server-runner and Docker suites: no attempt in this audit;
  supplied shmget/MySQL sandbox constraints remain. Real hosted P/p2 tests ran.
- Local Chrome/Firefox/Node production suite: not run in this docs lane; WEB CI
  proves all three. No local browser launch is claimed.
- Current p3 compressed MongoDB allocation rerun: not run; older failure recorded
  explicitly. Other local full WASI contracts not rerun; W2/W3 logs provide evidence.
- Independent syscall traces, complete operation perf/kernel/cycle/fuel/profile
  budgets, minimum-Windows/VM matrix, prolonged soak/fault/fuzz campaigns.
- Perry checkout/integration/GC stress/A-B and actual consumer archive verification.
- Publication, commits and pushes: outside audit scope; .git is read-only.

## Deviations / proposed DESIGN changes

No requirements or gates were changed. The audit used an additional green run on
the exact snapshot, release-workflow evidence, extra platform compile checks and
temporary failure probes to distinguish unverified gaps from observed failures.

Proposed clarifications, for review rather than automatic relaxation:

- Resolve queued-post progress versus §10.3; preserve no-spin and fairness.
- Define p3 host wait/cancellation bounds and normalize private timeout accounting.
- Document explicit BufLease release, current Backend revision including resolve,
  pool/file-worker topology and transfer/TTY/WASI/web capability boundaries.
- State the scope of allocation guarantees for core, codecs, owned results and JS
  host; a nonzero allocation baseline is not zero-allocation behavior.
- Reconcile §9's “no other Perry change” P0 wording with §12 exact deadlines/O(1)
  liveness, retaining the stronger requirements; remove the async-io tray option
  inconsistent with binding zero-runtime policy.

## Open questions

Which supported p3 runtime contract can prove bounded yield/cancellation? How will
queued completions and native fairness both satisfy §10.3? Which file/transfer/
worker capabilities and npm option sets are required by the actual Perry branch?
What minimum Windows versions/mobile CI hosts are supported? Which release App ID
context needs configuration? These are scoped planning questions, not reasons to
leave this documentation audit incomplete.

## Next steps

Integrator reviews/commits the docs, assigns I01–I08 first-class contract/release
work and I09–I17 capability/verification/compatibility work, then maps Perry P0–P8
to the actual Perry checkout. Retain historical lane evidence; use this snapshot's
CI inventory instead of propagating stale global UNRUN claims.
