# core2 verification commands

Cross-compilation is not runtime execution. Raw outputs: `.tools/core2/`.

- **PASS** `cargo check -p turnloop` (exit 0; log 1789402872-060718).
- **FAIL** `cargo test -p turnloop-contract --test native_surface -- --test-threads=1` (exit 101; log 1789402922-508323).
