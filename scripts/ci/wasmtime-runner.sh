#!/usr/bin/env bash
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
exec "$script_dir/../../.tools/bin/wasmtime" run -S inherit-network=y -W timeout=120s "$@"
