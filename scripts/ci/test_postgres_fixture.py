"""PostgreSQL CI configuration parity and real forwarding-proxy traffic."""
import importlib.util
import os
from pathlib import Path
import socket
import socketserver
import struct
import subprocess
import threading
import unittest
from unittest.mock import Mock, patch
from common import ROOT
from test_servers import private_runner


class PostgresFixture(unittest.TestCase):
    def test_proxy_shutdown_wakes_and_joins_idle_relays(self):
        spec = importlib.util.spec_from_file_location('tcp_proxy', ROOT / 'scripts/fixtures/tcp_proxy.py')
        proxy_module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(proxy_module)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            listener.settimeout(5)
            with proxy_module.TcpProxy(listener.getsockname()[1]) as proxy:
                client = socket.create_connection(('127.0.0.1', proxy.port), timeout=5)
                upstream, _ = listener.accept()
                upstream.settimeout(5)
            with client, upstream:
                self.assertEqual(client.recv(1), b'')
                self.assertEqual(upstream.recv(1), b'')
            self.assertEqual(proxy.connections, 1)
            self.assertEqual(proxy._sockets, set())
            self.assertFalse(proxy._thread.is_alive())

    def test_local_and_ci_share_bootstrap_user_settings_and_reload(self):
        transcripts = []
        for ci in (False, True):
            with self.subTest(ci=ci), private_runner() as fixtures:
                fixtures.SELECTED = {'postgres'}
                fixtures.SQL_TOOLS.mkdir()
                cert, key = fixtures.SQL_TOOLS / 'server.crt', fixtures.SQL_TOOLS / 'server.key'
                cert.write_text('certificate')
                key.write_text('key')
                commands, sql = [], []
                def execute(args, **kwargs):
                    args = list(map(str, args))
                    commands.append(args)
                    if 'initdb' in Path(args[0]).name:
                        self.assertIn('--username=turnloop', args)
                        pg = Path(args[args.index('-D') + 1])
                        pg.mkdir()
                        (pg / 'PG_VERSION').write_text('16')
                    if 'input' in kwargs:
                        sql.append(kwargs['input'])
                    return subprocess.CompletedProcess(args, 0, 't\n', '')
                child = Mock(pid=123)
                child.poll.return_value = None
                with patch.object(fixtures, 'SQL_BIN', fixtures.TOOLS), \
                     patch.object(fixtures, 'sql_certificates', return_value=(cert, key)), \
                     patch.object(fixtures, 'private_process', return_value=child), \
                     patch.object(fixtures.subprocess, 'run', side_effect=execute), \
                     patch.object(fixtures.subprocess, 'check_output', return_value='/var/lib/postgresql/data\n'), \
                     patch.dict(os.environ, {'POSTGRES_CONTAINER': 'pg-fixture', 'MYSQL_CONTAINER': 'mysql-fixture'}):
                    env = fixtures.sql_ci_start() if ci else fixtures.sql_start()
                self.assertIn('TURNLOOP_TEST_POSTGRES_PORT', env)
                bootstrap = [value for value in sql if 'ALTER SYSTEM' in value]
                self.assertEqual(bootstrap, [fixtures.POSTGRES_USERS + fixtures.POSTGRES_SETTINGS])
                self.assertTrue(any(args[-1] == 'SELECT pg_reload_conf()' for args in commands))
                self.assertTrue(any('pending_restart' in args[-1] for args in commands))
                if not ci:
                    self.assertTrue(any('-U' in args and args[args.index('-U') + 1] == 'turnloop' for args in commands))
                    self.assertEqual((fixtures.SQL_TOOLS / 'pgdata/server.key').read_text(), 'key')
                    self.assertNotIn('max_connections=30', (fixtures.SQL_TOOLS / 'pgdata/postgresql.conf').read_text())
                transcripts.append(bootstrap)
        self.assertEqual(*transcripts)

    def test_failed_ssl_reload_is_fatal(self):
        with private_runner() as fixtures:
            with patch.object(fixtures.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'f\n', '')) as command, \
                 patch.object(fixtures.time, 'sleep'), self.assertRaisesRegex(RuntimeError, 'reload did not take effect'):
                fixtures.postgres_configure(['psql'])
            self.assertEqual(command.call_count, 52)  # config + reload + fifty effective checks

    def test_proxy_separate_cancel_connection_bytes_and_half_closes(self):
        spec = importlib.util.spec_from_file_location('tcp_proxy', ROOT / 'scripts/fixtures/tcp_proxy.py')
        proxy_module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(proxy_module)
        cancel = struct.pack('!IIII', 16, 80877102, 123, 456)
        query = b'Q' + bytes(range(256)) * 1000
        cancelled = threading.Event()
        received = []
        errors = []
        class Backend(socketserver.BaseRequestHandler):
            def handle(self):
                try:
                    self.request.settimeout(5)
                    data = bytearray()
                    while chunk := self.request.recv(8192):
                        data.extend(chunk)
                    received.append((self.client_address[1], bytes(data)))
                    if data == cancel:
                        cancelled.set()
                    elif data == query:
                        self.request.sendall(b'active')
                        if not cancelled.wait(5):
                            raise RuntimeError('separate cancel connection never arrived')
                        self.request.sendall(b'cancelled')
                    else:
                        raise RuntimeError('incorrect forwarded bytes')
                except Exception as error:
                    errors.append(error)
        with socketserver.ThreadingTCPServer(('127.0.0.1', 0), Backend) as backend:
            thread = threading.Thread(target=backend.serve_forever)
            thread.start()
            try:
                with proxy_module.TcpProxy(backend.server_address[1]) as proxy:
                    with socket.create_connection(('127.0.0.1', proxy.port), timeout=5) as main:
                        main.sendall(query)
                        main.shutdown(socket.SHUT_WR)
                        self.assertEqual(main.recv(6), b'active')
                        with socket.create_connection(('127.0.0.1', proxy.port), timeout=5) as request:
                            for byte in cancel:
                                request.sendall(bytes([byte]))
                            request.shutdown(socket.SHUT_WR)
                            self.assertEqual(request.recv(1), b'')
                        with main.makefile('rb') as response:
                            self.assertEqual(response.read(), b'cancelled')
                self.assertEqual(proxy.connections, 2)
                self.assertEqual(proxy.cancel_requests, 1)
                self.assertEqual(proxy.bytes_forwarded, len(query) + len(cancel) + len(b'activecancelled'))
                self.assertFalse(proxy._thread.is_alive())
                self.assertEqual(proxy._sockets, set())
                self.assertEqual(errors, [])
                self.assertEqual(sorted(data for _, data in received), sorted([query, cancel]))
                self.assertEqual(len({port for port, _ in received}), 2)
            finally:
                backend.shutdown()
                thread.join(timeout=5)


if __name__ == '__main__':
    unittest.main()
