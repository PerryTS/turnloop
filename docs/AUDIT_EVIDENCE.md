# Audit evidence and reproducibility

Snapshot: `a0fbb1bf9a100bb3215219f54301b9ee82ffe71f`, 2026-09-15.
This is a documentation-only audit. No code, test, CI, gate, lockfile, policy or
Git metadata changes were made. Additional executable probes lived under `/tmp`.

## CI collection

Read DESIGN.md, CONTRIBUTING.md, the entire previous integration report, all
`docs/lanes/` reports, current trait/WASM/release documentation, workflow/test
runner metadata, and implementation/test paths cited by DESIGN_AUDIT.md.

For each of 34919345014, 34918732035 and 34916140575:

```sh
gh run view <run> --json headSha,headBranch,status,conclusion,event,url,jobs
gh api repos/PerryTS/turnloop/actions/jobs/<job>/logs
```

[ci-jobs.tsv](audit/ci-jobs.tsv) contains all 114 job records, including the three
skipped optional self-hosted Windows jobs. All **111 successful job logs** were
downloaded and read for the applicable subject evidence. SHA-256 identifies the
exact downloaded bytes; hosted logs remain accessible through the job links and
API while GitHub retains them. Local raw copies are under `/tmp/gap-audit-ci/`.
The TSV's `runner_counts` lists separate positive runner summaries in log order;
it is not a sum of unique tests. Workflow-lint mock outputs are excluded from
those counts. Its actual unittest runner reports **97 tests**.

`git diff --stat 163fd0c HEAD` was empty: PR #13 head and audited main are the same
tree. A separate exact-main run was nevertheless found and is the primary citation.
The earlier green main run corroborates earlier functionality; it is not evidence
for PR #13 additions unless also exercised in the later runs.

### What the evidence does and does not establish

- Native independent member and contract invocations reject zero counts. Workspace
  cfg-excluded zero-test binaries and ordinary ignored tests are not positive
  execution evidence for those bodies. Nonzero suites can still contain scoped
  exclusions; source cfgs and metadata were also examined.
- W2/W3 both execute debug and release contracts, then release allocation subjects.
  p3 also executes its entropy shim tests. Wasmtime 46 is CI's pinned runtime;
  the audit's extra p2 allocation run used installed Wasmtime 44.0.0.
- PW2/PW3 each execute 19 selected suites, with counts
  `2,1,9,16,3,2,1,3,3,1,3,3,5,11,2,2,3,1,3`.
  P additionally runs real p2 async-server tests. No p3 real-server run is inferred.
- WEB executes 13 subjects per engine and independently counts fixture traffic:
  `slow=2, fetches=5, aborted=2, websockets=6, echoed=7937` per engine. This proves
  real requests/aborts/messages, not complete JS Fetch/WS parity or zero JS allocation.
- `h2spec` reports 147/147/0 skips. MODEL means six production Loom models and
  timer/table Miri only. PERF means four Callgrind cases including control; no
  perf kernel, ETW, full-operation or Perry A/B evidence is inferred.
- Seven-day dependency soak passes for 251 registry versions. The existing
  rustls 0.23.45 exception is for RUSTSEC-2026-0285, configured `expires=2026-09-21`;
  the gate reports eligibility at `2026-09-21T15:11:17+00:00`. No exception was
  introduced or extended by this audit.

### Release evidence is separate from required CI

Read `gh run list --workflow release.yml` and release metadata/logs. In
[34919821444](https://github.com/PerryTS/turnloop/actions/runs/34919821444):

- [verify 104225201632](https://github.com/PerryTS/turnloop/actions/runs/34919821444/job/104225201632)
  logs `PASS ci-gate a0fbb1bf9a100bb3215219f54301b9ee82ffe71f run 34919345014 attempt 1`.
- [release-pr 104225245365](https://github.com/PerryTS/turnloop/actions/runs/34919821444/job/104225245365)
  fails during GitHub App token creation: the client/app ID input is empty.
  `.github/workflows/release.yml:67` supplies `vars.RELEASE_APP_ID`. This does not
  establish that its private key or any other secret is missing.
- `publish` 104225247044 is skipped. The earlier
  [34916736320 release-pr job](https://github.com/PerryTS/turnloop/actions/runs/34916736320/job/104215916851)
  shows the same empty-ID error.

The [alpha.2 release](https://github.com/PerryTS/turnloop/releases/tag/v0.1.0-alpha.2)
was published at 2026-09-15T01:10:54Z and states all 12 crates were published.
This audit did not download and checksum every registry archive or establish the
publication credentials used. Thus existing release, exact-CI verification and
successful automated OIDC publication have distinct evidence statuses.

## Local failures and probes

Full commands/statuses and hashes are in
[local-verification.tsv](audit/local-verification.tsv) and [LANE_REPORT.md](../LANE_REPORT.md).
Native fmt, default/all-feature strict Clippy, stable check, default-parallel
workspace tests (**251 passed, 13 ignored**), zero-tokio, soak and feature-mode
checks pass. Cross-clippy covers core/contracts only, not all protocol C dependencies
or linking. Runtime acceptance on Linux/Windows/mobile/FreeBSD is not claimed locally.

### Android all-target Clippy: FAIL

```sh
cargo clippy --locked -p turnloop -p turnloop-contract --all-targets --all-features --target aarch64-linux-android -- -D warnings -D clippy::undocumented_unsafe_blocks
```

Pinned nightly reports `clippy::missing_const_for_thread_local` twice at
`crates/turnloop/src/portable_tests.rs:13`, despite the `const { Cell::new(...) }`
initializers. This is a target test-lint failure, not proof the epoll library
cannot compile. No lint suppression or source change was made. Runtime UNRUN.

### Linux musl all-target Clippy: FAIL

Same command with `--target x86_64-unknown-linux-musl` rejects deprecated
`libc::time_t` at `crates/turnloop/src/backend/poller.rs:25` (twice). This affects
the production timespec conversion under strict warnings. Runtime UNRUN.

### WASI 0.2 compressed MongoDB: FAIL, subject ran

```sh
cargo test --locked -p turnloop-mongodb --release --target wasm32-wasip2 --test allocations --config 'target.wasm32-wasip2.runner="wasmtime run"' -- --nocapture
```

The first invocation without `--nocapture` trapped after announcing the warmed
workload. It was repeated once with output capture disabled to obtain the failed
assertion; both invocations fail. Exact second-run output:

```text
test allocation::allocator_counts_alloc_zeroed_and_realloc_on_the_measured_thread ... ok
assertion `left == right` failed: 1000 warmed commands/2000 rows allocated 1000 times (zlib=true, operation=false)
  left: 1000
 right: 0
```

The failing assertion is `protocols/turnloop-mongodb/tests/allocations.rs:139`.
The preceding test path asserts 2,004 rows including warm-up; calibration passes.
The compressed coordinator mode is UNRUN after the abort, not a pass. p3 was not
rerun in this audit; `docs/lanes/proto-fix1.md:144` records the same older p3 failure.
Current PW3 selects `asynchronous`, not this allocation test, so its green status
does not clear that failure. I03 requires both targets and all modes.

### Queued post with idle native I/O: FAIL, subject ran

A temporary Cargo binary depended only on the audited `crates/turnloop` path.
Run with `cargo run --offline --manifest-path /tmp/gap-audit-probe/Cargo.toml`.
The final source (recreate in any external scratch crate) was:

```rust
use turnloop::*;
fn main() -> Result<()> {
    let mut driver = Loop::new(Config::default())?;
    let socket = driver.udp_bind(([127, 0, 0, 1], 0).into(), &UdpOpts::default())?;
    driver.recv(socket, ReadBuf::Pooled, Token(1))?;
    let mut out = Completions::default();
    driver.turn(Timeout::Now, &mut out)?;
    assert_eq!(out.len(), 0);
    driver.poster().post(Token(2), Payload::U64(42)).expect("post accepted");
    let info = driver.turn(Timeout::Now, &mut out)?;
    assert_eq!(out.len(), 1);
    assert!(matches!(out.drain().next().expect("post delivered").result,
        OpResult::Posted(Payload::U64(42))));
    println!("queued post with idle pending UDP: os_waits={}, completions={}",
        info.os_waits, info.completions);
    assert_eq!(info.os_waits, 0, "DESIGN section 10 rule 3");
    Ok(())
}
```

Observed: `os_waits=1, completions=1`; final assertion fails (exit 101).
The initial scratch build used the nonexistent spelling `recv_from`; that build
failed before execution. Correcting the probe to the public `recv` API produced
the result above; no repository file changed. Source path: `driver.rs:1013–1024`
allows a zero-time backend poll with queued posts and pending I/O;
`backend/unix.rs:586` performs the OS wait. This is a §10.3 violation, not evidence
that the existing idle-timer no-spin test failed. I01 must preserve native progress.

## Inventory reproduction and limitations

The [marker TSV](audit/markers.tsv) is generated from the original snapshot, not
from the rewritten report. Equivalent collection patterns:

```sh
rg -n 'TODO|FIXME|unimplemented!|todo!' --glob '!Cargo.lock'
rg -ni 'Unsupported|StreamingUnsupported|UnsupportedOffset' crates protocols spikes
rg -n '#\[ignore' crates protocols spikes
rg -n '#!?\[cfg|cfg!\(|^\[target.*cfg' crates protocols spikes
rg -ni 'TODO|FIXME|UNRUN|pending|not[[:space:]]+yet|not implemented' --glob '*.md'
```

The actual inventory reads every tracked UTF-8 file in `git archive <SHA>` and
collects multiline cfg predicates; it includes four additional semantic doc
claims. Local temporary classification tooling is `/tmp/gap-audit-inventory.py`.
Each row retains source text/context and a per-marker verdict so reviewers can
reassess it without that temporary script. Unsupported mappings/examples and
pending-operation prose are retained as non-defects instead of silently dropped.
No root or `.github/SECURITY.md` occurs in the tracked-file inventory.

Future work items are interpretations of DESIGN and documented API limits,
explicitly distinguished from reproduced failures. Full security review, external
Perry source audit, every feature combination, all registry archive provenance,
long-duration stability and new platform runtime tests were not performed here.
