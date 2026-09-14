#!/usr/bin/env python3
"""Build the exact guest-matching wasm-bindgen CLI under our seven-day soak.

Cargo install's separate resolution would bypass the project resolver policy.
Copy the checksum-verified registry source into an isolated ordinary Cargo project
under .tools, resolve there with the pinned Cargo, and build its locked graph.
"""
import os
from pathlib import Path
import shutil
from common import PIN, ROOT, cargo, entrypoint, run


def main():
    version = '0.2.108'
    run(cargo(PIN) + ['info', 'wasm-bindgen-cli@' + version], cwd=ROOT)
    cargo_home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    sources = list((cargo_home / 'registry/src').glob('*/wasm-bindgen-cli-' + version))
    if len(sources) != 1:
        raise RuntimeError(f'Expected one checksum-verified registry source, got {sources}')
    local = ROOT / '.tools/wasm-bindgen-source'
    if not local.exists():
        shutil.copytree(sources[0], local)
        (local / 'Cargo.lock').unlink(missing_ok=True)
        with (local / 'Cargo.toml').open('a') as manifest:
            manifest.write('\n[workspace]\n')
        run(cargo(PIN) + ['generate-lockfile', '--manifest-path', str(local / 'Cargo.toml')], cwd=ROOT)
    run(cargo(PIN) + ['build', '--release', '--locked', '--manifest-path', str(local / 'Cargo.toml')], cwd=ROOT)
    binary_dir = local / 'target/release'
    if 'GITHUB_PATH' in os.environ:
        with open(os.environ['GITHUB_PATH'], 'a') as path_file:
            path_file.write(str(binary_dir) + '\n')
    print('Add to PATH: ' + str(binary_dir))


if __name__ == '__main__':
    entrypoint(main)
