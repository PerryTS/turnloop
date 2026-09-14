#!/usr/bin/env python3
"""Private PostgreSQL/MySQL fixtures. run COMMAND always stops both servers."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / '.tools'
BIN = Path('/opt/homebrew/bin')
STATE = TOOLS / 'sql-servers.json'


def command(args, **kw):
    return subprocess.run([str(x) for x in args], check=True, **kw)


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def stop():
    if not STATE.exists():
        return
    state = json.loads(STATE.read_text())
    for item in state.get('servers', []):
        pid = item['pid']
        # Check command line against our absolute data directory before signaling.
        info = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True).stdout
        if str(TOOLS) in info and item['binary'] in info:
            os.kill(pid, signal.SIGTERM)
    for _ in range(200):
        alive = False
        for item in state.get('servers', []):
            info = subprocess.run(['ps', '-p', str(item['pid']), '-o', 'command='], capture_output=True, text=True).stdout
            alive |= str(TOOLS) in info and item['binary'] in info
        if not alive:
            STATE.unlink()
            return
        time.sleep(.1)
    raise RuntimeError('private server did not stop; see .tools logs')


def start():
    if STATE.exists():
        raise RuntimeError('state exists; run stop first')
    TOOLS.mkdir(exist_ok=True)
    cert = TOOLS / 'server.crt'
    key = TOOLS / 'server.key'
    if not cert.exists():
        command([BIN / 'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-sha256', '-days', '7',
                 '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1',
                 '-addext', 'basicConstraints=critical,CA:FALSE', '-keyout', key, '-out', cert], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        key.chmod(0o600)
        command([BIN / 'openssl', 'x509', '-in', cert, '-outform', 'DER', '-out', TOOLS / 'server.der'])
    only_mysql = os.environ.get('TURNLOOP_SQL_SERVER') == 'mysql'
    pg = TOOLS / 'pgdata'
    if not only_mysql and not (pg / 'PG_VERSION').exists():
        command([BIN / 'initdb', '-D', pg, '--username=postgres', '--auth=trust', '--encoding=UTF8', '--locale=C'], stdout=subprocess.DEVNULL)
    pgport, myport = port(), port()
    while myport == pgport:
        myport = port()
    if not only_mysql:
        (pg / 'pg_hba.conf').write_text('local all all trust\nhostssl all tls_user 127.0.0.1/32 scram-sha-256\nhostnossl all tls_user 127.0.0.1/32 reject\nhost all scram_user 127.0.0.1/32 scram-sha-256\nhost all md5_user 127.0.0.1/32 md5\nhost all clear_user 127.0.0.1/32 password\nhost all postgres 127.0.0.1/32 trust\n')
        (pg / 'postgresql.conf').write_text(f"listen_addresses='127.0.0.1'\nport={pgport}\nunix_socket_directories='{TOOLS}'\nssl=on\nssl_cert_file='{cert}'\nssl_key_file='{key}'\nmax_connections=30\n")
    state = {'servers': [], 'env': {'TURNLOOP_PG_PORT': str(pgport), 'TURNLOOP_MYSQL_PORT': str(myport), 'TURNLOOP_SQL_TOOLS': str(TOOLS)}}
    STATE.write_text(json.dumps(state))
    def spawn(binary, args, log):
        with open(TOOLS / log, 'ab') as f:
            p = subprocess.Popen([str(BIN / binary), *[str(x) for x in args]], stdout=f, stderr=f, start_new_session=True)
        state['servers'].append({'pid': p.pid, 'binary': binary})
        STATE.write_text(json.dumps(state))
        return p
    def ready(p, args):
        for _ in range(400):
            if p.poll() is not None:
                raise RuntimeError('server exited; see .tools logs')
            if subprocess.run([str(x) for x in args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                return
            time.sleep(.1)
        raise RuntimeError('server readiness timeout')
    try:
        if not only_mysql:
            p = spawn('postgres', ['-D', pg], 'postgres.log')
            psql = [BIN / 'psql', '-h', '127.0.0.1', '-p', pgport, '-U', 'postgres', '-d', 'postgres', '-v', 'ON_ERROR_STOP=1']
            ready(p, [*psql, '-c', 'SELECT 1'])
            command([*psql], input="""
    DO $$ BEGIN
     IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='scram_user') THEN CREATE ROLE scram_user LOGIN; END IF;
     IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='tls_user') THEN CREATE ROLE tls_user LOGIN; END IF;
     IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='md5_user') THEN CREATE ROLE md5_user LOGIN; END IF;
     IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname='clear_user') THEN CREATE ROLE clear_user LOGIN; END IF;
    END $$;
    SET password_encryption='scram-sha-256';
    ALTER ROLE scram_user PASSWORD 'fixture-password';
    ALTER ROLE tls_user PASSWORD 'fixture-password';
    ALTER ROLE clear_user PASSWORD 'fixture-password';
    SET password_encryption='md5';
    ALTER ROLE md5_user PASSWORD 'fixture-password';
    """, text=True, stdout=subprocess.DEVNULL)
        my = TOOLS / 'mysqldata'
        if not (my / 'mysql').exists():
            command([BIN / 'mysqld', '--no-defaults', '--initialize-insecure', f'--datadir={my}', f'--log-error={TOOLS / "mysql-init.log"}'], stdout=subprocess.DEVNULL)
        sock = TOOLS / 'mysql.sock'
        p = spawn('mysqld', ['--no-defaults', f'--datadir={my}', '--bind-address=127.0.0.1', f'--port={myport}', f'--socket={sock}', f'--pid-file={TOOLS / "mysql.pid"}', '--mysqlx=OFF', '--local-infile=ON', f'--ssl-cert={cert}', f'--ssl-key={key}', f'--ssl-ca={cert}', f'--log-error={TOOLS / "mysql.log"}'], 'mysqld-console.log')
        mysql = [BIN / 'mysql', '--no-defaults', f'--socket={sock}', '-u', 'root']
        ready(p, [*mysql, '-e', 'SELECT 1'])
        command(mysql, input="""
CREATE DATABASE IF NOT EXISTS turnloop_test;
CREATE USER IF NOT EXISTS 'sql_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password';
CREATE USER IF NOT EXISTS 'tls_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password' REQUIRE SSL;
GRANT ALL ON turnloop_test.* TO 'sql_user'@'127.0.0.1';
GRANT ALL ON turnloop_test.* TO 'tls_user'@'127.0.0.1';
""", text=True, stdout=subprocess.DEVNULL)
        native = subprocess.run([str(x) for x in mysql] + ['-e', "CREATE USER 'native_user'@'127.0.0.1' IDENTIFIED WITH mysql_native_password BY 'fixture-password'"], capture_output=True, text=True)
        (TOOLS / 'mysql-native-auth.txt').write_text(native.stdout + native.stderr)
        return state['env']
    except BaseException:
        stop()
        raise


if __name__ == '__main__':
    os.chdir(ROOT)
    action = sys.argv[1] if len(sys.argv) > 1 else ''
    if action == 'stop':
        stop()
    elif action == 'start':
        print(json.dumps(start(), indent=2))
    elif action == 'run' and len(sys.argv) > 2:
        env = start()
        try:
            result = subprocess.run(sys.argv[2:], env={**os.environ, **env})
        finally:
            stop()
        sys.exit(result.returncode)
    else:
        raise SystemExit('usage: sql-servers.py start | stop | run COMMAND ...')
