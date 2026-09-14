#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo +stable test --all-targets --locked
cargo check --target wasm32-wasip2 --locked
cargo check --target wasm32-unknown-unknown --locked
python3 - <<'PY'
import json,subprocess
m=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1']))
forbidden={'tokio','async-std','smol','async-io','tokio-util','tokio-rustls'}
found=forbidden.intersection(p['name'] for p in m['packages'])
assert not found, f'Forbidden runtime dependencies: {found}'
print(f'Runtime dependency gate passed ({len(m["packages"])} packages, including dev/target dependencies)')
PY
