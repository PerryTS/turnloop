#!/usr/bin/env bash
# Wasmtime 46.0.0 official archives; SHA-256 pins are in tools.json.
# Python checks the digest before extracting. It installs only under .tools/.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
exec python3 "$script_dir/install-tools.py" wasmtime "$@"
