# core3 verification commands

Commands run on macOS arm64 using nightly-2026-08-20 unless specified.
Cross-compilation is not runtime execution. Raw output is in `.tools/core3/`.

- PASS: `cargo build --release -p turnloop-bench --target-dir target/core3-before`.
- PASS: five fresh processes of `target/core3-before/release/turnloop-bench`;
  every row asserted positive operations and `unit == instructions`.
  Raw before measurements: `.tools/core3/before.jsonl`.
