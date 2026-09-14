# ci-fix lane report

Updated 2026-09-14. Work in progress; the integrator owns commits and hosted CI.
Scope: dependency security exception, portable Windows suites, deterministic MySQL
 authentication, and Linux instruction-baseline bootstrap. WASI/web jobs untouched.

Implemented so far:
- Exact crate/version security exceptions with advisory, reason, checked expiry,
  checksum verification, active output and rejection of unused/expired entries.
- rustls 0.23.45 exception until seven-day eligibility, 2026-09-21T15:11:17Z.
- Native runner independently requires positive core/protocol counts; Windows
  production contracts remain explicitly pending with WINDOWS_HANDOFF.md summary.
- MySQL test deliberately warms both auth accounts, acknowledges FLUSH PRIVILEGES
  through a TLS admin fixture, then asserts full RSA/TLS and subsequent fast auth.
  CI image is pinned to mysql:9.6.0, matching local 9.6.0.

Verification ledger (updated as commands complete):
- PASS: required design, contribution, integration and relevant lane reports read;
  no applicable AGENTS.md or unnecessary cfg(unix) exclusions found.
- PASS: `env CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE='0 days' cargo +nightly-2026-08-20 update -p rustls --precise 0.23.45`.
  The pinned Cargo accepts this env form; override applies only to this command.
- PASS: registry index timestamp/checksum and RustSec advisory inspected; official
  upload-artifact v7.0.1 commit resolved via GitHub API.
- UNRUN (pending work): Python gate tests, fmt, native/cross Clippy, stable check,
  workspace tests, soak, no-tokio, cargo-deny and workflow lint.
- UNRUN: Windows/Linux execution and Linux Callgrind counts (no runtime here).
- UNRUN (sandbox): MySQL real-server body; initializer probe still to run.

Deviations / proposed DESIGN.md changes: no runtime, allocation, no-spin or soak
window relaxation. Security-only early lock updates are the user-authorized
exception to the dependency window. No DESIGN.md edit proposed yet.

Open questions / next steps: finish baseline bootstrap and adversarial tests;
run local checks, document exact artifact commit procedure, hand the coherent
working tree to the integrator for hosted CI and MySQL execution outside sandbox.
