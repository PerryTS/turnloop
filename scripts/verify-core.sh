#!/bin/sh
set -eu
python3 scripts/ci/check-paths.py
# Explicit toolchain avoids rustup trying to repair unrelated Wasm components.
export RUSTUP_TOOLCHAIN=nightly-2026-08-20
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
RUSTFLAGS='--cfg loom' cargo test -p turnloop models -- --test-threads=1
cargo clippy --workspace --all-targets --target x86_64-unknown-linux-gnu --all-features -- -D warnings
cargo +stable check --workspace --locked
