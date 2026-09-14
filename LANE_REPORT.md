# ci-fix2 lane report

Updated 2026-09-14. Implementation and verification in progress. Integrator owns
commits, pushes and the GitHub rerun; no Git mutations attempted.

## Findings and implementation

- Read DESIGN.md, CONTRIBUTING.md, docs/INTEGRATION_REPORT.md and CI, KV, MongoDB,
  SQL lane reports completely. The workflow/log contradict the proposed apt Redis
  explanation: --ci-services shadows redis-server/redis-cli with Docker wrappers
  using floating redis:8. The first five-second wait expired with the wrapper still
  alive. A cold image pull is plausible; the discarded diagnostics prevent proof.
  SQL container bootstrap succeeded; no protocol tests ran in job 34869729240.
- CI installs native Redis 8.4.0 (matches local Homebrew), official source archive
  checked against Redis's published SHA-256, BUILD_TLS=yes, cached by installer
  hash/platform. Redis Docker wrappers removed; Mongo image pulled before readiness.
- Private server stdout/stderr captured under .tools, bounded log tails on startup
  failure. Redis owns/reaps children and handles detached crashes with protocol
  identity checks; failed cleanup chains under the original startup error.
- Tests and verification are in progress. Original CI log will be deleted after
  evidence is summarized here.

## Verification ledger

| Command | Result |
|---|---|
| `redis-server --version` | PASS: 8.4.0, libc, macOS arm64 |
| `python3 -m py_compile scripts/test-servers.py` | PASS, initial edit |
| Official Redis hashes and actions/cache tag/release API reads | PASS: Redis 8.4.0 SHA-256 ca909aa15252f2ecb3a048cd086469827d636bf8334f50bb94d03fba4bfc56e8; cache v6.1.0 SHA 55cc8345863c7cc4c66a329aec7e433d2d1c52a9, released June 26, already soaked |
| Required local Rust/script/fixture/lint checks | UNRUN, pending |
| Linux/Windows/Docker runtime, GitHub cache execution | UNRUN: no host/Docker locally |

## Deviations / proposed DESIGN.md changes

No DESIGN.md change. No runtime, no-spin, allocation, dependency-soak or test-gate
policy changes. This lane changes fixture automation only; no operation path added.

## Open questions / next steps

Finish regression tests, run local Redis/MongoDB/SMTP tests, record SQL sandbox
limits, run required quality checks and workflow linters, review final diff and
remove supplied CI log. Integrator then commits/pushes and reruns protocol CI.
