#!/usr/bin/env python3
"""Verify a Perry per-version soak override against crates.io and the source commit."""
import argparse
import importlib.util
import json
from pathlib import Path
import tomllib
from common import entrypoint, fail


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--source-commit', required=True)
    parser.add_argument('--lockfile', type=Path, required=True)
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location('release', Path(__file__).with_name('release.py'))
    release = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(release)
    data = release.registry(args.package)
    if data is None:
        fail('Crate does not exist')
    version = next((v for v in data['versions'] if v['num'] == args.version), None)
    if version is None:
        fail('Crate version does not exist')
    packages = tomllib.loads(args.lockfile.read_text())['package']
    locked = [p for p in packages if p['name'] == args.package and p['version'] == args.version]
    if len(locked) != 1 or locked[0].get('checksum') != version['checksum'] or locked[0].get('source') != 'registry+https://github.com/rust-lang/crates.io-index':
        fail('Cargo.lock does not match the crates.io version/checksum/source')
    release.verify_existing({'name': args.package, 'version': args.version}, version, args.source_commit)
    print(json.dumps({'crate': args.package, 'version': args.version, 'published_at': version['created_at'],
        'source_commit': args.source_commit, 'sha256': version['checksum']}, indent=2))


if __name__ == '__main__':
    entrypoint(main)
