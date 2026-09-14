#!/usr/bin/env python3
"""A skipped required check is green on GitHub; explicitly reject unexpected skips."""
import json
import os
from common import entrypoint, fail


def check(needs, self_hosted):
    if not needs:
        fail('ci-gate has no upstream jobs')
    for name, job in needs.items():
        expected = 'skipped' if name == 'self-hosted-windows' and not self_hosted else 'success'
        if job['result'] != expected:
            fail(f'{name}: {job["result"]}; expected {expected}')
    if 'self-hosted-windows' not in needs:
        fail('Optional Windows job is missing from ci-gate needs')
    print(f'PASS ci-gate: inspected {len(needs)} job results')


def main():
    check(json.loads(os.environ['NEEDS_JSON']), os.environ.get('SELF_HOSTED_WINDOWS') == 'true')


if __name__ == '__main__':
    entrypoint(main)
