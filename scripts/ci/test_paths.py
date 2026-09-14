"""The guard rejects case mistakes even on a case-insensitive filesystem."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from test_gates import module

checker = module('check-paths')


class Paths(unittest.TestCase):
    def check(self, files, tracked=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, contents in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(contents)
            check = checker.Check(root, tracked if tracked is not None else files)
            errors = check.run()
            return errors, check.references

    def test_real_git_index_case_mismatch_cli(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', directory], check=True)
            (root / 'Readme.md').write_text('fixture')
            (root / 'lib.rs').write_text('const DOC: &str = include_str!("README.md");')
            subprocess.run(['git', '-C', directory, 'add', 'Readme.md', 'lib.rs'], check=True)
            command = [sys.executable, checker.__file__, '--root', directory]
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn('Git tracks Readme.md', result.stderr)
            (root / 'lib.rs').write_text('const DOC: &str = include_str!("Readme.md");')
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('1 Rust/Cargo references', result.stdout)

    def test_file_and_directory_collisions(self):
        errors, _ = self.check({}, ['Foo.txt', 'foo.txt', 'Src/a.txt', 'src/b.txt'])
        self.assertEqual(len(errors), 2)
        self.assertTrue(all('case collision' in e for e in errors))

    def test_all_reference_types_fail_in_disabled_cfg_too(self):
        cases = [
            ('include_str!("../README.md")', 'pkg/Readme.md', 'text'),
            ('include_bytes!(r#"../ASSET.bin"#)', 'pkg/Asset.bin', 'bytes'),
            ('#[cfg(any())] #[path = "Other.rs"] mod other;', 'pkg/src/other.rs', ''),
            ('mod child;', 'pkg/src/Child.rs', ''),
            ('mod child;', 'pkg/src/Child/mod.rs', ''),
            ('mod nested { mod child; }', 'pkg/src/nested/Child.rs', ''),
        ]
        for source, target, body in cases:
            with self.subTest(source=source):
                errors, count = self.check({'pkg/Cargo.toml': '[package]\nname="p"',
                                            'pkg/src/lib.rs': source, target: body})
                self.assertEqual(count, 1)
                self.assertEqual(len(errors), 1)
                self.assertIn('Git tracks', errors[0])
        for key in ('readme', 'license-file'):
            errors, count = self.check({'Cargo.toml': f'[workspace.package]\n{key}="README.md"',
                                       'Readme.md': ''})
            self.assertEqual(count, 1)
            self.assertIn(key, errors[0])

    def test_modules_attributes_inline_roots_raw_and_concat(self):
        files = {
            'Cargo.toml': '[package]\nname="p"\nreadme="README.md"\nlicense-file="LICENSE"\n'
                          '[[test]]\nname="custom"\npath="custom/entry.rs"',
            'README.md': '', 'LICENSE': '', 'custom/entry.rs': 'mod helper;',
            'custom/helper.rs': '',
            'src/lib.rs': '''
                mod outer;
                #[path = "chosen.rs"] pub(crate) mod renamed;
                #[cfg_attr(unix, path = "one.rs")]
                #[cfg_attr(not(unix), path = "two.rs")] mod platform;
                #[path = "custom"] mod inline { #[path = "child.rs"] mod child; }
                const S: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"));
                const B: &[u8] = include_bytes! {r##"../LICENSE"##};
                const U: &str = include_str!("../R\\u{45}ADME.md");
                const X: &str = include_str!("../R\\x45ADME.md");
            ''',
            'src/outer.rs': 'mod child; mod inline { #[path="different.rs"] mod local; }',
            'src/outer/child.rs': '', 'src/outer/inline/different.rs': '',
            'src/chosen.rs': 'mod child;', 'src/child.rs': '', 'src/one.rs': '', 'src/two.rs': '', 'src/custom/child.rs': ''}
        errors, count = self.check(files)
        self.assertEqual(errors, [])
        self.assertEqual(count, 16)

    def test_comments_strings_and_characters_are_not_code(self):
        errors, count = self.check({'source.rs': '''
            // include_str!("MISSING")
            /* /* nested */ mod nonexistent; */
            const S: &str = r###"mod missing; include_bytes!("MISSING")"###;
            const B: &[u8] = b"mod missing;";
            const C: char = '}'; const Q: char = '\\'';
            fn borrow<'a>(x: &'a str) -> &'a str { x }
        '''})
        self.assertEqual(errors, [])
        self.assertEqual(count, 0)

    def test_conditional_path_keeps_default_module_reference(self):
        errors, count = self.check({'Cargo.toml': '[package]\nname="p"',
            'src/lib.rs': '#[cfg_attr(unix, path="unix.rs")] mod portable;',
            'src/unix.rs': '', 'src/Portable.rs': ''})
        self.assertEqual(count, 2)
        self.assertEqual(len(errors), 1)
        self.assertIn('Git tracks src/Portable.rs', errors[0])

    def test_parent_normalization_does_not_hide_wrong_directory_case(self):
        errors, count = self.check({'source.rs': 'include_str!("Data/../README.md");',
                                   'data/fixture.txt': '', 'README.md': ''})
        self.assertEqual(count, 1)
        self.assertEqual(len(errors), 1)

    def test_untracked_file_and_unresolved_expression_fail_closed(self):
        errors, count = self.check({'source.rs': 'include_bytes!("data.bin");', 'data.bin': ''},
                                   ['source.rs'])
        self.assertEqual(count, 1)
        self.assertIn('exact case is not tracked', errors[0])
        errors, _ = self.check({'source.rs': 'include_str!(env!("OUT_DIR"));'})
        self.assertIn('cannot statically resolve', errors[0])


if __name__ == '__main__':
    unittest.main()
