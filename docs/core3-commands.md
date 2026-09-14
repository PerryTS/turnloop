# core3 verification commands

Commands run on macOS arm64 using nightly-2026-08-20 unless specified.
Cross-compilation is not runtime execution. Raw output is in `.tools/core3/`.

- PASS: `cargo build --release -p turnloop-bench --target-dir target/core3-before`.
- PASS: five fresh processes of `target/core3-before/release/turnloop-bench`;
  every row asserted positive operations and `unit == instructions`.
  Raw before measurements: `.tools/core3/before.jsonl`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 1/50, exit 0, 52 test passes, log `.tools/core3/contract50-01.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 2/50, exit 0, 52 test passes, log `.tools/core3/contract50-02.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 3/50, exit 0, 52 test passes, log `.tools/core3/contract50-03.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 4/50, exit 0, 52 test passes, log `.tools/core3/contract50-04.log`.

- **FAIL** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50 5/50, exit 101, 36 test passes, log `.tools/core3/contract50-05.log`.

- **PASS** `cargo test -p turnloop-contract --all-features --test native_surface kills_live_child_and_grandchild_as_a_group -- --test-threads=1`; group-reproduce 1/1, exit 0, 1 test passes, log `.tools/core3/group-reproduce-01.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 1/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-01.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 2/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-02.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 3/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-03.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 4/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-04.log`.

- **PASS** `cargo test -p turnloop-contract --all-features -- --test-threads=1`; contract50-final 5/50, exit 0, 52 test passes, log `.tools/core3/contract50-final-05.log`.
