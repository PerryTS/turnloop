# http-integrate lane report

Updated 2026-09-14. Implementation complete; final verification in progress. No commits or uploads (integrator owns Git).

## Implemented

- Unified HTTP/TLS/WebSocket shared dependencies and publication metadata; retained
  ring/std/tls12 for all rustls users. Removed reintroduced rustls-pemfile in favor
  of maintained rustls-pki-types PEM parsing. The seven-day soak is unchanged.
- Promoting the existing allocation fork into `turnloop-zstd-decoder`, with explicit
  versioned HTTP dependency, upstream MIT license, attribution and provenance.
  No root patch. Upstream 0.8.3 and soaked 0.9.0 both still allocate six times per
  default-table frame internally; no public API/configuration bypasses those sites.
- Restoring upstream corpus fixtures so workspace membership runs its tests.

## Verification ledger

PASS: complete required design/contribution/integration and relevant lane report reads;
upstream 0.9.0 archive SHA-256 verified against registry metadata and allocation
sites inspected. PASS: soaked lock regeneration (240 registry versions); strict native/Linux/WASI/web
Clippy; stable 1.97.1; no-tokio on eight targets plus union, default/all features;
18 automation tests; workflow lint; 15 HTTP/TLS/WebSocket interop tests; strict
h2spec 147/147 with zero skips; 20 executed WASI tests including decoder reuse;
all ten package dry runs (packaged tarball verification, no --no-verify).

FAIL release gate: RUSTSEC-2026-0285 affects shared rustls 0.23.44. Fixed 0.23.45
was published 2026-09-14 15:11:17.808465 UTC and is ineligible until
**2026-09-21 15:11:17.808465 UTC**. No advisory exception or soak override added.

UNRUN: SQL real-server bodies (full runner fails at PostgreSQL shmget; MySQL
initializer separately exits 2 in sandbox), Windows runtime/test-target Clippy
(missing SDK headers for ring/zstd), Linux/Docker runtime. Native non-SQL full
suite rerun pending after correcting a new corpus-count assertion: the upstream
snapshot contains 101 ordinary frames and 207 dictionary frames, not 301 ordinary
frames. Existing byte/checksum/corpus assertions remain unchanged.

Expected FAIL: unmodified upstream ruzstd allocation repro sees exactly 6000
allocations for 1000 frames; identical test passes at zero with the published fork.
Earlier corrected failures: workspace unsafe/lint errors in newly active upstream
source; wasm dictionary isize overflow (changed to i64); h2spec report identity
needed package+classname; ci-gate job-name parser needed digits; reserved upstream
Cargo.toml.orig filename renamed; Mozilla data license reviewed and included.

## Deviations / DESIGN proposals

No DESIGN rule changes or gate relaxations. Fork is option (b) from the brief.
The Rust standard library internal build feature is omitted from the standalone
fork; upstream decoder/encoder behavior and test expectations are retained.

## Open questions and next steps

Finish server lifecycle consolidation, h2spec full-suite enforcement, CI wiring,
portable compiler/test fixes, all requested verification and publish-order report.
Linux/Windows runtime and Docker unavailable; SQL sandbox limitations will be
recorded only after attempting the required invocations.
