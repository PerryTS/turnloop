"""Enforce the resolver policy AND registry age of every locked registry package."""
import argparse
from datetime import date, datetime, timedelta, timezone
import json
from pathlib import Path
import os
import re
import tomllib
import urllib.request
from common import PIN, cargo, entrypoint, fail, members, metadata, run


def security_exceptions(policy, now):
    exceptions = {}
    for entry in policy.get('security-exceptions', []):
        if set(entry) != {'crate', 'version', 'advisory', 'reason', 'expires'}:
            fail('Security exception requires crate, exact version, advisory, reason and expires')
        for key in ('crate', 'version', 'advisory', 'reason'):
            if not isinstance(entry[key], str) or not entry[key].strip():
                fail(f'Security exception has invalid {key}')
        if not re.fullmatch(r'[A-Za-z0-9_-]+', entry['crate']) or not re.fullmatch(
                r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?', entry['version']):
            fail('Security exception must name one crate and an exact version')
        if not re.fullmatch(r'RUSTSEC-\d{4}-\d{4}', entry['advisory']):
            fail('Security exception requires a RUSTSEC advisory ID')
        expires = entry['expires']
        if isinstance(expires, str):
            expires = date.fromisoformat(expires)
        if type(expires) is not date:
            fail('Security exception expires must be a UTC date (YYYY-MM-DD)')
        key = (entry['crate'], entry['version'])
        if key in exceptions:
            fail(f'Duplicate security exception: {key}')
        if now.date() > expires:
            fail(f'Expired security exception: {key}; remove it from policy.toml')
        exceptions[key] = {**entry, 'expires': expires}
    return exceptions


def check_age(package, record, now, exception=None):
    if record['cksum'] != package['checksum']:
        fail(f'Checksum mismatch for {package["name"]}@{package["version"]}')
    published = datetime.fromisoformat(record['pubtime'].replace('Z', '+00:00'))
    if published.tzinfo is None or published > now:
        fail(f'Invalid/future publish timestamp for {package["name"]}')
    eligible = published + timedelta(days=7)
    if exception is not None:
        if (exception['crate'], exception['version']) != (package['name'], package['version']):
            fail('Security exception does not match the locked crate/version')
        if exception['expires'] != eligible.date():
            fail('Security exception expiry must equal the registry publish date + 7 days')
        if now >= eligible:
            fail(f'Unused security exception: {package["name"]}@{package["version"]} '
                 'has completed its soak; remove it from policy.toml')
        print(f'ACTIVE security exception: {exception["crate"]}@{exception["version"]}; '
              f'{exception["advisory"]}; {exception["reason"]}; '
              f'expires {exception["expires"]} (eligible {eligible.isoformat()})')
        return True
    if now < eligible:
        fail(f'Supply-chain soak: {package["name"]}@{package["version"]} was published '
             f'{published.isoformat()}; eligible {eligible.isoformat()} (7 days). '
             'Choose an older version; a pre-filled Cargo.lock does not waive the soak.')
    return False


def check_unused(exceptions, used):
    unused = set(exceptions) - used
    if unused:
        fail(f'Unused security exceptions: {sorted(unused)}; remove them from policy.toml')


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
    exceptions = security_exceptions(tomllib.loads((root / 'scripts/ci/policy.toml').read_text()), now)
    used = set()
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
        key = (name, package['version'])
        if check_age(package, cache[name][package['version']], now, exceptions.get(key)):
            used.add(key)
        count += 1
    check_unused(exceptions, used)
    print(f'PASS soak: {count} locked registry versions; {len(used)} security exceptions; pinned resolver policy active')


if __name__ == '__main__':
    entrypoint(main)
