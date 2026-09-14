#!/usr/bin/env python3
"""Install the pinned Linux CI browsers and matching drivers, verifying SHA-256."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import tarfile
import tempfile
import urllib.request
import zipfile
from common import ROOT, entrypoint, fail, run


def extract_verified(archive, pin, destination):
    with archive.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    if digest != pin['sha256']:
        fail(f'{archive.name}: SHA-256 mismatch: {digest}')
    destination.mkdir(parents=True, exist_ok=True)
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as bundle:
            for member in bundle.infolist():
                target = (destination / member.filename).resolve()
                if not target.is_relative_to(destination.resolve()):
                    fail('Unsafe browser archive path')
                if (member.external_attr >> 16) & 0o170000 == 0o120000:
                    fail('Unexpected browser archive symlink')
            bundle.extractall(destination)
            for member in bundle.infolist():
                target = destination / member.filename
                if target.is_file():
                    target.chmod(0o755 if (member.external_attr >> 16) & 0o111 else 0o644)
    else:
        with tarfile.open(archive) as bundle:
            bundle.extractall(destination, filter='data')
    binary = destination / pin['executable']
    if not binary.is_file():
        fail(f'Missing browser executable: {binary}')
    binary.chmod(0o755)
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verify-only', action='store_true', help='Verify/extract Linux assets without executing them')
    args = parser.parse_args()
    if not args.verify_only and (platform.system(), platform.machine()) != ('Linux', 'x86_64'):
        fail('Pinned browser execution requires Linux x86_64; use --verify-only on other hosts')
    pins = json.loads((ROOT / 'scripts/ci/browsers.json').read_text())['Linux-x86_64']
    if pins['chrome']['version'] != pins['chromedriver']['version']:
        fail('Chrome and chromedriver pins must match exactly')
    root = ROOT / '.tools/browsers'
    root.mkdir(parents=True, exist_ok=True)
    paths = {}
    for name, pin in pins.items():
        archive = root / pin['url'].rsplit('/', 1)[1]
        if not archive.exists():
            with urllib.request.urlopen(pin['url'], timeout=60) as response, archive.open('wb') as output:
                shutil.copyfileobj(response, output)
        with tempfile.TemporaryDirectory(dir=root) as temporary:
            stage = Path(temporary)
            extract_verified(archive, pin, stage)
            destination = root / name
            if destination.exists():
                shutil.rmtree(destination)
            stage.rename(destination)
        binary = (destination / pin['executable']).resolve()
        paths[name] = str(binary)
        print(f'PASS verified {name} {pin["version"]} sha256:{pin["sha256"]}', flush=True)
        if not args.verify_only:
            output = run([str(binary), '--version'], capture=True)
            print(output, end='')
            if pin['version'] not in output:
                fail(f'{name}: executable version does not match pin')
    if not args.verify_only:
        config = root / 'paths.json'
        config.write_text(json.dumps(paths, indent=2) + '\n')
        if 'GITHUB_ENV' in os.environ:
            with open(os.environ['GITHUB_ENV'], 'a') as output:
                output.write(f'TURNLOOP_BROWSER_PATHS={config}\n')


if __name__ == '__main__':
    entrypoint(main)
