"""Private data is removed after shutdown and never traversed by log upload."""
from contextlib import redirect_stderr
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch
from test_servers import private_runner
from common import ROOT


class Cleanup(unittest.TestCase):
    def test_docker_cleanup_precedes_reaping_and_data_removal(self):
        from unittest.mock import Mock
        with private_runner() as fixtures:
            db = fixtures.MONGO_RUN / 'rs0/run'
            db.mkdir(parents=True)
            (db / 'data').write_text('owned fixture data')
            fixtures.MONGO_MANIFEST.write_text(json.dumps({'servers': [
                {'name': 'rs0', 'port': 32123, 'dbpath': str(db)}]}))
            container = 'turnloop-fixture-owned'
            (fixtures.TOOLS / 'docker-fixtures.json').write_text(json.dumps([container]))
            commands = []
            def command(args, **kwargs):
                self.assertTrue(db.exists(), 'data must survive until Docker has stopped')
                commands.append(args)
                return subprocess.CompletedProcess(args, 0, str(fixtures.TOOLS) + '\n', '')
            child = Mock()
            def reaped(**kwargs):
                self.assertEqual(commands[-1], ['docker', 'rm', '--force', container])
                self.assertTrue(db.exists())
            child.wait.side_effect = reaped
            fixtures.MONGO_PROCESSES['rs0'] = child
            with patch.object(fixtures.subprocess, 'run', side_effect=command), \
                 patch.object(fixtures, 'port_open', return_value=False):
                fixtures.mongo_stop()
            self.assertEqual([args[1] for args in commands], ['inspect', 'logs', 'rm'])
            child.wait.assert_called_once_with(timeout=40)
            self.assertFalse(db.exists())
            self.assertFalse(fixtures.MONGO_MANIFEST.exists())
            self.assertFalse((fixtures.TOOLS / 'docker-fixtures.json').exists())

    def test_mongo_and_sql_reap_children_before_removing_restrictive_data(self):
        for binary, name in [('mongod', 'rs0/run'), ('postgres', 'pgdata'), ('mysqld', 'mysqldata')]:
            with self.subTest(binary=binary), private_runner() as fixtures:
                parent = fixtures.MONGO_RUN if binary == 'mongod' else fixtures.SQL_TOOLS
                db = parent / name
                db.mkdir(parents=True)
                log = parent / 'server.log'
                log.write_text('keep the server diagnostics')
                script = '''import pathlib, socket, sys, time
db = pathlib.Path(sys.argv[1])
tmp = db / '_tmp'
tmp.mkdir()
(tmp / 'data').write_bytes(b'server actually ran')
tmp.chmod(0)
listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen()
print(listener.getsockname()[1], flush=True)
time.sleep(60)
'''
                with subprocess.Popen([sys.executable, '-c', script, str(db)],
                                      stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True) as child:
                    try:
                        port = int(child.stdout.readline())
                        self.assertTrue(fixtures.port_open(port))
                        entry = {'name': 'rs0', 'port': port, 'pid': child.pid, 'dbpath': str(db), 'binary': binary}
                        if binary == 'mongod':
                            fixtures.MONGO_MANIFEST.write_text(json.dumps({'servers': [entry]}))
                            fixtures.MONGO_PROCESSES['rs0'] = child
                            stop = fixtures.mongo_stop
                        else:
                            fixtures.SQL_STATE.write_text(json.dumps({'servers': [entry]}))
                            fixtures.SQL_CHILDREN[child.pid] = child
                            stop = fixtures.sql_stop
                        remove = fixtures.remove_private_data
                        removals = []
                        def checked_remove(directory, root):
                            self.assertIsNotNone(child.returncode, 'child must be reaped before data removal')
                            self.assertFalse(fixtures.port_open(port))
                            removals.append(str(directory))
                            remove(directory, root)
                        with patch.object(fixtures, 'remove_private_data', side_effect=checked_remove):
                            stop()
                        self.assertIn(str(db), removals)
                        self.assertFalse(db.exists())
                        self.assertEqual(log.read_text(), 'keep the server diagnostics')
                        self.assertFalse(fixtures.SQL_STATE.exists())
                        self.assertFalse(fixtures.MONGO_MANIFEST.exists())
                    finally:
                        if child.poll() is None:
                            child.kill()
                        child.wait(timeout=5)
                        if db.exists():
                            fixtures.remove_private_data(db, parent)

    def test_live_port_or_failed_shutdown_keeps_mongo_data_and_manifest(self):
        import socket
        with private_runner() as fixtures, socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            db = fixtures.MONGO_RUN / 'rs0/run'
            db.mkdir(parents=True)
            entry = {'name': 'rs0', 'port': listener.getsockname()[1], 'dbpath': str(db)}
            fixtures.MONGO_MANIFEST.write_text(json.dumps({'servers': [entry]}))
            with patch.object(fixtures, 'mongo_env', return_value={}), patch.object(fixtures.subprocess, 'run') as stop:
                with self.assertRaisesRegex(RuntimeError, 'still listening'):
                    fixtures.mongo_stop()
            stop.assert_called_once()
            self.assertTrue(db.exists())
            self.assertTrue(fixtures.MONGO_MANIFEST.exists())

    def test_log_staging_does_not_walk_sql_mongo_or_redis_data(self):
        with private_runner() as fixtures:
            logs = ['sql/postgres.log', 'sql/mysql-init.log', 'mongodb/rs0/mongod.log',
                    'mongodb/rs0/process.log', 'redis/single/server.log', 'smtp/server.log', 'http/server.log']
            for name in logs:
                path = fixtures.TOOLS / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('old line\n' * 80 + f'actual {name}\n')
            restricted = []
            for name in ('sql/pgdata', 'sql/mysqldata', 'mongodb/rs0/run', 'redis/single/data'):
                path = fixtures.TOOLS / name
                path.mkdir()
                (path / 'must-not-upload.log').write_text('database data')
                path.chmod(0)
                restricted.append(path)
            try:
                output = io.StringIO()
                with redirect_stderr(output):
                    fixtures.collect_logs()
                collected = fixtures.TOOLS / 'protocol-logs'
                self.assertEqual(sorted(p.name for p in collected.iterdir()), sorted(n.replace('/', '-') for n in logs))
                for name in logs:
                    self.assertIn(f'actual {name}', output.getvalue())
                    self.assertEqual((collected / name.replace('/', '-')).read_text(), (fixtures.TOOLS / name).read_text())
            finally:
                for path in restricted:
                    path.chmod(0o700)

    def test_data_cleanup_rejects_outside_roots_and_preserves_symlink_targets(self):
        with private_runner() as fixtures:
            private = fixtures.TOOLS / 'data'
            private.mkdir()
            outside = fixtures.TOOLS.parent / 'untouched'
            outside.mkdir()
            (outside / 'evidence').write_text('keep')
            (private / 'link').symlink_to(outside, target_is_directory=True)
            for invalid in (outside, fixtures.TOOLS, private / 'link'):
                with self.assertRaisesRegex(RuntimeError, 'Invalid private data'):
                    fixtures.remove_private_data(invalid, fixtures.TOOLS)
            fixtures.remove_private_data(private, fixtures.TOOLS)
            self.assertEqual((outside / 'evidence').read_text(), 'keep')

    def test_workflow_artifacts_and_caches_have_only_dedicated_roots(self):
        import re
        source = (ROOT / '.github/workflows/ci.yml').read_text()
        paths = re.findall(r'^          path: (.+)$', source, re.MULTILINE)
        self.assertEqual(set(paths), {'.tools/redis-build', '.tools/protocol-logs/', '.tools/instruction-baselines/'})
        self.assertIn('image: postgres:16.13', source)
        step = source.split('- name: SQL service container logs on failure\n')[1].split('      - name:')[0]
        self.assertIn('if: failure()', step)
        for name in ('POSTGRES', 'MYSQL'):
            self.assertIn(f'docker logs "${name}_CONTAINER"', step)
            self.assertIn(f'{name}_CONTAINER: ${{{{ job.services.{name.lower()}.id }}}}', step)

    def test_both_container_logs_are_attempted_when_one_docker_logs_fails(self):
        import tempfile
        import textwrap
        source = (ROOT / '.github/workflows/ci.yml').read_text()
        step = source.split('- name: SQL service container logs on failure\n')[1].split('      - name:')[0]
        script = textwrap.dedent(step.split('        run: |\n')[1])
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            fake = directory / 'docker'
            fake.write_text('#!/bin/sh\n[ "$1" = logs ] || exit 9\n'
                            'echo "server log from $2"\n[ "$2" = mysql-fixture ]\n')
            fake.chmod(0o755)
            result = subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', script], cwd=directory,
                                    env={**os.environ, 'PATH': str(directory) + os.pathsep + os.environ['PATH'],
                                         'POSTGRES_CONTAINER': 'pg-fixture', 'MYSQL_CONTAINER': 'mysql-fixture'},
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            for name, container in [('postgres', 'pg-fixture'), ('mysql', 'mysql-fixture')]:
                self.assertIn('server log from ' + container, result.stdout)
                self.assertEqual((directory / f'.tools/protocol-logs/{name}-container.log').read_text(),
                                 f'server log from {container}\n')


if __name__ == '__main__':
    unittest.main()
