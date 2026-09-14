"""No wasm compiler archive is extracted before its committed hash is checked."""
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from test_gates import module

installer = module('install-wasm-toolchain')


class WasmToolchain(unittest.TestCase):
    def test_hash_mismatch_cannot_extract(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / 'sdk.tar.gz'
            archive.write_bytes(b'not even a tar file')
            with self.assertRaisesRegex(RuntimeError, 'SHA-256 mismatch'):
                installer.extract(archive, root / 'out', '0' * 64)
            self.assertFalse((root / 'out').exists())

    def test_verified_archive_preserves_tree_and_rejects_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('sdk/bin/clang', '../escaped'):
                archive = root / 'sdk.tar.gz'
                with tarfile.open(archive, 'w:gz') as bundle:
                    entry = tarfile.TarInfo(name)
                    entry.size = 5
                    bundle.addfile(entry, io.BytesIO(b'probe'))
                digest = hashlib.sha256(archive.read_bytes()).hexdigest()
                if name.startswith('..'):
                    with self.assertRaises(tarfile.FilterError):
                        installer.extract(archive, root / 'out', digest)
                    self.assertFalse((root / 'escaped').exists())
                else:
                    installer.extract(archive, root / 'out', digest)
                    self.assertEqual((root / 'out' / name).read_bytes(), b'probe')

    def test_all_targets_use_sdk_tools_with_absolute_paths(self):
        env = installer.environment(Path('/tools with spaces/sdk'))
        self.assertEqual(len(env), 6)
        for target in ('wasm32_wasip2', 'wasm32_wasip3', 'wasm32_unknown_unknown'):
            self.assertEqual(env['CC_' + target], '/tools with spaces/sdk/bin/clang')
            self.assertEqual(env['AR_' + target], '/tools with spaces/sdk/bin/llvm-ar')


if __name__ == '__main__':
    unittest.main()
