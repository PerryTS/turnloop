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
- **FAIL** `cargo check -p turnloop` (exit 101; log 1789403784-513664).
- **PASS** `cargo check -p turnloop` (exit 0; log 1789403838-759359).
- **PASS** `cargo check -p turnloop --features executor` (exit 0; log 1789404051-297516).
- **PASS** `cargo clippy -p turnloop -p turnloop-contract --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` (exit 0; log 1789404080-249518).
- **FAIL** `cargo test -p turnloop-contract --features executor --test executor -- --test-threads=1` (exit 101; log 1789404153-917636).
- **PASS** `cargo clippy -p turnloop -p turnloop-contract --all-targets --all-features --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` (exit 0; log 1789404161-645415).
- **FAIL** `cargo test -p turnloop-contract --all-features --test native_surface --test executor -- --test-threads=1` (exit 101; log 1789404275-238389).
- **FAIL** `cargo test -p turnloop-contract --all-features --test native_surface --test executor -- --test-threads=1` (exit 101; log 1789404297-032008).
- **FAIL** `cargo test -p turnloop-contract --all-features --test native_surface --test executor -- --test-threads=1` (exit 101; log 1789404381-516986).
- **FAIL** `cargo test -p turnloop-contract --all-features --test native_surface file_backed_stdio_runs_in_the_child -- --test-threads=1 --nocapture` (exit 101; log 1789404434-847488).
- **PASS** `cargo test -p turnloop-contract --all-features --test native_surface --test executor -- --test-threads=1` (exit 0; log 1789404456-296511).
- **FAIL** `cargo test -p turnloop -p turnloop-contract --all-features -- --test-threads=1` (exit 101; log 1789404534-233569).
- **PASS** `cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::undocumented_unsafe_blocks` (exit 0; log 1789404543-528808).
