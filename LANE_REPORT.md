# Integration lane report

In progress. The integrator owns commits; this sandbox cannot write Git metadata.

Implemented: complete crate/source rename, shared workspace and metadata. Stable installed here is 1.97.1, so the declared MSRV is 1.97.1 (verification pending).

Verification, dependency decisions, publish order and remaining work are tracked in [docs/INTEGRATION_REPORT.md](docs/INTEGRATION_REPORT.md).

Next: unified fixtures and CI, no-spin contract, native/cross checks, audits and publish dry runs. Windows/WASM production backends await wave 2; HTTP/TLS/WebSocket crates await their lane.
