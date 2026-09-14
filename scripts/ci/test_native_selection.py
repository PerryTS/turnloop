"""Portable member execution remains mandatory while Windows IOCP is pending."""
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from common import ROOT
from feature_modes import native_modes
from test_gates import module, package


class NativeSelection(unittest.TestCase):
    def run_native(self, *, windows=True, pending=True, empty=None, platform=None, mode=None):
        runner = module('run-tests')
        packages = [package('turnloop'), package('pg'), package('mysql'), package('redis'),
                    package('smtp'), package('mongo'), package('new-http'), package('turnloop-contract')]
        for p in packages:
            p['metadata'] = {'turnloop-ci': {'role': 'protocol'}}
        packages[0]['metadata']['turnloop-ci']['role'] = 'core'
        packages[-1]['metadata']['turnloop-ci'] = {'role': 'contract'}
        packages[-1]['dependencies'] = [{'name': 'turnloop'}]
        if pending:
            packages[-1]['metadata']['turnloop-ci']['windows-contracts-pending'] = 'WINDOWS_HANDOFF.md'
        data = dict(packages=packages, workspace_members=[p['id'] for p in packages], workspace_root='/workspace')
        commands = []
        original = runner.checked_tests
        def execute(command, **kwargs):
            commands.append(command)
            selected = command[command.index('-p') + 1] if '-p' in command else 'workspace'
            count = 0 if selected == empty or (selected == 'turnloop-contract' and windows) else 1
            original([sys.executable, '-c', f'print("test result: ok. {count} passed; 0 failed;")'], **kwargs)
        with tempfile.TemporaryDirectory() as folder:
            summary = Path(folder) / 'summary.md'
            with patch.object(runner, 'checked_tests', side_effect=execute), \
                 patch.dict(os.environ, {'GITHUB_STEP_SUMMARY': str(summary)}):
                runner.native_tests(data, ['cargo', 'test'], ROOT, windows=windows,
                                    modes=native_modes(platform or ('win32' if windows else 'darwin'), mode))
            return commands, summary.read_text() if summary.exists() else ''

    def test_windows_requires_every_portable_member_in_all_applicable_modes(self):
        commands, summary = self.run_native()
        self.assertEqual(len(commands), 24)  # workspace + seven portable members, three modes
        for name in ('turnloop', 'pg', 'mysql', 'redis', 'smtp', 'mongo', 'new-http'):
            selected = [c for c in commands if name in c]
            self.assertEqual(len(selected), 3)
            self.assertEqual(sum('--all-features' in c for c in selected), 1)
        self.assertFalse(any('turnloop-contract' in c for c in commands))
        self.assertIn('PENDING Windows backend contracts: turnloop-contract', summary)
        self.assertIn('WINDOWS_HANDOFF.md', summary)
        self.assertIn('no-spin', summary)

    def test_zero_workspace_core_or_protocol_count_still_fails(self):
        for name in ('workspace', 'turnloop', 'pg', 'mysql', 'redis', 'smtp', 'mongo', 'new-http'):
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
                self.run_native(empty=name)

    def test_pending_marker_does_not_exempt_unix_or_unmarked_windows_contracts(self):
        with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
            self.run_native(windows=False, empty='turnloop-contract')
        with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
            self.run_native(pending=False)
        commands, summary = self.run_native(windows=False)
        self.assertEqual(len([c for c in commands if 'turnloop-contract' in c]), 3)
        self.assertEqual(summary, '')

    def test_each_linux_mode_selects_workspace_and_all_members_with_exact_features(self):
        for mode, features in native_modes('linux'):
            with self.subTest(mode=mode):
                commands, _ = self.run_native(windows=False, platform='linux', mode=mode)
                self.assertEqual(len(commands), 9)
                for command in commands:
                    end = command.index('--')
                    selection = command[2:end]
                    actual = selection[1:] if '--workspace' in selection else selection[2:]
                    if '--workspace' in selection or 'turnloop-contract' in selection:
                        expected = features
                    elif features == ['--all-features'] or not features:
                        expected = features
                    elif 'turnloop' in selection:
                        expected = ['--features', ','.join(f for f in features[1].split(',')
                                                         if f.startswith('turnloop/'))]
                    else:
                        expected = []  # Independent sans-IO protocol has no core dependency.
                    self.assertEqual(actual, expected)
                    self.assertEqual(command[end:], ['--', '--test-threads=1'])
                self.assertEqual(sum('turnloop-contract' in c for c in commands), 1)
                with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
                    self.run_native(windows=False, platform='linux', mode=mode, empty='turnloop-contract')


if __name__ == '__main__':
    unittest.main()
