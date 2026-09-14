"""Private fixture lifecycle regressions; HTTP uses Node, database failures use fakes."""
import importlib.util
from contextlib import contextmanager, ExitStack, redirect_stderr
import io
import json
import os
from pathlib import Path
import subprocess
import socket
import sys
import tempfile
import unittest
from unittest.mock import patch, Mock

ROOT = Path(__file__).resolve().parents[2]


def runner():
    spec = importlib.util.spec_from_file_location('test_servers_runner', ROOT / 'scripts/test-servers.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@contextmanager
def private_runner():
    fixtures = runner()
    with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
        tools = Path(directory).resolve() / '.tools'
        for name, path in {
            'TOOLS': tools, 'STATE': tools / 'test-servers.json',
            'REDIS_DATA': tools / 'redis', 'REDIS_STATE': tools / 'redis/instances.json',
            'SQL_TOOLS': tools / 'sql', 'SQL_STATE': tools / 'sql/sql-servers.json',
            'MONGO_RUN': tools / 'mongodb', 'MONGO_MANIFEST': tools / 'mongodb/servers.json',
        }.items():
            stack.enter_context(patch.object(fixtures, name, path))
        tools.mkdir()
        yield fixtures


def crashing_binary(path):
    path.write_text('#!/usr/bin/env python3\nimport sys\n'
                    'print("old output outside the tail", flush=True)\n'
                    'for i in range(80): print("startup line", i, flush=True)\n'
                    'print("stdout: fixture actually executed", flush=True)\n'
                    'print("stderr: rejected fixture option", file=sys.stderr, flush=True)\n'
                    'sys.exit(23)\n')
    path.chmod(0o755)
    return str(path)


class Fixtures(unittest.TestCase):
    def test_default_run_starts_http_and_preserves_the_test_exit_code(self):
        with private_runner() as fixtures:
            arguments = ['test-servers.py', 'run', sys.executable, '-c', 'raise SystemExit(19)']
            env = {'TURNLOOP_TEST_HTTP_PORT': '32123', 'TURNLOOP_TEST_HTTP2_PORT': '32124'}
            with patch.object(sys, 'argv', arguments), \
                 patch.object(fixtures, 'sql_start', return_value={}) as sql, \
                 patch.object(fixtures, 'redis_start', return_value={}) as redis, \
                 patch.object(fixtures, 'mongo_start', return_value={}) as mongo, \
                 patch.object(fixtures, 'smtp_start', return_value={}) as smtp, \
                 patch.object(fixtures, 'SMTP_CHILD', Mock(pid=123)), \
                 patch.object(fixtures, 'http_start', return_value=env) as http, \
                 patch.object(fixtures, 'stop_all') as stop:
                self.assertEqual(fixtures.main(), 19)
            for start in (sql, redis, mongo, smtp, http):
                start.assert_called_once()
            stop.assert_called_once()
            self.assertEqual(json.loads(fixtures.STATE.read_text())['env']['TURNLOOP_TEST_HTTP2_PORT'], '32124')

    def test_http_failed_start_reports_tail_and_reaps_child(self):
        with private_runner() as fixtures:
            fixtures.SELECTED = {'http'}
            executable = crashing_binary(fixtures.TOOLS / 'node')
            errors = io.StringIO()
            with patch.object(fixtures, 'find_binary', return_value=executable), \
                 redirect_stderr(errors), self.assertRaises((RuntimeError, json.JSONDecodeError)):
                fixtures.start_all()
            self.assertIn('HTTP startup failed', errors.getvalue())
            self.assertIn('stdout: fixture actually executed', errors.getvalue())
            self.assertIn('stderr: rejected fixture option', errors.getvalue())
            self.assertNotIn('old output outside the tail', errors.getvalue())
            self.assertIn(str(fixtures.TOOLS / 'http/server.log'), errors.getvalue())
            self.assertIsNotNone(fixtures.HTTP_CHILD.returncode)
            self.assertFalse(fixtures.STATE.exists())
            self.assertFalse((fixtures.TOOLS / 'http/state.json').exists())

    def test_http_start_error_remains_primary_when_reaping_fails(self):
        with private_runner() as fixtures:
            child = Mock(returncode=23)
            child.poll.return_value = 23
            cleanup_error = subprocess.TimeoutExpired('node', 10)
            child.wait.side_effect = cleanup_error
            with patch.object(fixtures, 'private_process', return_value=child) as spawn, \
                 redirect_stderr(io.StringIO()), \
                 self.assertRaisesRegex(RuntimeError, 'HTTP fixture exited with status 23') as caught:
                fixtures.http_start()
            spawn.assert_called_once()
            child.wait.assert_called_once_with(timeout=10)
            child.terminate.assert_not_called()
            self.assertIs(caught.exception.__cause__, cleanup_error)
            self.assertFalse((fixtures.TOOLS / 'http/state.json').exists())

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

    def test_http_lifecycle_authenticates_and_closes_both_listeners(self):
        import socket
        import urllib.error
        import urllib.request
        fixtures = runner()
        with tempfile.TemporaryDirectory() as folder, patch.object(fixtures, 'TOOLS', Path(folder)):
            env = fixtures.http_start()
            ports = [int(env[key]) for key in ('TURNLOOP_TEST_HTTP_PORT', 'TURNLOOP_TEST_HTTP2_PORT')]
            try:
                self.assertNotEqual(*ports)
                for port in ports:
                    with socket.create_connection(('127.0.0.1', port), timeout=2):
                        pass
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                request = urllib.request.Request(f'http://127.0.0.1:{ports[0]}/__turnloop_shutdown',
                    method='POST', headers={'x-turnloop-test-token': 'wrong'})
                with self.assertRaises(urllib.error.HTTPError) as error:
                    opener.open(request, timeout=2)
                self.assertEqual(error.exception.code, 403)
                error.exception.close()
                with opener.open(f'http://127.0.0.1:{ports[0]}/alive', timeout=2) as response:
                    self.assertEqual(response.read(), b'GET /alive ')
            finally:
                fixtures.http_stop()
            self.assertFalse((Path(folder) / 'http/state.json').exists())
            self.assertEqual(fixtures.HTTP_CHILD.returncode, 0)
            for port in ports:
                with self.assertRaises(OSError):
                    socket.create_connection(('127.0.0.1', port), timeout=.2)

    def test_redis_crashed_start_reports_tail_and_cleans_all_state(self):
        with private_runner() as fixtures:
            fixtures.SELECTED = {'redis'}
            fixtures.REDIS_SERVER = crashing_binary(fixtures.TOOLS / 'redis-server')
            fixtures.REDIS_CLI = str(fixtures.TOOLS / 'must-not-run-cli')
            errors = io.StringIO()
            with redirect_stderr(errors), self.assertRaisesRegex(RuntimeError, 'Redis single exited with status 23') as caught:
                fixtures.start_all()
            self.assertIsNone(caught.exception.__cause__)
            self.assertIn('stdout: fixture actually executed', errors.getvalue())
            self.assertIn('stderr: rejected fixture option', errors.getvalue())
            self.assertNotIn('old output outside the tail', errors.getvalue())
            self.assertIn(str(fixtures.REDIS_DATA / 'single/server.log'), errors.getvalue())
            self.assertFalse(fixtures.REDIS_STATE.exists())
            self.assertFalse(fixtures.STATE.exists())
            self.assertEqual(fixtures.REDIS_CHILDREN, {})
            fixtures.redis_stop()  # Idempotent after a crashed start.

    def test_missing_redis_binary_reports_spawn_failure(self):
        with private_runner() as fixtures:
            fixtures.REDIS_SERVER = str(fixtures.TOOLS / 'absent-redis-server')
            errors = io.StringIO()
            with redirect_stderr(errors), self.assertRaises(FileNotFoundError):
                fixtures.redis_start()
            self.assertIn('Redis single startup failed', errors.getvalue())
            self.assertTrue((fixtures.REDIS_DATA / 'single/server.log').exists())
            self.assertFalse(fixtures.REDIS_STATE.exists())

    def test_crashed_start_remains_primary_when_stop_also_fails(self):
        with private_runner() as fixtures:
            fixtures.REDIS_SERVER = crashing_binary(fixtures.TOOLS / 'redis-server')
            cleanup_error = RuntimeError('injected cleanup failure')
            try:
                with patch.object(fixtures, 'redis_stop', side_effect=cleanup_error) as stop, \
                     redirect_stderr(io.StringIO()), \
                     self.assertRaisesRegex(RuntimeError, 'Redis single exited with status 23') as caught:
                    fixtures.redis_start()
                stop.assert_called_once()
                self.assertIs(caught.exception.__cause__, cleanup_error)
                self.assertTrue(fixtures.REDIS_STATE.exists())
            finally:
                fixtures.redis_stop()
            self.assertFalse(fixtures.REDIS_STATE.exists())

    def test_stop_after_detached_crash_or_refused_connection_cleans_state(self):
        for missing_process in (False, True):
            with self.subTest(missing_process=missing_process), private_runner() as fixtures:
                fixtures.REDIS_DATA.mkdir()
                child = fixtures.private_process([sys.executable, '-c', 'print("crashed start", flush=True); raise SystemExit(23)'], fixtures.REDIS_DATA / 'crash.log')
                self.assertEqual(child.wait(timeout=5), 23)
                # Use our live PID for the refused-connection case: no signal may
                # be sent to a stored PID, including one reused by another process.
                record = {'pid': child.pid if missing_process else os.getpid(),
                          'port': fixtures.sql_port(), 'config': str(fixtures.REDIS_DATA / 'single/redis.conf')}
                fixtures.REDIS_STATE.write_text(json.dumps([record]))
                (fixtures.REDIS_DATA / 'env.json').write_text('{}')
                with patch.object(fixtures.subprocess, 'run') as command:
                    fixtures.redis_stop()
                command.assert_not_called()
                self.assertFalse(fixtures.REDIS_STATE.exists())
                self.assertFalse((fixtures.REDIS_DATA / 'env.json').exists())

    def test_owned_child_that_never_listens_is_terminated_and_reaped(self):
        with private_runner() as fixtures:
            fixtures.REDIS_DATA.mkdir()
            child = fixtures.private_process([sys.executable, '-c', 'import time; time.sleep(60)'], fixtures.REDIS_DATA / 'hang.log')
            fixtures.REDIS_CHILDREN[child.pid] = child
            fixtures.REDIS_STATE.write_text(json.dumps([{'pid': child.pid, 'port': fixtures.sql_port(), 'config': 'unused'}]))
            try:
                fixtures.redis_stop()
                self.assertIsNotNone(child.returncode)
                self.assertFalse(fixtures.REDIS_STATE.exists())
            finally:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)

    def test_stop_retains_only_live_children_that_will_not_stop(self):
        with private_runner() as fixtures:
            fixtures.REDIS_DATA.mkdir()
            live = {'pid': 111, 'port': 32123, 'config': 'live'}
            dead = {'pid': 222, 'port': 32124, 'config': 'dead'}
            child = Mock()
            child.poll.return_value = None
            child.wait.side_effect = subprocess.TimeoutExpired('redis-server', 10)
            crashed = Mock()
            crashed.poll.return_value = 23
            fixtures.REDIS_CHILDREN.update({111: child, 222: crashed})
            fixtures.REDIS_STATE.write_text(json.dumps([live, dead]))
            with self.assertRaises(ExceptionGroup) as caught:
                fixtures.redis_stop()
            self.assertIsInstance(caught.exception.exceptions[0], subprocess.TimeoutExpired)
            self.assertEqual(json.loads(fixtures.REDIS_STATE.read_text()), [live])
            child.terminate.assert_called_once()
            crashed.wait.assert_called_once()
            crashed.terminate.assert_not_called()

    def test_detached_stop_refuses_a_different_config(self):
        with private_runner() as fixtures, socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            fixtures.REDIS_DATA.mkdir()
            record = {'pid': os.getpid(), 'port': listener.getsockname()[1], 'config': 'our-config'}
            fixtures.REDIS_STATE.write_text(json.dumps([record]))
            with patch.object(fixtures.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'config_file:someone-else\n', '')) as command:
                with self.assertRaises(ExceptionGroup) as caught:
                    fixtures.redis_stop()
            self.assertIn('does not identify our private config', str(caught.exception.exceptions[0]))
            self.assertEqual(command.call_count, 1)
            self.assertEqual(json.loads(fixtures.REDIS_STATE.read_text()), [record])

    def test_readiness_timeout_reports_the_named_server_log(self):
        with private_runner() as fixtures:
            path = fixtures.TOOLS / 'timeout.log'
            child = fixtures.private_process([sys.executable, '-c', 'import time; print("still initializing", flush=True); time.sleep(60)'], path)
            errors = io.StringIO()
            try:
                with redirect_stderr(errors), self.assertRaisesRegex(RuntimeError, 'SMTP did not start'):
                    with fixtures.startup_logs('SMTP', path):
                        fixtures.wait_port('SMTP', fixtures.sql_port(), child, timeout=.3)
                self.assertIn('still initializing', errors.getvalue())
                self.assertIn(str(path), errors.getvalue())
            finally:
                child.terminate()
                child.wait(timeout=5)

    def test_shutdown_connection_error_is_success_when_private_port_closes(self):
        with private_runner() as fixtures, socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            fixtures.REDIS_DATA.mkdir()
            record = {'pid': os.getpid(), 'port': listener.getsockname()[1], 'config': 'private-config'}
            fixtures.REDIS_STATE.write_text(json.dumps([record]))

            def command(args, **_kwargs):
                if args[-2:] == ['INFO', 'server']:
                    return subprocess.CompletedProcess(args, 0, 'config_file:private-config\n', '')
                self.assertEqual(args[-2:], ['SHUTDOWN', 'NOSAVE'])
                listener.close()
                return subprocess.CompletedProcess(args, 1, '', 'Connection refused')

            with patch.object(fixtures.subprocess, 'run', side_effect=command) as commands:
                fixtures.redis_stop()
            self.assertEqual(commands.call_count, 2)
            self.assertFalse(fixtures.REDIS_STATE.exists())

    def test_failed_test_command_remains_primary_when_cleanup_fails(self):
        with private_runner() as fixtures:
            arguments = ['test-servers.py', '--services', 'redis', 'run', sys.executable, '-c', 'raise SystemExit(17)']
            cleanup_error = RuntimeError('injected final cleanup failure')
            with patch.object(sys, 'argv', arguments), patch.object(fixtures, 'start_all', return_value={}), \
                 patch.object(fixtures, 'stop_all', side_effect=cleanup_error) as stop, \
                 self.assertRaises(subprocess.CalledProcessError) as caught:
                fixtures.main()
            self.assertEqual(caught.exception.returncode, 17)
            self.assertIs(caught.exception.__cause__, cleanup_error)
            stop.assert_called_once()

    def test_private_sql_initializers_report_both_output_streams(self):
        for service, binary, label in [('postgres', 'initdb', 'PostgreSQL initdb'), ('mysql', 'mysqld', 'MySQL initialization')]:
            with self.subTest(service=service), private_runner() as fixtures:
                fixtures.SQL_TOOLS.mkdir()
                fixtures.SELECTED = {service}
                executable = crashing_binary(fixtures.TOOLS / binary)
                errors = io.StringIO()
                with patch.object(fixtures, 'SQL_BIN', fixtures.TOOLS), \
                     patch.object(fixtures, 'sql_certificates', return_value=('cert', 'key')), \
                     redirect_stderr(errors), self.assertRaises(subprocess.CalledProcessError) as caught:
                    fixtures.sql_start()
                self.assertEqual(caught.exception.returncode, 23)
                self.assertIn(label + ' startup failed', errors.getvalue())
                self.assertIn('stdout: fixture actually executed', errors.getvalue())
                self.assertIn('stderr: rejected fixture option', errors.getvalue())
                self.assertFalse(fixtures.SQL_STATE.exists())

    def test_private_sql_servers_report_crashes_after_initialization(self):
        for service, binary in [('postgres', 'postgres'), ('mysql', 'mysqld')]:
            with self.subTest(service=service), private_runner() as fixtures:
                fixtures.SELECTED = {service}
                (fixtures.SQL_TOOLS / 'pgdata').mkdir(parents=True)
                (fixtures.SQL_TOOLS / 'pgdata/PG_VERSION').write_text('16')
                (fixtures.SQL_TOOLS / 'mysqldata/mysql').mkdir(parents=True)
                executable = crashing_binary(fixtures.TOOLS / binary)
                errors = io.StringIO()
                crashing_binary(fixtures.TOOLS / 'psql')
                crashing_binary(fixtures.TOOLS / 'mysql')
                # A failing readiness query gives the real child time to exit.
                with patch.object(fixtures, 'SQL_BIN', fixtures.TOOLS), \
                     patch.object(fixtures, 'sql_certificates', return_value=('cert', 'key')), \
                     redirect_stderr(errors), self.assertRaisesRegex(RuntimeError, 'server exited with status 23'):
                    fixtures.sql_start()
                self.assertIn('stdout: fixture actually executed', errors.getvalue())
                self.assertIn('stderr: rejected fixture option', errors.getvalue())
                self.assertFalse(fixtures.SQL_STATE.exists())

    def test_mongo_and_smtp_crashed_start_report_server_output(self):
        for service in ('mongodb', 'smtp'):
            with self.subTest(service=service), private_runner() as fixtures, patch.dict(os.environ):
                fixtures.SELECTED = {service}
                executable = crashing_binary(fixtures.TOOLS / ('mongod' if service == 'mongodb' else 'smtp-sink'))
                os.environ['PATH'] = str(fixtures.TOOLS) + os.pathsep + os.environ['PATH']
                # Exercise the actual Mongo process / SMTP supervisor, including
                # certificate generation and descriptor inheritance.
                errors = io.StringIO()
                with redirect_stderr(errors), self.assertRaisesRegex(RuntimeError, 'exited with status'):
                    fixtures.start_all()
                self.assertTrue(Path(executable).exists())
                self.assertIn('stdout: fixture actually executed', errors.getvalue())
                self.assertIn('stderr: rejected fixture option', errors.getvalue())
                self.assertFalse(fixtures.STATE.exists())
                self.assertFalse(fixtures.MONGO_MANIFEST.exists())
                self.assertFalse((fixtures.TOOLS / 'smtp/control.sock').exists())

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
            tools.mkdir()
            wrappers = tools / 'test-bin'
            wrappers.mkdir()
            for name in ('redis-cli', 'redis-server'):
                (wrappers / name).write_text('stale Redis wrapper')
            with patch.object(fixtures, 'TOOLS', tools):
                fixtures.prepare_docker_wrappers()
            self.assertFalse((wrappers / 'redis-cli').exists())
            self.assertFalse((wrappers / 'redis-server').exists())
            args = json.loads(subprocess.check_output(['mongod', '--port', '32123'], text=True))
            self.assertEqual(args[:2], ['run', '--rm'])
            self.assertEqual(args[args.index('--label') + 1], 'turnloop.fixture=' + str(tools))
            self.assertEqual(args[args.index('--volume') + 1], str(root) + ':' + str(root))
            self.assertEqual(args[-4:], ['mongod', 'mongo:8', '--port', '32123'])
            recorded = json.loads((tools / 'docker-fixtures.json').read_text())
            self.assertEqual(recorded, [args[args.index('--name') + 1]])
            self.assertTrue(recorded[0].startswith('turnloop-fixture-'))


if __name__ == '__main__':
    unittest.main()
