# core2 verification commands

Cross-compilation is not runtime execution. Raw outputs: `.tools/core2/`.

- **PASS** `cargo check -p turnloop` (exit 0; log 1789402872-060718).
- **FAIL** `cargo test -p turnloop-contract --test native_surface -- --test-threads=1` (exit 101; log 1789402922-508323).
- **FAIL** `cargo check -p turnloop` (exit 101; log 1789403313-560113).
- **PASS** `cargo test -p turnloop-contract --test native_surface -- --test-threads=1` (exit 0; log 1789403340-731985).
- **FAIL** `cargo test -p turnloop-contract --test native_surface -- --test-threads=1` (exit 101; log 1789403454-694022).
- **PASS** `cargo check -p turnloop` (exit 0; log 1789403548-662864).
- **PASS** `cargo test -p turnloop-contract --test native_surface -- --test-threads=1` (exit 0; log 1789403604-260402).
- **FAIL** `cargo clippy -p turnloop -p turnloop-contract --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` (exit 101; log 1789403650-349026).
