"""Portable member execution remains mandatory while Windows IOCP is pending."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from common import ROOT
from test_gates import module, package


class NativeSelection(unittest.TestCase):
    def run_native(self, *, windows=True, pending=True, empty=None):
        runner = module('run-tests')
        packages = [package('core'), package('pg'), package('mysql'), package('redis'),
                    package('smtp'), package('mongo'), package('new-http'), package('contract')]
        for p in packages:
            p['metadata'] = {'turnloop-ci': {'role': 'protocol'}}
        packages[0]['metadata']['turnloop-ci']['role'] = 'core'
        packages[-1]['metadata']['turnloop-ci'] = {'role': 'contract'}
        if pending:
            packages[-1]['metadata']['turnloop-ci']['windows-contracts-pending'] = 'WINDOWS_HANDOFF.md'
        data = dict(packages=packages, workspace_members=[p['id'] for p in packages], workspace_root='/workspace')
        commands = []
        original = runner.checked_tests
        def execute(command, **kwargs):
            commands.append(command)
            selected = command[command.index('-p') + 1] if '-p' in command else 'workspace'
            count = 0 if selected == empty or (selected == 'contract' and windows) else 1
            original([sys.executable, '-c', f'print("test result: ok. {count} passed; 0 failed;")'], **kwargs)
        with tempfile.TemporaryDirectory() as folder:
            summary = Path(folder) / 'summary.md'
            with patch.object(runner, 'checked_tests', side_effect=execute), \
                 patch.dict(os.environ, {'GITHUB_STEP_SUMMARY': str(summary)}):
                runner.native_tests(data, ['cargo', 'test'], ROOT, windows=windows)
            return commands, summary.read_text() if summary.exists() else ''

    def test_windows_requires_every_portable_member_in_both_feature_modes(self):
        commands, summary = self.run_native()
        self.assertEqual(len(commands), 16)  # workspace + seven portable members, twice
        for name in ('core', 'pg', 'mysql', 'redis', 'smtp', 'mongo', 'new-http'):
            selected = [c for c in commands if name in c]
            self.assertEqual(len(selected), 2)
            self.assertEqual(sum('--all-features' in c for c in selected), 1)
        self.assertFalse(any('contract' in c for c in commands))
        self.assertIn('PENDING Windows backend contracts: contract', summary)
        self.assertIn('WINDOWS_HANDOFF.md', summary)
        self.assertIn('no-spin', summary)

    def test_zero_workspace_core_or_protocol_count_still_fails(self):
        for name in ('workspace', 'core', 'pg', 'mysql', 'redis', 'smtp', 'mongo', 'new-http'):
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
                self.run_native(empty=name)

    def test_pending_marker_does_not_exempt_unix_or_unmarked_windows_contracts(self):
        with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
            self.run_native(windows=False, empty='contract')
        with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
            self.run_native(pending=False)
        commands, summary = self.run_native(windows=False)
        self.assertEqual(len([c for c in commands if 'contract' in c]), 2)
        self.assertEqual(summary, '')


if __name__ == '__main__':
    unittest.main()
