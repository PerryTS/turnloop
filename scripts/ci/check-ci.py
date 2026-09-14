#!/usr/bin/env python3
"""Verify ci-gate on the exact push-to-main commit using authenticated GitHub APIs."""
import argparse
import os
import re
from common import entrypoint, fail, get_json

REPOSITORY = 'PerryTS/turnloop'


def validate_run(run, sha):
    if (run['head_sha'] != sha or run['event'] != 'push' or run['head_branch'] != 'main'
            or run['head_repository']['full_name'] != REPOSITORY
            or run['path'] != '.github/workflows/ci.yml'
            or run['status'] != 'completed' or run['conclusion'] != 'success'):
        fail('Release requires a successful ci.yml push-to-main run for the exact release SHA')


def verify(sha, run_id, token):
    if not re.fullmatch(r'[0-9a-f]{40}', sha) or not str(run_id).isdigit():
        fail('Invalid release SHA or CI run ID')
    base = f'https://api.github.com/repos/{REPOSITORY}'
    run = get_json(f'{base}/actions/runs/{run_id}', token=token)
    validate_run(run, sha)
    # Select the latest attempt, not a green job from a superseded attempt.
    jobs = []
    page = 1
    while True:
        batch = get_json(f'{base}/actions/runs/{run_id}/attempts/{run["run_attempt"]}/jobs?per_page=100&page={page}', token=token)['jobs']
        jobs.extend(batch)
        if len(batch) < 100:
            break
        page += 1
    gates = [job for job in jobs if job['name'] == 'ci-gate']
    if len(gates) != 1 or gates[0]['conclusion'] != 'success' or gates[0]['status'] != 'completed':
        fail('Exact commit ci-gate is missing, skipped, pending, or unsuccessful')
    print(f'PASS ci-gate {sha} run {run_id} attempt {run["run_attempt"]}')
    return base


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sha', default=os.environ.get('RELEASE_SHA'))
    parser.add_argument('--run-id', default=os.environ.get('CI_RUN_ID'))
    parser.add_argument('--release-pr', action='store_true')
    args = parser.parse_args()
    base = verify(args.sha or '', args.run_id or '', os.environ['GH_TOKEN'])
    if args.release_pr:
        pulls = get_json(f'{base}/commits/{args.sha}/pulls?per_page=100', token=os.environ['GH_TOKEN'])
        release = any(p.get('merged_at') and p['base']['ref'] == 'main'
            and p['base']['repo']['full_name'] == REPOSITORY
            and p['head']['repo'] and p['head']['repo']['full_name'] == REPOSITORY
            and p['head']['ref'].startswith('release-plz-')
            and p['merge_commit_sha'] == args.sha for p in pulls)
        if os.environ.get('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'], 'a', encoding='utf-8') as output:
                output.write('release=' + str(release).lower() + '\n')
        print(f'Merged release PR: {release}')


if __name__ == '__main__':
    entrypoint(main)
