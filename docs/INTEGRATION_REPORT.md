# Integration report

In progress. No upload or Git write was attempted. Historical lane evidence lives under `docs/lanes/`.

The initial read-only inventory (`pwd`, `rg --files`, `git status --short`, `cat`/`sed`/`wc` of DESIGN, LANES, manifests, lane reports and CI/harness sources) passed. `.tools` did not exist initially. One guessed contract test filename did not exist; actual tests are being discovered. `rustc +stable --version` PASS: 1.97.1, used as verified compiler/MSRV, despite the environment description's 1.98.

Verification commands and exact exit statuses will be appended to [integration-commands.md](integration-commands.md). Logs live locally under `.tools/verification/`.

Pending: complete workspace verification, fixture consolidation, CI alignment, no-spin contract, dependency audit, publication dry runs. Linux/Windows runtime tests and Linux instruction measurements are UNRUN: no host. SQL server runtime checks are expected UNRUN due to sandbox limits, to be reproduced and logged.
