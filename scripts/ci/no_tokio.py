"""Audit default AND all-feature workspace graphs on every supported target."""
import argparse
from pathlib import Path
import re
import tomllib
from common import PIN, P3_PIN, ROOT, cargo, entrypoint, fail, members, metadata, run

POLICY = tomllib.loads((ROOT / 'scripts/ci/policy.toml').read_text())
BANNED = frozenset(POLICY['dependencies']['banned'])


def check_tree(tree):
    count = 0
    for line in tree.splitlines():
        if not line.strip():
            continue
        match = re.match(r'^([A-Za-z0-9_-]+) v[^ |]+(?: .*)?\|', line)
        if not match:
            fail(f'Unrecognized cargo tree row (fail closed): {line}')
        name = match[1]
        if name in BANNED:
            fail(f'Forbidden dependency {name}: {line}. DESIGN.md §5b permits no exceptions.')
        count += 1
    if not count:
        fail('Empty cargo tree does not prove a dependency audit ran')
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', default='Cargo.toml')
    parser.add_argument('--target', action='append', help='Repeatable; defaults to every policy triple')
    args = parser.parse_args()
    policy = tomllib.loads((ROOT / 'scripts/ci/policy.toml').read_text())
    data = metadata(args.manifest_path)
    members(data)
    for target in args.target or policy['targets']['triples']:
        toolchain = P3_PIN if target == 'wasm32-wasip3' else PIN
        base = cargo(toolchain) + ['tree', '--locked', '--manifest-path',
                str(Path(args.manifest_path).resolve()), '--workspace', '--target', target,
                '--edges', 'normal,build,dev', '--charset', 'ascii',
                '--prefix', 'none', '--format', '{p}|{f}']
        for features in ([], ['--all-features']):
            tree = run(base + features, cwd=data['workspace_root'], capture=True)
            count = check_tree(tree)
            print(f'PASS no-tokio {target} {features or "default features"}: {count} package rows')


if __name__ == '__main__':
    entrypoint(main)
