"""Pinned ZIP tools extract only the named regular executable."""
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile
from test_gates import module


class ToolArchives(unittest.TestCase):
    def install_zip(self, members, *, digest=None):
        installer = module('install-tools')
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, 'w') as bundle:
            for name, content, symlink in members:
                item = zipfile.ZipInfo(name)
                item.external_attr = (0o120777 if symlink else 0o100755) << 16
                bundle.writestr(item, content)
        data = archive.getvalue()
        pin = {'curl': {'Windows-AMD64': {
            'url': 'https://example.invalid/pinned.zip', 'version': 'pinned',
            'sha256': digest or hashlib.sha256(data).hexdigest(),
            'executables': ['curl.exe'],
        }}}
        with tempfile.TemporaryDirectory() as folder:
            destination = Path(folder)
            with patch.object(installer.platform, 'system', return_value='Windows'), \
                 patch.object(installer.platform, 'machine', return_value='AMD64'), \
                 patch.object(Path, 'read_text', return_value=json.dumps(pin)), \
                 patch.object(installer.urllib.request, 'urlopen', return_value=io.BytesIO(data)):
                installer.install('curl', destination)
            self.assertEqual([p.name for p in destination.iterdir()], ['curl.exe'])
            return (destination / 'curl.exe').read_bytes()

    def test_extracts_only_the_requested_regular_file(self):
        self.assertEqual(self.install_zip([
            ('release/bin/curl.exe', b'fixture', False),
            ('../../outside.txt', b'do not extract', False),
        ]), b'fixture')

    def test_rejects_digest_mismatch(self):
        with self.assertRaisesRegex(RuntimeError, 'SHA-256 mismatch'):
            self.install_zip([('curl.exe', b'fixture', False)], digest='0' * 64)

    def test_rejects_symlink_and_duplicate_basenames(self):
        for members in [[('curl.exe', b'other.exe', True)],
                        [('a/curl.exe', b'one', False), ('b/curl.exe', b'two', False)]]:
            with self.subTest(members=members), self.assertRaisesRegex(RuntimeError, 'exactly one'):
                self.install_zip(members)


if __name__ == '__main__':
    unittest.main()
