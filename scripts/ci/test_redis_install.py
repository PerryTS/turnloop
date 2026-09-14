"""Fail closed on corrupt downloads and incompatible Redis cache contents."""
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


def installer():
    path = Path(__file__).with_name('install-redis.py')
    spec = importlib.util.spec_from_file_location('redis_installer', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class RedisInstall(unittest.TestCase):
    def test_bad_digest_never_extracts_or_builds(self):
        build = installer()
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(build.urllib.request, 'urlopen', return_value=io.BytesIO(b'corrupt source')) as download, \
             patch.object(build.tarfile, 'open') as extract, \
             patch.object(build.subprocess, 'run') as make:
            destination = Path(tmp) / 'redis-build'
            with self.assertRaisesRegex(RuntimeError, 'Redis SHA-256 mismatch'):
                build.install(destination)
            download.assert_called_once()
            extract.assert_not_called()
            make.assert_not_called()
            self.assertFalse((destination / 'build.json').exists())

    def test_binary_checks_execute_and_reject_wrong_version_or_missing_tls(self):
        build = installer()
        cases = [('8.4.0', True, None), ('7.0.15', True, 'Wrong Redis fixture version'),
                 ('8.4.01', True, 'Wrong Redis fixture version'),
                 ('8.4.0', False, 'lacks TLS support')]
        for version, tls, error in cases:
            with self.subTest(version=version, tls=tls), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                server = directory / 'redis-server'
                server.write_text(f'#!/bin/sh\necho "Redis server v={version} malloc=libc"\n')
                server.chmod(0o755)
                cli = directory / 'redis-cli'
                help_text = '--tls --cacert' if tls else '--help'
                cli.write_text(f'#!/bin/sh\nif [ "$1" = "--version" ]; then\n'
                               f'echo "redis-cli {version} (git:abcdef01)"\nelse\n'
                               f'echo "{help_text}"\nfi\n')
                cli.chmod(0o755)
                if error:
                    with self.assertRaisesRegex(RuntimeError, error):
                        build.verify_binaries(directory)
                else:
                    build.verify_binaries(directory)


if __name__ == '__main__':
    unittest.main()
