# Integration verification commands

- PASS (exit 0): `cargo generate-lockfile` — log `.tools/verification/1789400226612845000.log`.
- PASS (exit 0): `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck cargo-deny` — log `.tools/verification/1789400242421835000.log`.
- FAIL (exit 101): `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` — log `.tools/verification/1789400242416777000.log`.
- PASS (exit 0): `cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` — log `.tools/verification/1789400309362332000.log`.
