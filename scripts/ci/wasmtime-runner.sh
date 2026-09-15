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
exec "$root/.tools/bin/wasmtime" run -S inherit-network=y -S allow-ip-name-lookup=y -W timeout=120s "${fixture_args[@]}" "$@"
