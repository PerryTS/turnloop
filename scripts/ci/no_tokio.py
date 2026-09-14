"""Audit default AND all-feature workspace graphs on each supported target."""
import argparse
from pathlib import Path
import re
import tomllib
from common import PIN, P3_PIN, ROOT, cargo, entrypoint, fail, members, metadata, run


def check_tree(tree, exception):
    found = []
    for line in tree.splitlines():
        match = re.fullmatch(r'tokio v([^ |]+)(?: \([^|]*\))?\|([^|]*?)(?: \(\*\))?', line)
        if match:
            features = set(filter(None, match[2].split(',')))
            if not exception:
                fail(f'tokio {match[1]} found; DESIGN.md §5b exception is disabled')
            if features != {'io-util'}:
                fail(f'tokio {match[1]} has forbidden features {sorted(features - {"io-util"})}; '
                     'the exception requires exactly io-util')
            found.append(match[1])
        elif line.startswith('tokio '):
            fail(f'Unrecognized tokio tree row (fail closed): {line}')
    return set(found)


def check_ancestry(tree, workspace_names):
    path = []
    roots = 0
    for line in tree.splitlines():
        match = re.match(r'^(\d+)([^ ]+)', line)
        if not match:
            fail(f'Unrecognized inverse tree row: {line}')
        depth, name = int(match[1]), match[2]
        path[depth:] = [name]
        if name in workspace_names:
            roots += 1
            if 'h2' not in path:
                fail(f'tokio dependency bypasses h2: {path}')
    if not roots:
        fail('Could not prove tokio -> h2 -> workspace ancestry')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', default='Cargo.toml')
    parser.add_argument('--target', action='append', help='Repeatable; default is every policy triple')
    args = parser.parse_args()
    policy = tomllib.loads((ROOT / 'scripts/ci/policy.toml').read_text())
    exception = policy['h2_exception']
    if exception['enabled'] and not exception['reason'].strip():
        fail('The h2 exception requires a reviewed, non-empty reason')
    data = metadata(args.manifest_path)
    names = {p['name'] for p in members(data)}
    targets = args.target or policy['targets']['triples']
    for target in targets:
        toolchain = P3_PIN if target == 'wasm32-wasip3' else PIN
        base = cargo(toolchain) + ['tree', '--locked', '--manifest-path',
                str(Path(args.manifest_path).resolve()), '--workspace', '--target', target,
                '--edges', 'normal,build,dev', '--no-dedupe', '--charset', 'ascii']
        for features in ([], ['--all-features']):
            tree = run(base + features + ['--prefix', 'none', '--format', '{p}|{f}'],
                       cwd=data['workspace_root'], capture=True)
            versions = check_tree(tree, exception['enabled'])
            for version in versions:
                inverse = run(base + features + ['--invert', 'tokio@' + version,
                    '--prefix', 'depth', '--format', '{p}'], cwd=data['workspace_root'], capture=True)
                check_ancestry(inverse, names)
            print(f'PASS no-tokio {target} {features or "default features"}; {len(versions)} reviewed exceptions')


if __name__ == '__main__':
    entrypoint(main)
