#!/usr/bin/env python3
"""Workspace release planning and Cargo's atomic preflight, without a crate-name list.

release-plz authors version/changelog PRs. Cargo's workspace publisher stages all
sibling packages before publishing in dependency order, keeping the soak active.
Only the publish subcommand writes to crates.io/GitHub; it is CI-only.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import tarfile
import time
import urllib.error
import urllib.request
from common import PIN, ROOT, cargo, entrypoint, fail, get_json, metadata, publish_order, run


def release_notes(package):
    """Release body for one crate: its own changelog section, or a stated fallback.

    A workspace release bumps every crate, but release-plz only writes a section
    for the crates whose own sources changed. Failing on the others used to abort
    the publish step *after* crates.io uploads had succeeded, leaving the release
    half-tagged (alpha.4: 12 crates published, 1 tag created). The requirement is
    kept where it can still block a release - preflight, before any upload - and a
    version-only bump gets an explicit body instead of stopping a live publish.
    """
    tag = package['name'] + '-v' + package['version']
    changelog = Path(package['manifest_path']).parent / 'CHANGELOG.md'
    if not changelog.is_file():
        fail(f'Missing release-plz changelog for {tag}')
    lines = changelog.read_text().splitlines(keepends=True)
    start = next((i for i, line in enumerate(lines)
                  if line.startswith('## ') and package['version'] in line), None)
    if start is None:
        return (f'{package["name"]} {package["version"]}\n\n'
                'No crate-specific changes; released with the workspace.\n')
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith('## ')), len(lines))
    return ''.join(lines[start:end])


def semver_key(number):
    """SemVer 2.0.0 precedence; build metadata is ignored."""
    core, _, pre = number.split('+', 1)[0].partition('-')
    major, minor, patch = (int(part) for part in core.split('.'))
    if not pre:
        return (major, minor, patch, 1, ())
    ids = tuple((0, int(i), '') if i.isdigit() else (1, 0, i) for i in pre.split('.'))
    return (major, minor, patch, 0, ids)


def semver_baseline(record, version):
    """Highest non-yanked published version below `version`.

    Stable releases are preferred, as before. A crate that has only
    pre-releases (the 0.1.0-alpha series) is checked against its newest earlier
    pre-release instead of failing for lack of a stable baseline.
    """
    target = semver_key(version)
    earlier = [v['num'] for v in record['versions']
               if not v.get('yanked') and semver_key(v['num']) < target]
    stable = [n for n in earlier if '-' not in n.split('+', 1)[0]]
    candidates = stable or earlier
    return max(candidates, key=semver_key) if candidates else None


def baseline_source(name, baseline):
    """Prefer the release tag over a registry download for the semver baseline.

    Building the baseline from crates.io resolves `name = "=<baseline>"`, which
    the seven-day publish-age soak rejects for a version released this week.
    The release tag has the same source (verify_existing pins published archives
    to their commits), resolves workspace siblings by path, and cannot pull a
    freshly published version into this OIDC-enabled job.
    """
    tag = f'{name}-v{baseline}'
    if run(['git', 'tag', '--list', tag], capture=True).strip():
        return ['--baseline-rev', tag]
    return ['--baseline-version', baseline]


def registry(name):
    try:
        return get_json(f'https://crates.io/api/v1/crates/{name}')
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def current_commit():
    return run(['git', 'rev-parse', 'HEAD'], capture=True).strip()


def verify_existing(package, version, sha):
    """Retry only publications from this exact source commit, never relabel older code."""
    name, number = package['name'], package['version']
    if version['yanked']:
        fail(f'{name}@{number} is yanked; create a new version')
    url = f'https://static.crates.io/crates/{name}/{name}-{number}.crate'
    with urllib.request.urlopen(url, timeout=60) as response:
        blob = response.read()
    if hashlib.sha256(blob).hexdigest() != version['checksum']:
        fail(f'{name}@{number}: published archive checksum mismatch')
    with tarfile.open(fileobj=io.BytesIO(blob), mode='r:gz') as archive:
        record = archive.extractfile(f'{name}-{number}/.cargo_vcs_info.json')
        if record is None or json.load(record)['git']['sha1'] != sha:
            fail(f'{name}@{number} is already published from a different commit')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['order', 'preflight', 'publish'])
    parser.add_argument('--manifest-path', default='Cargo.toml')
    args = parser.parse_args()
    data = metadata(args.manifest_path)
    packages = publish_order(data)
    if args.command == 'order':
        for package in packages:
            print(f'{package["name"]}\t{package["version"]}\t{package["manifest_path"]}')
        return
    sha = current_commit()
    if sha != os.environ.get('RELEASE_SHA'):
        fail('Checkout differs from the CI-verified release SHA')
    plan_path = ROOT / '.tools/release-plan.json'
    if args.command == 'preflight':
        pending, recovery = [], []
        for package in packages:
            record = registry(package['name'])
            if record is None:
                fail(f'{package["name"]}: first publish must be performed manually; see RELEASING.md')
            versions = record['versions']
            existing = next((v for v in versions if v['num'] == package['version']), None)
            if existing:
                # Fully released older packages need no new release/tag.
                tag = package['name'] + '-v' + package['version']
                tags = run(['git', 'tag', '--list', tag], capture=True).strip()
                if tags:
                    print(f'Already released: {tag}')
                    continue
                verify_existing(package, existing, sha)
                recovery.append(package['name'])
            else:
                pending.append(package['name'])
            baseline = semver_baseline(record, package['version'])
            if not baseline:
                fail(f'{package["name"]}: no earlier published version for semver-checks')
            run(cargo(PIN) + ['semver-checks', '--manifest-path', package['manifest_path'],
                *baseline_source(package['name'], baseline), '--all-features'], cwd=data['workspace_root'])
        for package in packages:
            if package['name'] in pending:
                release_notes(package)  # fail here, not after uploading to crates.io
        # One invocation checks EACH crate and stages unpublished siblings in a
        # temporary registry. Separate invocations fail for new dependency versions.
        command = cargo(PIN) + ['publish', '--registry', 'crates-io', '--locked', '--dry-run', '--manifest-path', str(Path(args.manifest_path).resolve())]
        for package in packages:
            command += ['-p', package['name']]
        run(command, cwd=data['workspace_root'])
        plan_path.parent.mkdir(parents=True, exist_ok=True)
        plan_path.write_text(json.dumps({'sha': sha, 'pending': pending, 'recovery': recovery,
            'versions': {p['name']: p['version'] for p in packages}}, indent=2) + '\n')
        return
    # crates.io Trusted Publishing issues tokens only to push/release/workflow_dispatch
    # runs; release.yml dispatches itself after verifying the workflow_run.
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('GITHUB_EVENT_NAME') != 'workflow_dispatch':
        fail('Publishing is only supported by the protected release.yml workflow')
    plan = json.loads(plan_path.read_text())
    if plan['sha'] != sha or plan['versions'] != {p['name']: p['version'] for p in packages}:
        fail('Release plan no longer matches the checkout')
    if plan['pending']:
        command = cargo(PIN) + ['publish', '--registry', 'crates-io', '--locked', '--manifest-path', str(Path(args.manifest_path).resolve())]
        for name in plan['pending']:
            command += ['-p', name]
        run(command, cwd=data['workspace_root'])
    for package in packages:
        if package['name'] not in plan['pending'] + plan['recovery']:
            continue
        version = None
        for attempt in range(24):
            record = registry(package['name'])
            version = next((v for v in record['versions'] if v['num'] == package['version']), None) if record else None
            if version is not None:
                break
            if attempt < 23:
                time.sleep(5)
        if version is None:
            fail(f'{package["name"]}: uploaded version is not visible in the crates.io API after 120s')
        verify_existing(package, version, sha)
        tag = package['name'] + '-v' + package['version']
        notes = ROOT / '.tools/release-notes.md'
        notes.parent.mkdir(parents=True, exist_ok=True)
        notes.write_text(release_notes(package))
        run(['gh', 'release', 'create', tag, '--repo', 'PerryTS/turnloop', '--target', sha,
             '--title', tag, '--notes-file', str(notes)])


if __name__ == '__main__':
    entrypoint(main)
