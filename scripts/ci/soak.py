"""Enforce the resolver policy AND registry age of every locked registry package."""
import argparse
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import os
import tomllib
import urllib.request
from common import PIN, cargo, entrypoint, fail, members, metadata, run


def check_age(package, record, now):
    if record['cksum'] != package['checksum']:
        fail(f'Checksum mismatch for {package["name"]}@{package["version"]}')
    published = datetime.fromisoformat(record['pubtime'].replace('Z', '+00:00'))
    eligible = published + timedelta(days=7)
    if now < eligible:
        fail(f'Supply-chain soak: {package["name"]}@{package["version"]} was published '
             f'{published.isoformat()}; eligible {eligible.isoformat()} (7 days). '
             'Choose an older version; a pre-filled Cargo.lock does not waive the soak.')


def index_path(name):
    name = name.lower()
    if len(name) < 3:
        return f'{len(name)}/{name}'
    if len(name) == 3:
        return f'3/{name[0]}/{name}'
    return f'{name[:2]}/{name[2:4]}/{name}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', default='Cargo.toml')
    args = parser.parse_args()
    data = metadata(args.manifest_path)
    members(data)
    root = Path(data['workspace_root'])
    config = tomllib.loads((root / '.cargo/config.toml').read_text())
    if config.get('unstable', {}).get('min-publish-age') is not True:
        fail('Pinned-nightly min-publish-age switch must remain enabled')
    if config.get('registry', {}).get('global-min-publish-age') != '7 days':
        fail('registry.global-min-publish-age must remain "7 days"')
    if config.get('resolver', {}).get('incompatible-publish-age', 'fallback') == 'allow':
        fail('incompatible-publish-age=allow disables the soak')
    if any('PUBLISH_AGE' in key for key in os.environ):
        fail('Environment publish-age overrides are forbidden in CI')
    # Resolve under the actual repository config and pin; never regenerate the lock.
    run(cargo(PIN) + ['metadata', '--locked', '--all-features', '--format-version', '1',
        '--manifest-path', str(Path(args.manifest_path).resolve())], cwd=root, capture=True)
    lock = tomllib.loads((root / 'Cargo.lock').read_text())
    count = 0
    cache = {}
    now = datetime.now(timezone.utc)
    for package in lock['package']:
        source = package.get('source', '')
        if not source:
            continue
        if source != 'registry+https://github.com/rust-lang/crates.io-index':
            fail(f'Unapproved dependency source for {package["name"]}: {source}')
        name = package['name']
        if name not in cache:
            url = 'https://index.crates.io/' + index_path(name)
            request = urllib.request.Request(url, headers={'User-Agent': 'PerryTS-turnloop-ci'})
            with urllib.request.urlopen(request, timeout=60) as response:
                cache[name] = {r['vers']: r for r in map(json.loads, response.read().decode().splitlines())}
        check_age(package, cache[name][package['version']], now)
        count += 1
    print(f'PASS soak: {count} locked registry versions; pinned resolver policy active')


if __name__ == '__main__':
    entrypoint(main)
