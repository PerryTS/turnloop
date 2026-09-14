# Integration lane report

Updated 2026-09-14. Wave-1 integration and local verification are complete; external
release prerequisites remain explicitly pending. The integrator makes commits because Git metadata is read-only.

Implemented: full turnloop rename; eight-member workspace (six publishable crates,
two private helpers); stable 1.97.1 metadata; shared soaked dependencies; private
start/stop/run server runner and common TURNLOOP_TEST_* ports; consolidated CI with
explicit pending platform/baseline gates; native and cross-target test fixes;
no-spin contract and the cached-readiness/EAGAIN core fix.

PASS: default workspace tests, strict native/Linux/WASI/web Clippy, stable
1.97.1, rustdoc, five loom models, two Miri suites, no-tokio on all eight target
graphs, seven-day soak, cargo-deny, workflow lint wrapper and 15 automation tests. All six
publishable crates passed fully verified cargo publish dry runs; no --no-verify
was needed and nothing was uploaded. Redis (single/ACL/TLS/cluster/Sentinel), MongoDB
(standalone/replicas/auth/TLS), and SMTP real-server subset passed.

UNRUN: SQL test bodies (PostgreSQL shmget denied; MySQL initializer crashes in this
sandbox), Linux/Windows runtime suites, full Windows test-target Clippy (ring needs
Windows SDK headers), Docker CI fixture path, Windows/WASI/web production contracts,
and Linux instruction measurements. Their gates have not been weakened or skipped.

Audit changes: removed unmaintained rustls-pemfile; updated the iai-callgrind gate
to its maintained Gungraun 0.19.4 successor to remove proc-macro-error2; kept all
thresholds and v6 summary checks. Pinned email-encoding 0.4.1 to share base64 0.22.1.
Added permissive 0BSD for quoted_printable, with no advisory/runtime exceptions.

The detailed command history, earlier failures and remaining lane questions are in
[docs/INTEGRATION_REPORT.md](docs/INTEGRATION_REPORT.md) and its linked command log.
Next: integrator reruns SQL and hosted CI, integrates
wave-2 backends/HTTP, obtains the Linux baseline and handles first publications.

Earlier failures, including a loaded timer-precision run, remain in the report.
The unchanged full native gate passed afterward. Raw actionlint still rejects
GitHub concurrency.queue; the strict validated compatibility wrapper passes.
