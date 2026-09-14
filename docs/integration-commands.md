# Integration verification commands

- PASS (exit 0): `cargo generate-lockfile` — log `.tools/verification/1789400226612845000.log`.
- PASS (exit 0): `python3 scripts/ci/install-tools.py actionlint zizmor shellcheck cargo-deny` — log `.tools/verification/1789400242421835000.log`.
- FAIL (exit 101): `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` — log `.tools/verification/1789400242416777000.log`.
- PASS (exit 0): `cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` — log `.tools/verification/1789400309362332000.log`.
- FAIL (exit 101): `cargo test -p turnloop-contract idle_socket_timers_do_not_spin -- --nocapture` — log `.tools/verification/1789400478865047000.log`.
- PASS (exit 0): `cargo +stable check --workspace` — log `.tools/verification/1789400480002595000.log`.
- PASS (exit 0): `cargo generate-lockfile` — log `.tools/verification/1789400492562365000.log`.
- FAIL (exit 1): `scripts/test-servers.py --services postgres run true` — log `.tools/verification/1789400493291513000.log`.
- PASS (exit 0): `cargo test -p turnloop-contract idle_socket_timers_do_not_spin -- --nocapture` — log `.tools/verification/1789400515139760000.log`.
- FAIL (exit 101): `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` — log `.tools/verification/1789400516270300000.log`.
- FAIL (exit 1): `scripts/test-servers.py --services mysql run true` — log `.tools/verification/1789400523887800000.log`.
- FAIL (exit 101): `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` — log `.tools/verification/1789400554514729000.log`.
- FAIL (exit 5): `env PATH=.tools/bin:/opt/homebrew/bin:/usr/bin:/bin:/Users/amlug/.cargo/bin cargo deny check` — log `.tools/verification/1789400568146303000.log`.
- PASS (exit 0): `cargo test --workspace` — log `.tools/verification/1789400565811533000.log`.
- PASS (exit 0): `scripts/test-servers.py --services redis,mongodb,smtp run cargo test -p turnloop-redis -p turnloop-mongodb -p turnloop-smtp -- --include-ignored --test-threads=1` — log `.tools/verification/1789400566980343000.log`.
