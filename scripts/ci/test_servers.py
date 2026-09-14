"""Lifecycle regressions for the shared fixture runner; no external servers needed."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def runner():
    spec = importlib.util.spec_from_file_location('test_servers_runner', ROOT / 'scripts/test-servers.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Fixtures(unittest.TestCase):
    def test_failed_first_initializer_cleans_partial_state(self):
        fixtures = runner()
        with tempfile.TemporaryDirectory() as directory:
            tools = Path(directory) / '.tools'
            state = tools / 'test-servers.json'
            with patch.object(fixtures, 'TOOLS', tools), patch.object(fixtures, 'STATE', state), \
                 patch.object(fixtures, 'SELECTED', {'postgres', 'smtp'}), \
                 patch.object(fixtures, 'sql_start', side_effect=RuntimeError('initializer failed')), \
                 patch.object(fixtures, 'sql_stop') as sql_stop, \
                 patch.object(fixtures, 'redis_stop') as redis_stop, \
                 patch.object(fixtures, 'mongo_stop') as mongo_stop, \
                 patch.object(fixtures, 'smtp_start') as smtp_start, \
                 patch.object(fixtures, 'docker_cleanup'):
                with self.assertRaisesRegex(RuntimeError, 'initializer failed'):
                    fixtures.start_all()
                self.assertFalse(state.exists())
                smtp_start.assert_not_called()
                sql_stop.assert_called_once()
                redis_stop.assert_called_once()
                mongo_stop.assert_called_once()

    def test_docker_wrapper_records_only_its_private_container(self):
        fixtures = runner()
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ):
            root = Path(directory).resolve()
            tools = root / '.tools'
            fake = root / 'fake-bin'
            fake.mkdir()
            docker = fake / 'docker'
            docker.write_text('#!/usr/bin/env python3\nimport json,sys\nprint(json.dumps(sys.argv[1:]))\n')
            docker.chmod(0o755)
            os.environ['PATH'] = str(fake) + os.pathsep + os.environ['PATH']
            with patch.object(fixtures, 'TOOLS', tools):
                fixtures.prepare_docker_wrappers()
            args = json.loads(subprocess.check_output(['redis-cli', '-p', '32123', 'PING'], text=True))
            self.assertEqual(args[:2], ['run', '--rm'])
            self.assertEqual(args[args.index('--label') + 1], 'turnloop.fixture=' + str(tools))
            self.assertEqual(args[args.index('--volume') + 1], str(root) + ':' + str(root))
            self.assertEqual(args[-5:], ['redis-cli', 'redis:8', '-p', '32123', 'PING'])
            recorded = json.loads((tools / 'docker-fixtures.json').read_text())
            self.assertEqual(recorded, [args[args.index('--name') + 1]])
            self.assertTrue(recorded[0].startswith('turnloop-fixture-'))


if __name__ == '__main__':
    unittest.main()
