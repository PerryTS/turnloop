#!/usr/bin/env python3
"""Validate GitHub's queue extension before running the pinned workflow linters.

actionlint v1.7.12 predates concurrency.queue. Its one unsupported-key diagnostic
is filtered ONLY after checking the exact required queue/cancellation expressions.
Every other actionlint diagnostic remains fatal. Remove this compatibility check
when an official actionlint release supports queue.
"""
from pathlib import Path
import re
from common import ROOT, entrypoint, fail, run


def check_queue(path):
    source = path.read_text()
    match = re.search(r'^concurrency:\n((?:[ \t]+[^\n]*\n)+)', source, re.MULTILINE)
    if not match:
        fail(f'{path}: workflow concurrency is required')
    values = dict(re.findall(r'^  ([a-z-]+): (.+)$', match[1], re.MULTILINE))
    ci = path.name == 'ci.yml'
    expected_queue = "${{ github.event_name == 'pull_request' && 'single' || 'max' }}" if ci else 'max'
    expected_cancel = "${{ github.event_name == 'pull_request' }}" if ci else 'false'
    if values.get('queue') != expected_queue or values.get('cancel-in-progress') != expected_cancel:
        fail(f'{path}: main must queue:max and never cancel; PRs alone may cancel')
    # Reject unvalidated extra occurrences, including job-level queues.
    if len(re.findall(r'^\s*queue:', source, re.MULTILINE)) != 1:
        fail(f'{path}: every queue property must be validated')


def main():
    paths = sorted((ROOT / '.github/workflows').glob('*.yml'))
    if not paths:
        fail('No workflows')
    for path in paths:
        check_queue(path)
    run(['actionlint', '-ignore', '^unexpected key "queue" for "concurrency" section\\. expected one of "cancel-in-progress", "group"$', *map(str, paths)])
    run(['zizmor', '--offline', '--min-severity', 'low', str(ROOT / '.github/workflows')])
    run(['shellcheck', *map(str, sorted((ROOT / 'scripts/ci').glob('*.sh')))])


if __name__ == '__main__':
    entrypoint(main)
