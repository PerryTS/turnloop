"""Real subprocess probes: one failing/empty protocol suite cannot hide later work."""
from contextlib import redirect_stdout, redirect_stderr
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
from test_gates import module, package
from common import ROOT


class ProtocolRunner(unittest.TestCase):
    def test_failure_zero_tests_and_bad_metadata_do_not_short_circuit(self):
        runner = module('run-tests')
        packages = [package(name) for name in ('pg', 'mysql', 'redis', 'mongo', 'smtp', 'http')]
        for p in packages:
            p['targets'] = [{'name': 'server', 'kind': ['test']}]
            p['metadata'] = {'turnloop-ci': {'integration-tests': ['server']}}
        # Also keep going within a crate and past invalid/missing declarations.
        packages[0]['metadata']['turnloop-ci']['integration-tests'] += ['second']
        packages[0]['targets'] += [{'name': 'second', 'kind': ['test']}]
        packages[3]['metadata']['turnloop-ci']['integration-tests'] = ['absent', 'server']
        packages[4]['metadata']['turnloop-ci']['integration-tests'] = []
        with tempfile.TemporaryDirectory() as folder:
            log = Path(folder) / 'executed.txt'
            fake = Path(folder) / 'cargo.py'
            fake.write_text('''import os, pathlib, sys
name = sys.argv[sys.argv.index('-p') + 1]
target = sys.argv[sys.argv.index('--test') + 1]
assert os.environ['TURNLOOP_TEST_REQUIRED'] == '1'
assert sys.argv[-4:] == ['--', '--include-ignored', '--test-threads=1', '--nocapture']
with pathlib.Path(sys.argv[1]).open('a') as log: log.write(name + '/' + target + '\\n')
if name == 'pg' and target == 'server':
    print('test result: FAILED. 2 passed; 1 failed;')
    raise SystemExit(23)
if name == 'redis': print('test result: ok. 0 passed; 0 failed;')
else: print('test result: ok. 3 passed; 0 failed;')
''')
            summary = Path(folder) / 'summary.md'
            summary.write_text('Earlier step\n')
            with patch.dict(os.environ, {'GITHUB_STEP_SUMMARY': str(summary)}), \
                 redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()), \
                 self.assertRaisesRegex(RuntimeError, 'Protocol suites failed'):
                runner.protocol_tests(packages, [sys.executable, str(fake), str(log)], ROOT,
                                      {**os.environ, 'TURNLOOP_TEST_REQUIRED': '1'})
            self.assertEqual(log.read_text().splitlines(),
                             ['pg/server', 'pg/second', 'mysql/server', 'redis/server', 'mongo/server', 'http/server'])
            table = summary.read_text()
            self.assertTrue(table.startswith('Earlier step\n'))
            for suite in ('pg/server', 'redis/server', 'mongo/absent', 'smtp'):
                self.assertIn(f'| {suite} | FAIL |', table)
            for suite in ('pg/second', 'mysql/server', 'mongo/server', 'http/server'):
                self.assertIn(f'| {suite} | PASS | 3 tests passed |', table)

    def test_all_pass_and_no_suites(self):
        runner = module('run-tests')
        p = package('one')
        p['targets'] = [{'name': 'server', 'kind': ['test']}]
        p['metadata'] = {'turnloop-ci': {'integration-tests': ['server']}}
        with patch.dict(os.environ, {}, clear=True), redirect_stdout(io.StringIO()):
            result = runner.protocol_tests([p], [sys.executable, '-c',
                'print("test result: ok. 2 passed; 0 failed;")'], ROOT, None)
            self.assertEqual(result, [('one/server', 'PASS', '2 tests passed')])
            with self.assertRaisesRegex(RuntimeError, 'Protocol suites failed'):
                runner.protocol_tests([], [], ROOT, None)

    def test_spawn_failure_is_reported_and_later_suite_runs(self):
        runner = module('run-tests')
        p = package('one')
        p['targets'] = [{'name': name, 'kind': ['test']} for name in ('first', 'second')]
        p['metadata'] = {'turnloop-ci': {'integration-tests': ['first', 'second']}}
        original = runner.checked_tests
        called = []
        def execute(command, **kwargs):
            called.append(command)
            if len(called) == 1:
                return original(['/no/such/executable'], **kwargs)
            return original([sys.executable, '-c', 'print("test result: ok. 1 passed;")'], **kwargs)
        with patch.dict(os.environ, {}, clear=True), patch.object(runner, 'checked_tests', side_effect=execute), \
             redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()), self.assertRaises(RuntimeError):
            runner.protocol_tests([p], [], ROOT, None)
        self.assertEqual(len(called), 2)

    def test_wasi_real_servers_runs_native_setup_and_requires_actual_execution(self):
        runner = module('run-tests')
        p = package('mongo')
        p['manifest_path'] = str(ROOT / 'protocols/mongo/Cargo.toml')
        p['metadata'] = {'turnloop-ci': {'role': 'protocol', 'wasi-tests': ['socket'],
            'wasi-setup-tests': ['bootstrap'], 'wasi-integration-tests': ['real']}}
        p['targets'] = [{'name': name, 'kind': ['test'], 'required-features': ['turnloop']}
                        for name in ('socket', 'bootstrap', 'real')]
        data = {'workspace_root': str(ROOT), 'workspace_members': ['mongo'], 'packages': [p]}
        argv = ['run-tests.py', 'protocol-wasi', '--target', 'wasm32-wasip2', '--real-servers']
        for count in (2, 0):
            with self.subTest(passed=count), tempfile.TemporaryDirectory() as folder:
                log = Path(folder) / 'calls.txt'
                script = Path(folder) / 'cargo.py'
                script.write_text("import pathlib,sys,os\n"
                    "assert os.environ['TURNLOOP_TEST_REQUIRED'] == '1'\n"
                    "with pathlib.Path(sys.argv[1]).open('a') as f: f.write(' '.join(sys.argv[2:]) + '\\n')\n"
                    f"print('test result: ok. {count} passed; 0 failed;')\n")
                with patch.object(sys, 'argv', argv), patch.object(runner, 'metadata', return_value=data), \
                     patch.object(runner, 'cargo', return_value=[sys.executable, str(script), str(log)]), \
                     redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    if count:
                        runner.main()
                    else:
                        with self.assertRaises(RuntimeError):
                            runner.main()
                calls = log.read_text().splitlines()
                self.assertEqual(len(calls), 3 if count else 1)
                self.assertIn('--test bootstrap', calls[0])
                self.assertNotIn('--target', calls[0])
                if count:
                    self.assertIn('--test socket', calls[1])
                    self.assertNotIn('--include-ignored', calls[1])
                    self.assertIn('--target wasm32-wasip2', calls[2])
                    self.assertIn('--test real', calls[2])
                    self.assertIn('--include-ignored', calls[2])
                    self.assertIn('--features turnloop', calls[2])

    def test_p3_allocator_profile_keeps_the_same_suite_and_required_features(self):
        runner = module('run-tests')
        p = package('db')
        p['metadata'] = {'turnloop-ci': {'role': 'protocol', 'wasi-tests': ['asynchronous'],
                                      'wasi-p3-release-tests': ['asynchronous']}}
        p['targets'] = [{'name': 'asynchronous', 'kind': ['test'], 'required-features': ['turnloop']}]
        data = {'workspace_root': '/workspace', 'workspace_members': ['db'], 'packages': [p]}
        for target in ('wasm32-wasip2', 'wasm32-wasip3'):
            with self.subTest(target=target), patch.object(sys, 'argv', ['run-tests.py', 'protocol-wasi', '--target', target]), \
                 patch.object(runner, 'metadata', return_value=data), patch.object(runner, 'checked_tests') as checked:
                runner.main()
                checked.assert_called_once()
                command = checked.call_args.args[0]
                self.assertEqual(command[command.index('--test') + 1], 'asynchronous')
                self.assertEqual('--release' in command, target == 'wasm32-wasip3')
                self.assertIn('--all-features' if target.endswith('p3') else '--features', command)
                self.assertEqual(command[-2:], ['--', '--test-threads=1'])


if __name__ == '__main__':
    unittest.main()
