# http-integrate lane report

Updated 2026-09-14. Integration in progress; no commits or uploads (integrator owns Git).

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
sites inspected. Workspace lock regeneration and initial strict Clippy are running.
All final gates, server tests, h2spec and packaged dry runs are currently UNRUN.

## Deviations / DESIGN proposals

No DESIGN rule changes or gate relaxations. Fork is option (b) from the brief.
The Rust standard library internal build feature is omitted from the standalone
fork; upstream decoder/encoder behavior and test expectations are retained.

## Open questions and next steps

Finish server lifecycle consolidation, h2spec full-suite enforcement, CI wiring,
portable compiler/test fixes, all requested verification and publish-order report.
Linux/Windows runtime and Docker unavailable; SQL sandbox limitations will be
recorded only after attempting the required invocations.
