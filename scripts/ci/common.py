"""Shared, stdlib-only CI helpers. Never discover crates by directory globbing."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import urllib.request

PIN = 'nightly-2026-08-20'
P3_PIN = 'nightly-2026-09-07'
ROOT = Path(__file__).resolve().parents[2]


def fail(message):
    raise RuntimeError(message)


def run(command, *, cwd=None, env=None, capture=False):
    print('+ ' + shlex.join(map(str, command)), file=sys.stderr, flush=True)
    result = subprocess.run(command, cwd=cwd, env=env, text=True,
                            stdout=subprocess.PIPE if capture else None, check=True)
    return result.stdout if capture else None


def cargo(toolchain=PIN):
    return ['cargo', '+' + toolchain]


def metadata(manifest='Cargo.toml', *, toolchain=PIN, resolved=False, target=None):
    manifest = Path(manifest).resolve()
    if not manifest.is_file():
        fail(f'Workspace manifest missing: {manifest}; integrate the core workspace first.')
    args = cargo(toolchain) + ['metadata', '--format-version', '1', '--locked',
                              '--manifest-path', str(manifest)]
    if not resolved:
        args += ['--no-deps']
    else:
        args += ['--all-features']
    if target:
        args += ['--filter-platform', target]
    return json.loads(run(args, cwd=manifest.parent, capture=True))


def members(data):
    ids = set(data['workspace_members'])
    result = [p for p in data['packages'] if p['id'] in ids]
    if not result:
        fail('cargo metadata reported zero workspace members')
    root = Path(data['workspace_root'])
    for package in result:
        rel = Path(package['manifest_path']).relative_to(root)
        if rel.parts[0] == 'spikes':
            fail(f'Spike is a workspace member: {rel}; exclude it from the workspace.')
    return result


def settings(package):
    return (package.get('metadata') or {}).get('turnloop-ci', {})


def role(package):
    explicit = settings(package).get('role')
    if explicit:
        return explicit
    # Classify helper roles without a hard-coded package list.
    name = package['name']
    if name.endswith('-contract'):
        return 'contract'
    if name.endswith('-bench'):
        return 'bench'
    if Path(package['manifest_path']).parent.parent.name == 'protocols':
        return 'protocol'
    return 'core'


def select(data, wanted):
    result = [p for p in members(data) if role(p) == wanted]
    if not result:
        fail(f'No {wanted} member found through cargo metadata')
    return result


def publish_order(data):
    selected = {p['name']: p for p in members(data)
                if p['publish'] is None or 'crates-io' in p['publish']}
    ordered, visiting, done = [], set(), set()

    def visit(name):
        if name in visiting:
            fail(f'Cycle in publish dependencies at {name}')
        if name in done:
            return
        visiting.add(name)
        for dep in selected[name]['dependencies']:
            if dep['kind'] == 'dev':
                continue
            if dep.get('path'):
                if dep['name'] not in selected:
                    fail(f'{name} depends on non-publishable workspace/path crate {dep["name"]}')
                if dep['req'] == '*':
                    fail(f'{name} -> {dep["name"]} needs an explicit registry version')
                visit(dep['name'])
        visiting.remove(name)
        done.add(name)
        ordered.append(selected[name])

    for name in sorted(selected):
        visit(name)
    if not ordered:
        fail('No publishable workspace crates')
    return ordered


def get_json(url, *, token=None):
    headers = {'User-Agent': 'PerryTS-turnloop-ci (https://github.com/PerryTS/turnloop)',
               'Accept': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=60) as response:
        return json.load(response)


def entrypoint(main):
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError, ValueError, KeyError) as error:
        print(f'ERROR: {error}', file=sys.stderr)
        sys.exit(1)
