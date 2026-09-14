"""Negative controls for explicit feature coverage and required native mode arms."""
import json
import tomllib
import unittest
from common import ROOT
from feature_modes import check, matrix_rows, native_modes


class FeatureModes(unittest.TestCase):
    def setUp(self):
        self.source = (ROOT / '.github/workflows/ci.yml').read_text()
        self.manifest = tomllib.loads((ROOT / 'crates/turnloop/Cargo.toml').read_text())

    def replace_row(self, source, old, new=None):
        line = '          - ' + json.dumps(old) + '\n'
        self.assertIn(line, source)
        return source.replace(line, '' if new is None else '          - ' + json.dumps(new) + '\n', 1)

    def test_actual_matrix_covers_every_feature_and_both_linux_architectures(self):
        rows = check(self.source, self.manifest)
        self.assertEqual(len(rows), 18)
        self.assertEqual(len(native_modes('linux')), 6)
        self.assertEqual(len(native_modes('darwin')), 3)
        self.assertEqual(len(native_modes('win32')), 3)

    def test_all_features_does_not_cover_an_unlisted_new_feature(self):
        self.manifest['features']['untested-mode'] = []
        with self.assertRaisesRegex(RuntimeError, 'without explicit runtime CI arms'):
            check(self.source, self.manifest)
        # Explicit runtime rows, wired to the runner, are the way to add coverage.
        row = {'os': 'ubuntu-24.04', 'mode': 'new-mode', 'features': 'turnloop/untested-mode'}
        first = matrix_rows(self.source)[0]
        line = '          - ' + json.dumps(first) + '\n'
        changed = self.source.replace(line, line + '          - ' + json.dumps(row) + '\n')
        self.assertEqual(len(check(changed, self.manifest)), 19)

    def test_implicit_optional_dependency_features_need_runtime_arms(self):
        for table in (self.manifest, self.manifest.setdefault('target', {}).setdefault('cfg(unix)', {})):
            table.setdefault('dependencies', {})['uncovered-dependency'] = {'version': '1', 'optional': True}
            with self.assertRaisesRegex(RuntimeError, 'without explicit runtime CI arms'):
                check(self.source, self.manifest)
            del table['dependencies']['uncovered-dependency']
        # futures-io is deliberately hidden behind the explicitly tested executor.
        self.assertEqual(len(check(self.source, self.manifest)), 18)

    def test_every_required_row_is_individually_mandatory(self):
        for row in matrix_rows(self.source):
            with self.subTest(row=row), self.assertRaisesRegex(RuntimeError, 'Missing or changed required mode'):
                check(self.replace_row(self.source, row), self.manifest)

    def test_mode_feature_drift_and_unknown_features_fail(self):
        rows = matrix_rows(self.source)
        for row in rows:
            changed = dict(row, features='turnloop/not-a-feature')
            with self.subTest(row=row), self.assertRaisesRegex(RuntimeError, 'Unknown turnloop features'):
                check(self.replace_row(self.source, row, changed), self.manifest)
        row = next(r for r in rows if r['mode'] == 'epoll-timerfd')
        with self.assertRaisesRegex(RuntimeError, 'Missing or changed required mode'):
            check(self.replace_row(self.source, row, dict(row, features='all')), self.manifest)

    def test_skip_exclude_duplicate_and_disconnected_matrix_fail(self):
        changes = [
            self.source.replace('  test-native:\n', '  test-native:\n    if: false\n'),
            self.source.replace('  test-native:\n', '  test-native:\n    continue-on-error: true\n'),
            self.source.replace('        include:\n', '        exclude:\n', 1),
            self.source.replace('run-tests.py native --mode "$NATIVE_MODE"', 'run-tests.py native'),
            self.source.replace('run-tests.py interop --mode "$NATIVE_MODE"', 'run-tests.py interop'),
            self.source.replace(', test-native,', ','),
            self.source.replace('python3 scripts/ci/feature_modes.py', 'true'),
        ]
        row = matrix_rows(self.source)[0]
        line = '          - ' + json.dumps(row) + '\n'
        changes.append(self.source.replace(line, line + line))
        for source in changes:
            with self.subTest(source=source[:100]), self.assertRaises(RuntimeError):
                check(source, self.manifest)

    def test_linux_modes_cannot_be_selected_on_other_platforms(self):
        for platform in ('darwin', 'win32'):
            for mode in ('epoll-timerfd', 'process-sigchld', 'fallbacks'):
                with self.subTest(platform=platform, mode=mode), self.assertRaisesRegex(RuntimeError, 'not applicable'):
                    native_modes(platform, mode)
        with self.assertRaisesRegex(RuntimeError, 'not applicable'):
            native_modes('linux', 'typo')
        with self.assertRaisesRegex(RuntimeError, 'No required native CI modes'):
            native_modes('unknown')

    def test_btree_cannot_unify_into_the_core(self):
        self.assertNotIn('timer-btree', self.manifest['features'])
        bench = tomllib.loads((ROOT / 'crates/turnloop-bench/Cargo.toml').read_text())
        self.assertEqual(bench['features']['timer-btree'], [])


if __name__ == '__main__':
    unittest.main()
