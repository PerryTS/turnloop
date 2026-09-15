#!/usr/bin/env bash
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
fixture_args=()
# Pass only the fixture contract, never the host's complete environment.
while IFS= read -r name; do
  case "$name" in TURNLOOP_TEST_*) fixture_args+=(--env "$name");; esac
done < <(compgen -e)
# Fixture certificates/dumps use absolute paths supplied by test-servers.py.
root=$(cd -- "$script_dir/../.." && pwd)
if [[ -d "$root/.tools" ]]; then fixture_args+=(--dir "$root/.tools"); fi
# Filesystem contracts get a fresh, empty preopen; it is the only path authority.
fs_root=$(mktemp -d "${TMPDIR:-/tmp}/turnloop-wasi-fs.XXXXXX")
trap 'rm -rf -- "$fs_root"' EXIT
fixture_args+=(--dir "$fs_root::/turnloop-fs")
"$root/.tools/bin/wasmtime" run -S inherit-network=y -S allow-ip-name-lookup=y -W timeout=120s "${fixture_args[@]}" "$@"
