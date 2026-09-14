"""Adversarial tests: policy gates must reject absent work and forbidden inputs."""
from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from common import ROOT, publish_order, members, settings
from no_tokio import BANNED, check_tree
from soak import check_age, index_path


def module(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def package(name, deps=(), publish=None):
    return {'id': name, 'name': name, 'manifest_path': '/workspace/crates/' + name + '/Cargo.toml',
            'publish': publish, 'dependencies': list(deps)}


class Gates(unittest.TestCase):
    def test_all_banned_names_and_alias_output(self):
        for name in BANNED:
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                check_tree(f'core v0.1.0 (/workspace)|\n{name} v1.0.0|io-util\n')
        self.assertEqual(check_tree('tokio-free v1.0.0|\ncore v0.1.0 (/a path)|default\nlibc v0.2.175| (*)'), 3)

    def test_empty_or_malformed_dependency_tree(self):
        for tree in ('', 'cargo failed', 'tokio malformed', '   '):
            with self.subTest(tree=tree), self.assertRaises(RuntimeError):
                check_tree(tree)

    def test_null_package_metadata(self):
        self.assertEqual(settings({'metadata': None}), {})

    def test_publish_order_renames_and_cycle(self):
        deps = [{'name': 'renamed-core', 'path': '/workspace/crates/renamed-core', 'kind': None, 'req': '^0.1'}]
        packages = [package('client', deps), package('renamed-core'), package('private-bench', publish=[])]
        data = {'packages': packages, 'workspace_members': [p['id'] for p in packages], 'workspace_root': '/workspace'}
        self.assertEqual([p['name'] for p in publish_order(data)], ['renamed-core', 'client'])
        packages[1]['dependencies'] = [{'name': 'client', 'path': '/workspace/crates/client', 'kind': None, 'req': '^0.1'}]
        with self.assertRaises(RuntimeError):
            publish_order(data)

    def test_spikes_cannot_be_released(self):
        p = package('spike'); p['manifest_path'] = '/workspace/spikes/p3/Cargo.toml'
        with self.assertRaises(RuntimeError):
            members({'packages': [p], 'workspace_members': ['spike'], 'workspace_root': '/workspace'})

    def test_locked_young_version_and_checksum(self):
        now = datetime(2026, 9, 14, tzinfo=timezone.utc)
        p = {'name': 'dep', 'version': '1.0.0', 'checksum': 'a' * 64}
        record = {'cksum': 'a' * 64, 'pubtime': (now-timedelta(days=7)).isoformat()}
        check_age(p, record, now)
        record['pubtime'] = (now-timedelta(days=6)).isoformat()
        with self.assertRaisesRegex(RuntimeError, 'Supply-chain soak'):
            check_age(p, record, now)
        record['cksum'] = 'b' * 64
        with self.assertRaisesRegex(RuntimeError, 'Checksum mismatch'):
            check_age(p, record, now)
        self.assertEqual([index_path(n) for n in ('a', 'ab', 'abc', 'serde')], ['1/a', '2/ab', '3/a/abc', 'se/rd/serde'])

    def test_fan_in_unexpected_skips_failures_and_cancellations(self):
        gate = module('ci-gate')
        needs = {'test': {'result': 'success'}, 'self-hosted-windows': {'result': 'skipped'}}
        gate.check(needs, False)
        with self.assertRaises(RuntimeError):
            gate.check(needs, True)
        for status in ('skipped', 'failure', 'cancelled'):
            with self.subTest(status=status), self.assertRaises(RuntimeError):
                gate.check({**needs, 'test': {'result': status}}, False)
        with self.assertRaises(RuntimeError):
            gate.check({}, False)

    def test_fan_in_declares_every_job(self):
        import re
        source = (ROOT / '.github/workflows/ci.yml').read_text()
        jobs = set(re.findall(r'^  ([a-z][a-z0-9-]+):$', source.split('jobs:\n')[1], re.MULTILINE))
        declared = re.search(r'^    needs: \[(.+)\]$', source, re.MULTILINE)[1]
        self.assertEqual(set(declared.split(', ')), jobs - {'ci-gate'})
        for workflow in (ROOT / '.github/workflows').glob('*.yml'):
            for action in re.findall(r'uses: ([^\n]+)', workflow.read_text()):
                self.assertRegex(action, r'^[^@]+@[0-9a-f]{40} # v')

    def test_h2spec_rejects_partial_skipped_duplicate_and_failed_reports(self):
        h2 = module('h2spec')
        import xml.etree.ElementTree as ET
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'report.xml'
            report = ET.Element('testsuites')
            suite = ET.SubElement(report, 'testsuite', tests='147', failures='0', errors='0', skipped='0')
            for index in range(147):
                ET.SubElement(suite, 'testcase', package='http2', classname=str(index))
            def check():
                ET.ElementTree(report).write(path)
                h2.check_report(path)
            check()
            for tag in ('failure', 'error', 'skipped'):
                bad = ET.SubElement(suite[0], tag)
                with self.subTest(tag=tag), self.assertRaises(RuntimeError):
                    check()
                suite[0].remove(bad)
            suite[-1].set('classname', '0')
            with self.assertRaises(RuntimeError):
                check()
            suite.remove(suite[-1])
            with self.assertRaises(RuntimeError):
                check()
            report.remove(suite)
            with self.assertRaises(RuntimeError):
                check()

    def test_h2spec_checksum_precedes_extraction(self):
        h2 = module('h2spec')
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
                h2.extract_source(b'not trusted', {'sha256': '0' * 64}, directory)
            self.assertEqual(list(directory.iterdir()), [])

    def test_exact_ci_sha_event_and_workflow(self):
        gate = module('check-ci')
        sha = 'a'*40
        data = dict(head_sha=sha, event='push', head_branch='main',
            head_repository={'full_name': 'PerryTS/turnloop'}, path='.github/workflows/ci.yml',
            status='completed', conclusion='success')
        gate.validate_run(data, sha)
        for key, value in [('head_sha', 'b'*40), ('event', 'pull_request'), ('head_branch', 'feature'),
                           ('conclusion', 'skipped'), ('path', '.github/workflows/fake.yml')]:
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                gate.validate_run({**data, key: value}, sha)
        with patch.object(gate, 'get_json', side_effect=[{**data, 'run_attempt': 2}, {'jobs': []}]):
            with self.assertRaisesRegex(RuntimeError, 'ci-gate is missing'):
                gate.verify(sha, '123', 'fake')

    def test_positive_test_counts_required(self):
        runner = module('run-tests')
        import sys
        runner.checked_tests([sys.executable, '-c', 'print("test result: ok. 2 passed; 0 failed;")'], cwd=ROOT)
        with self.assertRaises(RuntimeError):
            runner.checked_tests([sys.executable, '-c', 'print("test result: ok. 0 passed; 0 failed;")'], cwd=ROOT)

    def test_instruction_regression_missing_and_control(self):
        bench = module('instructions')
        baseline = {'counts': {'control': 100, 'idle': 200}, 'controls': ['control']}
        bench.compare(baseline, [{'control': 100, 'idle': 206}]*3)
        for counts in ({'control': 100, 'idle': 207}, {'control': 101, 'idle': 200},
                       {'control': 100}, {'control': 100, 'idle': 0}):
            with self.subTest(counts=counts), self.assertRaises(RuntimeError):
                bench.compare(baseline, [counts]*3)
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            with self.assertRaises(RuntimeError):
                bench.read_counts(directory)
            record = {'version': '6', 'module_path': 'bench::idle', 'id': None, 'profiles': [
                {'tool': 'Callgrind', 'summaries': {'total': {'summary': {'Callgrind': {'Ir': {'metrics': {'Left': {'Int': 123}}}}}}}}]}
            (directory/'summary.json').write_text(json.dumps(record))
            self.assertEqual(bench.read_counts(directory), {'bench::idle': 123})
            record['profiles'][0]['summaries']['total']['summary']['Callgrind']['Ir']['metrics'] = {'Right': {'Int': 123}}
            (directory/'summary.json').write_text(json.dumps(record))
            with self.assertRaises(RuntimeError):
                bench.read_counts(directory)

    def test_installer_verifies_digest_before_extracting(self):
        import hashlib
        import io
        import tarfile
        installer = module('install-tools')
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w:gz') as bundle:
            contents = b'#!/bin/sh\nexit 0\n'
            info = tarfile.TarInfo('nested/tool')
            info.size = len(contents)
            bundle.addfile(info, io.BytesIO(contents))
        blob = stream.getvalue()
        pin = {'version': 'test', 'url': 'https://example.invalid/tool.tar.gz',
               'sha256': hashlib.sha256(blob).hexdigest(), 'executables': ['tool']}
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            with patch.object(installer.json, 'loads', return_value={'test': {'Linux-x86_64': pin}}), \
                 patch.object(installer.platform, 'system', return_value='Linux'), \
                 patch.object(installer.platform, 'machine', return_value='x86_64'), \
                 patch.object(installer.urllib.request, 'urlopen', return_value=io.BytesIO(blob)):
                installer.install('test', directory)
            self.assertEqual((directory/'tool').read_bytes(), contents)
            (directory/'tool').unlink()
            pin['sha256'] = '0'*64
            with patch.object(installer.json, 'loads', return_value={'test': {'Linux-x86_64': pin}}), \
                 patch.object(installer.platform, 'system', return_value='Linux'), \
                 patch.object(installer.platform, 'machine', return_value='x86_64'), \
                 patch.object(installer.urllib.request, 'urlopen', return_value=io.BytesIO(blob)):
                with self.assertRaisesRegex(RuntimeError, 'SHA-256 mismatch'):
                    installer.install('test', directory)
            self.assertFalse((directory/'tool').exists())

    def test_web_fixture_rejects_missing_traffic_and_cleans_up(self):
        import io
        from web_fixture import WebFixture
        traffic = {'fetches': 3, 'slow': 1, 'aborted': 1, 'websockets': 4, 'echoed': 6657}
        fixture = WebFixture(None)
        fixture.url = 'http://127.0.0.1:1'
        with patch('web_fixture.urllib.request.urlopen', return_value=io.BytesIO(json.dumps(traffic).encode())):
            fixture.verify(1)
        for field in traffic:
            bad = {**traffic, field: 0}
            with self.subTest(field=field), patch('web_fixture.urllib.request.urlopen', return_value=io.BytesIO(json.dumps(bad).encode())):
                with self.assertRaisesRegex(RuntimeError, 'subject did not run'):
                    fixture.verify(1)
        # A real fixture with no clients must fail and still be reaped.
        import shutil
        if not shutil.which('node'):
            self.skipTest('real fixture cleanup requires Node; mocked traffic checks ran')
        with self.assertRaisesRegex(RuntimeError, 'subject did not run'):
            with WebFixture(ROOT / 'crates/turnloop-contract/tests/web/fixture.mjs') as live:
                process = live.process
                live.verify(1)
        self.assertIsNotNone(process.poll())

    def test_queue_is_never_silently_weakened(self):
        lint = module('lint-workflows')
        source = (ROOT/'.github/workflows/ci.yml').read_text()
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder)/'ci.yml'
            path.write_text(source)
            lint.check_queue(path)
            path.write_text(source.replace("'single' || 'max'", "'single' || 'single'"))
            with self.assertRaises(RuntimeError):
                lint.check_queue(path)

    def test_checked_tests_delivers_stdin_and_requires_execution(self):
        import sys
        runner = module('run-tests')
        command = [sys.executable, '-c',
                   'import sys; assert sys.stdin.read() == "fixture\\n"; print("test result: ok. 1 passed;")']
        self.assertEqual(runner.checked_tests(command, cwd=ROOT, input_text='fixture\n'), 1)
        with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
            runner.checked_tests([sys.executable, '-c', 'import sys; sys.stdin.read(); print("test result: ok. 0 passed;")'],
                                 cwd=ROOT, input_text='fixture\n')

    def test_wasi_runner_preserves_protocol_and_release_allocation_gates(self):
        import sys
        runner = module('run-tests')
        packages = [package(name) for name in ('core', 'contract', 'http', 'decoder')]
        for p, kind in zip(packages, ('core', 'contract', 'protocol', 'codec')):
            p['manifest_path'] = str(ROOT / 'crates' / p['name'] / 'Cargo.toml')
            p['metadata'] = {'turnloop-ci': {'role': kind, 'wasi-tests': ['subject']}}
            p['targets'] = [{'name': name, 'kind': ['test']} for name in ('subject', 'allocations')]
        packages[0]['metadata']['turnloop-ci'] = {'role': 'core', 'wasi-lib-tests': True}
        packages[1]['metadata']['turnloop-ci']['wasi-allocation-tests'] = ['allocations']
        data = dict(packages=packages, workspace_members=[p['id'] for p in packages], workspace_root=str(ROOT))
        for suite in ('wasi', 'protocol-wasi'):
            commands = []
            original = runner.checked_tests
            def execute(command, **kwargs):
                commands.append((command, kwargs))
                return original([sys.executable, '-c', 'import sys; sys.stdin.read(); print("test result: ok. 1 passed;")'], **kwargs)
            with patch.object(sys, 'argv', ['run-tests.py', suite, '--target', 'wasm32-wasip3']), \
                 patch.object(runner, 'metadata', return_value=data), \
                 patch.object(runner, 'checked_tests', side_effect=execute):
                runner.main()
            names = [c[c.index('-p') + 1] for c, _ in commands]
            if suite == 'protocol-wasi':
                self.assertEqual(names, ['http', 'decoder'])
            else:
                self.assertEqual(names, ['core', 'contract', 'contract', 'contract'])
                self.assertTrue(all('--all-features' in c for c, _ in commands))
                self.assertEqual(['--release' in c for c, _ in commands], [True, False, True, True])
                self.assertIn('allocations', commands[-1][0])
                self.assertTrue(all(k['input_text'] == 'turnloop revision two stdin fixture\n' for _, k in commands[1:]))


if __name__ == '__main__':
    unittest.main()
