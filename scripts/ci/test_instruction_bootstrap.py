"""Run the full bootstrap entrypoint against fresh synthetic Callgrind summaries.

These validate automation only; the fixtures are never repository baselines.
"""
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from test_gates import module


class Bootstrap(unittest.TestCase):
    def exercise(self, *, record=False, existing=False, mutation=None):
        bench = module('instructions')
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            relative = Path('crates/bench/benchmarks/instructions.json')
            baseline = root / relative
            baseline.parent.mkdir(parents=True)
            counts = {'bench::control': 100, 'bench::idle': 200}
            committed = dict(toolchain=bench.PIN, valgrind='valgrind-3.test',
                             counts=counts, controls=['bench::control'])
            if existing:
                baseline.write_text(json.dumps(committed))
            original = baseline.read_bytes() if existing else None
            package = dict(name='bench', manifest_path=str(root / 'crates/bench/Cargo.toml'),
                           id='bench', dependencies=[dict(name='gungraun', req='=0.19.4')],
                           targets=[dict(kind=['bench'], name='instructions')],
                           metadata={'turnloop-ci': dict(role='bench', **{
                               'instruction-baseline': 'benchmarks/instructions.json',
                               'instruction-cases': list(counts), 'instruction-controls': ['bench::control']})})
            data = dict(workspace_root=str(root), packages=[package], workspace_members=['bench'])
            runs = []
            def run(command, **kwargs):
                if command == ['valgrind', '--version']:
                    return 'valgrind-3.test\n'
                self.assertIn('--bench', command)
                self.assertIn('--locked', command)
                directory = Path(kwargs['env']['GUNGRAUN_HOME'])
                self.assertFalse(list(directory.iterdir()), 'each round starts empty')
                runs.append(str(directory))
                measured = counts.copy()
                if mutation:
                    mutation(measured, len(runs))
                for index, (key, value) in enumerate(measured.items()):
                    target = directory / str(index)
                    target.mkdir()
                    summary = {'version': '6', 'module_path': key, 'id': None, 'profiles': [
                        {'tool': 'Callgrind', 'summaries': {'total': {'summary': {
                            'Callgrind': {'Ir': {'metrics': {'Left': {'Int': value}}}}}}}}]}
                    (target / 'summary.json').write_text(json.dumps(summary))
            artifacts = root / 'artifacts'
            args = ['instructions.py', '--artifact-dir', str(artifacts)]
            if record:
                args.append('--record-baselines')
            output = io.StringIO()
            error = None
            with patch.object(bench, 'metadata', return_value=data), patch.object(bench, 'run', side_effect=run), \
                 patch.object(bench.platform, 'system', return_value='Linux'), \
                 patch.object(bench.platform, 'machine', return_value='x86_64'), \
                 patch('sys.argv', args), redirect_stdout(output):
                try:
                    bench.main()
                except RuntimeError as failure:
                    error = str(failure)
            self.assertEqual(baseline.read_bytes() if baseline.exists() else None, original,
                             'recording never changes committed baselines')
            files = {p.relative_to(artifacts).as_posix(): p.read_text() for p in artifacts.rglob('*') if p.is_file()}
            return error, runs, files, output.getvalue(), relative.as_posix()

    def test_missing_baseline_measures_uploadable_artifact_then_fails(self):
        error, runs, files, output, relative = self.exercise()
        self.assertIn('Missing instruction baselines', error)
        self.assertEqual(len(set(runs)), 3)
        candidate = json.loads(files[relative])
        self.assertEqual(candidate['counts'], {'bench::control': 100, 'bench::idle': 200})
        self.assertEqual(len(json.loads(files['measurements.json'])['bench']['rounds']), 3)
        for text in ('instruction-baselines', f'cp .tools/instruction-baselines/{relative} {relative}',
                     'git add ' + relative, 'git commit'):
            self.assertIn(text, output)
            self.assertIn(text, files['README.txt'])

    def test_dispatch_records_with_and_without_existing_baseline(self):
        for existing in (False, True):
            with self.subTest(existing=existing):
                error, runs, files, _, relative = self.exercise(record=True, existing=existing)
                self.assertIsNone(error)
                self.assertEqual(len(set(runs)), 3)
                self.assertIn(relative, files)

    def test_normal_gate_requires_measured_positive_counts(self):
        error, runs, files, _, _ = self.exercise(existing=True)
        self.assertIsNone(error)
        self.assertEqual(len(set(runs)), 3)
        self.assertEqual(files, {})
        for mutate in (lambda c, r: c.update({'bench::idle': 0}),
                       lambda c, r: c.pop('bench::idle'),
                       lambda c, r: c.update({'bench::control': 100 + r}),
                       lambda c, r: c.update({'bench::idle': 207 if r == 3 else 200})):
            for record in (False, True):
                with self.subTest(mutate=mutate, record=record):
                    error, _, files, _, relative = self.exercise(record=record, existing=True, mutation=mutate)
                    self.assertIsNotNone(error)
                    self.assertNotIn(relative, files)

    def test_workflow_uploads_on_failure_and_dispatch_is_explicit(self):
        from common import ROOT
        source = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn('workflow_dispatch:\n    inputs:\n      record_baselines:', source)
        self.assertIn("github.event_name == 'workflow_dispatch' && inputs.record_baselines", source)
        upload = source[source.index('      - name: Upload measured baseline'):source.index('  ci-gate:')]
        self.assertIn('!cancelled()', upload)
        self.assertIn('name: instruction-baselines', upload)
        self.assertIn('if-no-files-found: error', upload)

    def test_incomplete_rounds_cannot_pass(self):
        bench = module('instructions')
        baseline = {'counts': {'control': 100, 'idle': 200}, 'controls': ['control']}
        for rounds in ([], [baseline['counts']], [baseline['counts']] * 2):
            with self.subTest(rounds=len(rounds)), self.assertRaisesRegex(RuntimeError, 'three fresh'):
                bench.compare(baseline, rounds)


if __name__ == '__main__':
    unittest.main()
