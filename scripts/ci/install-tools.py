#!/usr/bin/env python3
"""Install official release binaries using committed SHA-256 pins, without sudo."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import tarfile
import tempfile
import urllib.request
import zipfile
from common import ROOT, entrypoint, fail


def install(name, destination):
    pins = json.loads((ROOT / 'scripts/ci/tools.json').read_text())
    host = f'{platform.system()}-{platform.machine()}'
    if host == 'Darwin-arm64':
        host = 'Darwin-aarch64'
    pin = pins[name][host]
    expected = pin['sha256']
    with urllib.request.urlopen(pin['url'], timeout=120) as response:
        archive = response.read()
    digest = hashlib.sha256(archive).hexdigest()
    if digest != expected:
        fail(f'{name}: SHA-256 mismatch: expected {expected}, got {digest}')
    # Extract only regular executable files by basename, never archive paths/links.
    with tempfile.TemporaryFile() as handle:
        handle.write(archive)
        handle.seek(0)
        if pin['url'].endswith('.zip'):
            with zipfile.ZipFile(handle) as bundle:
                for executable in pin['executables']:
                    matches = [m for m in bundle.infolist()
                               if not m.is_dir() and (m.external_attr >> 16) & 0o170000 in (0, 0o100000)
                               and Path(m.filename).name == executable]
                    if len(matches) != 1:
                        fail(f'{name}: expected exactly one {executable} in archive')
                    target = destination / executable
                    target.write_bytes(bundle.read(matches[0]))
        else:
            install_tar(handle, pin, name, destination)
    print(f'PASS install {name} {pin["version"]}: sha256:{digest}')


def install_tar(handle, pin, name, destination):
    with tarfile.open(fileobj=handle, mode='r:*') as bundle:
        for executable in pin['executables']:
            matches = [m for m in bundle.getmembers()
                       if m.isfile() and Path(m.name).name == executable]
            if len(matches) != 1:
                fail(f'{name}: expected exactly one {executable} in archive')
            contents = bundle.extractfile(matches[0])
            if contents is None:
                fail(f'{name}: cannot read {executable}')
            target = destination / executable
            target.write_bytes(contents.read())
            target.chmod(0o755)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tools', nargs='+')
    parser.add_argument('--destination', type=Path, default=ROOT / '.tools/bin')
    args = parser.parse_args()
    args.destination.mkdir(parents=True, exist_ok=True)
    for name in args.tools:
        install(name, args.destination)
    if os.environ.get('GITHUB_PATH'):
        with open(os.environ['GITHUB_PATH'], 'a', encoding='utf-8') as handle:
            handle.write(str(args.destination.resolve()) + '\n')


if __name__ == '__main__':
    entrypoint(main)
