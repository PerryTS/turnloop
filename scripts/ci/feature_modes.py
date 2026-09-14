#!/usr/bin/env python3
"""Require explicit runtime CI coverage for every public turnloop Cargo feature.

The native matrix uses JSON flow mappings (a YAML subset), so this gate and the
runner consume exactly the same rows without installing a second YAML parser.
Unknown matrix syntax fails closed; all-features never covers a new feature here.
"""
import json
import re
import tomllib
from common import ROOT, entrypoint, fail


LINUX = ('ubuntu-24.04', 'ubuntu-24.04-arm')
NATIVE = (*LINUX, 'macos-15', 'windows-2025')


def job_block(source, name):
    match = re.search(r'^  ' + re.escape(name) + r':\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)',
                      source, re.MULTILINE | re.DOTALL)
    if not match:
        fail(f'Missing required job: {name}')
    return match[1]


def matrix_rows(source):
    block = job_block(source, 'test-native')
    match = re.search(r'^      matrix:\n((?: {8,}[^\n]*\n)+)', block, re.MULTILINE)
    if not match:
        fail('test-native needs an explicit include matrix')
    lines = [line.strip() for line in match[1].splitlines()
             if line.strip() and not line.strip().startswith('#')]
    if not lines or lines.pop(0) != 'include:':
        fail('Native matrix must contain only explicit include rows')
    rows = []
    for line in lines:
        if not line.startswith('- {'):
            fail(f'Unsupported native matrix syntax: {line}')
        row = json.loads(line[2:])
        if set(row) != {'os', 'mode', 'features'} or not all(isinstance(v, str) for v in row.values()):
            fail('Each native matrix row needs string os, mode and features fields')
        rows.append(row)
    if not rows:
        fail('Native matrix cannot be empty')
    return rows


def feature_args(features):
    if features == 'all':
        return ['--all-features']
    if not features:
        return []
    if any(not re.fullmatch(r'[a-z][a-z0-9-]*/[a-z][a-z0-9-]*', f) for f in features.split(',')):
        fail(f'Use explicit package/feature names: {features}')
    return ['--features', features]


def core_features(row):
    return {f.removeprefix('turnloop/') for f in row['features'].split(',')
            if f.startswith('turnloop/')}


def public_features(manifest):
    declared = manifest.get('features', {})
    namespaced = {value.removeprefix('dep:') for values in declared.values()
                  for value in values if value.startswith('dep:')}
    # Optional dependencies create implicit public Cargo features unless dep:
    # syntax suppresses that name. Include target-specific optional dependencies.
    optional = set()
    for table in [manifest, *manifest.get('target', {}).values()]:
        for section in ('dependencies', 'build-dependencies'):
            optional.update(name for name, spec in table.get(section, {}).items()
                            if isinstance(spec, dict) and spec.get('optional'))
    return set(declared) | (optional - namespaced)


def check(source, manifest):
    rows = matrix_rows(source)
    block = job_block(source, 'test-native')
    if re.search(r'^\s*(?:if|continue-on-error|exclude):', block, re.MULTILINE):
        fail('Native modes must be unconditional and required')
    for required in ('fail-fast: false', 'runs-on: ${{ matrix.os }}',
                     'NATIVE_MODE: ${{ matrix.mode }}',
                     'run-tests.py native --mode "$NATIVE_MODE"',
                     'run-tests.py interop --mode "$NATIVE_MODE"'):
        if required not in block:
            fail(f'Native matrix is not wired to execution: missing {required}')
    needs = re.search(r'^    needs: \[(.+)\]$', job_block(source, 'ci-gate'), re.MULTILINE)
    if not needs or not {'test-native', 'workflow-lint'} <= set(needs[1].split(', ')):
        fail('ci-gate must require native modes and the feature coverage gate')
    if 'python3 scripts/ci/feature_modes.py' not in job_block(source, 'workflow-lint'):
        fail('workflow-lint must execute the feature coverage gate')
    features = public_features(manifest)
    covered = set()
    seen = set()
    by_os = {os: {} for os in NATIVE}
    for row in rows:
        os, mode, selected = row['os'], row['mode'], row['features']
        if os not in by_os or not re.fullmatch(r'[a-z][a-z0-9-]*', mode):
            fail(f'Unknown native runner or invalid mode: {row}')
        if (os, mode) in seen:
            fail(f'Duplicate native mode: {os}/{mode}')
        seen.add((os, mode))
        feature_args(selected)
        explicit = core_features(row)
        if unknown := explicit - features:
            fail(f'Unknown turnloop features in matrix: {sorted(unknown)}')
        if os not in LINUX and explicit & {'epoll-timerfd', 'process-sigchld'}:
            fail(f'Linux fallback cannot be exercised by {os}')
        by_os[os][mode] = selected
        covered.update(explicit)
        if mode == 'default' and selected == '':
            covered.add('default')
    for os, modes in by_os.items():
        required = {'default': '', 'executor': 'turnloop/executor,turnloop-contract/executor',
                    'all-features': 'all'}
        if os in LINUX:
            required.update({'epoll-timerfd': 'turnloop/epoll-timerfd',
                             'process-sigchld': 'turnloop/process-sigchld',
                             'fallbacks': 'turnloop/epoll-timerfd,turnloop/process-sigchld'})
        for mode, selected in required.items():
            if modes.get(mode) != selected:
                fail(f'Missing or changed required mode: {os}/{mode} ({selected})')
    if missing := features - covered:
        fail(f'Public turnloop features without explicit runtime CI arms: {sorted(missing)}')
    return rows


def native_modes(platform, mode=None, *, source=None):
    rows = matrix_rows(source if source is not None else (ROOT / '.github/workflows/ci.yml').read_text())
    os = {'linux': LINUX[0], 'darwin': 'macos-15', 'win32': 'windows-2025'}.get(platform)
    if os is None:
        fail(f'No required native CI modes for platform {platform}')
    selected = [(r['mode'], feature_args(r['features'])) for r in rows
                if r['os'] == os and (mode is None or r['mode'] == mode)]
    if not selected:
        fail(f'Native mode {mode} is not applicable to {platform}')
    return selected


def main():
    source = (ROOT / '.github/workflows/ci.yml').read_text()
    manifest = tomllib.loads((ROOT / 'crates/turnloop/Cargo.toml').read_text())
    rows = check(source, manifest)
    print(f'PASS {len(public_features(manifest))} public features, {len(rows)} required native CI arms')


if __name__ == '__main__':
    entrypoint(main)
