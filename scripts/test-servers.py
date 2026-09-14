#!/usr/bin/env python3
"""Private protocol fixtures. start/stop/run; only instances under .tools are touched.

Default: all services, fail on missing capability. Select a subset explicitly with
--services postgres,mysql,redis,mongodb,smtp,http; never silently bypass unavailable tests.
"""
import argparse
from contextlib import contextmanager, nullcontext
import shutil
import shlex
from pathlib import Path
import subprocess
import os
import sys
import json
import signal
import socket
import time
import secrets
import stat
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / '.tools'
STATE = TOOLS / 'test-servers.json'
SELECTED = set()
CI_SERVICES = False
POSTGRES_HBA = """local all all trust
hostssl all tls_user 127.0.0.1/32 scram-sha-256
hostnossl all tls_user 127.0.0.1/32 reject
host all scram_user 127.0.0.1/32 scram-sha-256
host all md5_user 127.0.0.1/32 md5
host all clear_user 127.0.0.1/32 password
"""

# Shared by local and Docker provisioning; ALTER SYSTEM is deliberately applied
# after startup in both environments, followed by a reload and effective checks.
POSTGRES_SETTINGS = """ALTER SYSTEM SET ssl='on';
ALTER SYSTEM SET ssl_cert_file='server.crt';
ALTER SYSTEM SET ssl_key_file='server.key';
"""

MYSQL_USERS = """CREATE DATABASE IF NOT EXISTS turnloop_test;
CREATE USER IF NOT EXISTS 'auth_admin'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password' REQUIRE SSL;
GRANT RELOAD ON *.* TO 'auth_admin'@'127.0.0.1';
GRANT SELECT ON turnloop_test.* TO 'auth_admin'@'127.0.0.1';
CREATE USER IF NOT EXISTS 'auth_rsa_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password';
GRANT ALL ON turnloop_test.* TO 'auth_rsa_user'@'127.0.0.1';
CREATE USER IF NOT EXISTS 'sql_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password';
CREATE USER IF NOT EXISTS 'tls_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password' REQUIRE SSL;
GRANT ALL ON turnloop_test.* TO 'sql_user'@'127.0.0.1';
GRANT ALL ON turnloop_test.* TO 'tls_user'@'127.0.0.1';
"""

POSTGRES_USERS = """DO $$ BEGIN
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

GRANT ALL ON SCHEMA public TO scram_user, tls_user, md5_user, clear_user;
"""

@contextmanager
def startup_logs(name, *paths):
    """Report bounded tails even for config errors emitted before server logging starts."""
    try:
        yield
    except BaseException:
        print(f'{name} startup failed; private server log tails:', file=sys.stderr, flush=True)
        for path in paths:
            try:
                with Path(path).open('rb') as log:
                    log.seek(0, os.SEEK_END)
                    log.seek(max(0, log.tell() - 16384))
                    tail = b'\n'.join(log.read().splitlines()[-40:]).decode(errors='replace')
            except OSError as error:
                tail = f'(log unavailable: {error})'
            print(f'--- {path} ---\n{tail}', file=sys.stderr, flush=True)
        raise


def private_process(args, log_path, *, env=None):
    with Path(log_path).open('wb') as log:
        return subprocess.Popen([str(x) for x in args], stdout=log, stderr=log,
                                start_new_session=True, env=env)


def cleanup_after_failure(error, cleanup):
    """Keep the original failure primary and retain a failing cleanup as its cause."""
    try:
        cleanup()
    except Exception as cleanup_error:
        raise error from cleanup_error


def wait_port(name, value, process, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f'{name} exited with status {process.returncode} before listening on {value}')
        try:
            with socket.create_connection(('127.0.0.1', value), timeout=.1):
                return
        except OSError:
            time.sleep(.025)
    raise RuntimeError(f'{name} did not start on {value} within {timeout}s')


def find_binary(name):
    binary = shutil.which(name)
    if binary:
        return binary
    for directory in ('/opt/homebrew/bin', '/usr/libexec/postfix', '/usr/lib/postfix/sbin'):
        p = Path(directory) / name
        if p.is_file():
            return str(p)
    for p in sorted(Path('/usr/lib/postgresql').glob('*/bin/' + name), reverse=True):
        return str(p)
    raise RuntimeError(f'{name} is required; install it outside the sandbox or supply it on PATH')

class BinaryDirectory:
    def __truediv__(self, name):
        return find_binary(name)


SQL_ROOT = Path(__file__).resolve().parent.parent
SQL_TOOLS = SQL_ROOT / '.tools' / 'sql'
SQL_BIN = BinaryDirectory()
SQL_STATE = SQL_TOOLS / 'sql-servers.json'
SQL_CHILDREN = {}


def sql_command(args, **kw):
    return subprocess.run([str(x) for x in args], check=True, **kw)


def sql_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]


def sql_stop():
    if not SQL_STATE.exists():
        return
    state = json.loads(SQL_STATE.read_text())
    for item in state.get('servers', []):
        child = SQL_CHILDREN.get(item['pid'])
        if child is not None:
            # Popen still owns this child: an unreaped child's PID cannot be reused.
            if child.poll() is None:
                child.send_signal(signal.SIGINT if item['binary'] == 'postgres' else signal.SIGTERM)
                child.wait(timeout=30)
            continue
        # Separate start/stop invocations address only our explicit data/socket
        # paths. No ps (sandbox blocks it), default instance, or arbitrary PID kill.
        if item['binary'] == 'postgres':
            sql_command([SQL_BIN / 'pg_ctl', '-D', SQL_TOOLS / 'pgdata', '-m', 'fast', '-w', '-t', '20', 'stop'])
        elif item['binary'] == 'mysqld':
            sql_command([SQL_BIN / 'mysqladmin', '--no-defaults', '--connect-timeout=3',
                     f'--socket={SQL_TOOLS / "mysql.sock"}', '-u', 'root', 'shutdown'])
        else:
            raise RuntimeError('unknown private server in state file')
    for name in ('pgdata', 'mysqldata'):
        remove_private_data(SQL_TOOLS / name, SQL_TOOLS)
    SQL_CHILDREN.clear()
    SQL_STATE.unlink()


def remove_private_data(directory, parent):
    """Remove only private data, after its server stopped; never follow symlinks."""
    directory, parent = Path(directory), Path(parent).resolve()
    if directory.is_symlink() or not directory.resolve().is_relative_to(parent) or directory.resolve() == parent:
        raise RuntimeError(f'Invalid private data directory: {directory}')
    if not directory.exists():
        return
    # Native servers and Docker --user use our UID. Repair restrictive modes
    # before traversal; logs and certificates live outside these data roots.
    directory.chmod(0o700)
    for root, dirs, _files in os.walk(directory, followlinks=False):
        for name in dirs:
            path = Path(root) / name
            if not path.is_symlink():
                path.chmod(0o700)
    shutil.rmtree(directory)


def postgres_configure(psql):
    sql_command(psql, input=POSTGRES_USERS + POSTGRES_SETTINGS, text=True, stdout=subprocess.DEVNULL)
    sql_command([*psql, '-c', 'SELECT pg_reload_conf()'])
    effective = """SELECT current_setting('ssl') = 'on'
        AND current_setting('ssl_cert_file') = 'server.crt'
        AND current_setting('ssl_key_file') = 'server.key'
        AND NOT EXISTS (SELECT FROM pg_settings WHERE pending_restart)"""
    for _ in range(50):
        result = subprocess.run([str(x) for x in [*psql, '-Atc', effective]],
                                check=True, capture_output=True, text=True)
        if result.stdout.strip() == 't':
            break
        time.sleep(.1)
    else:
        raise RuntimeError('PostgreSQL TLS reload did not take effect')
    sql_command([*psql, '-c', """SELECT version();
        SELECT rolname, rolsuper FROM pg_roles WHERE rolname IN
          ('postgres', 'turnloop', 'scram_user', 'tls_user', 'md5_user', 'clear_user');
        SELECT name, setting, source, pending_restart FROM pg_settings WHERE name IN
          ('ssl', 'ssl_cert_file', 'ssl_key_file', 'statement_timeout',
           'idle_in_transaction_session_timeout', 'idle_session_timeout', 'max_connections')
          ORDER BY name;"""])


def sql_certificates():
    SQL_TOOLS.mkdir(parents=True, exist_ok=True)
    cert = SQL_TOOLS / 'server.crt'
    key = SQL_TOOLS / 'server.key'
    if not cert.exists() or subprocess.run([SQL_BIN / 'openssl', 'x509', '-in', cert, '-checkend', '86400', '-noout'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0:
        sql_command([SQL_BIN / 'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-sha256', '-days', '7',
                 '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1',
                 '-addext', 'basicConstraints=critical,CA:FALSE', '-keyout', key, '-out', cert], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        key.chmod(0o600)
        sql_command([SQL_BIN / 'openssl', 'x509', '-in', cert, '-outform', 'DER', '-out', SQL_TOOLS / 'server.der'])
    return cert, key


def sql_start():
    if SQL_STATE.exists():
        raise RuntimeError('state exists; run stop first')
    cert, key = sql_certificates()
    only_mysql = "postgres" not in SELECTED
    pg = SQL_TOOLS / 'pgdata'
    # Record before initdb/mysqld initialization so failed partial data is removed.
    state = {'servers': [], 'env': {}}
    SQL_STATE.write_text(json.dumps(state))
    if not only_mysql and not (pg / 'PG_VERSION').exists():
        try:
            with startup_logs('PostgreSQL initdb', SQL_TOOLS / 'postgres-init.log'), (SQL_TOOLS / 'postgres-init.log').open('wb') as log:
                sql_command([SQL_BIN / 'initdb', '-D', pg, '--username=turnloop', '--auth=trust', '--encoding=UTF8', '--locale=C'], stdout=log, stderr=log)
        except BaseException as error:
            cleanup_after_failure(error, sql_stop)
            raise
    pgport, myport = sql_port(), sql_port()
    while myport == pgport:
        myport = sql_port()
    if not only_mysql:
        (pg / 'pg_hba.conf').write_text(POSTGRES_HBA.replace('127.0.0.1/32', '0.0.0.0/0'))
        (pg / 'postgresql.conf').write_text(f"listen_addresses='127.0.0.1'\nport={pgport}\nunix_socket_directories='{SQL_TOOLS}'\n")
        shutil.copyfile(cert, pg / 'server.crt')
        shutil.copyfile(key, pg / 'server.key')
        (pg / 'server.key').chmod(0o600)
    state = {'servers': [], 'env': {'TURNLOOP_TEST_POSTGRES_PORT': str(pgport), 'TURNLOOP_TEST_MYSQL_PORT': str(myport), 'TURNLOOP_TEST_SQL_TOOLS': str(SQL_TOOLS)}}
    SQL_STATE.write_text(json.dumps(state))
    def spawn(binary, args, log):
        p = private_process([SQL_BIN / binary, *args], SQL_TOOLS / log)
        SQL_CHILDREN[p.pid] = p
        state['servers'].append({'pid': p.pid, 'binary': binary})
        SQL_STATE.write_text(json.dumps(state))
        return p
    def ready(p, args):
        for _ in range(400):
            if p.poll() is not None:
                raise RuntimeError(f'server exited with status {p.returncode} before readiness')
            if subprocess.run([str(x) for x in args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                return
            time.sleep(.1)
        raise RuntimeError('server readiness timeout')
    try:
        if not only_mysql:
            with startup_logs('PostgreSQL', SQL_TOOLS / 'postgres.log'):
                p = spawn('postgres', ['-D', pg], 'postgres.log')
                psql = [SQL_BIN / 'psql', '-h', SQL_TOOLS, '-p', pgport, '-U', 'turnloop', '-d', 'postgres', '-v', 'ON_ERROR_STOP=1']
                ready(p, [*psql, '-c', 'SELECT 1'])
                postgres_configure(psql)
        if 'mysql' not in SELECTED:
            return {k:v for k,v in state['env'].items() if not (k == 'TURNLOOP_TEST_MYSQL_PORT' and 'mysql' not in SELECTED) and not (k == 'TURNLOOP_TEST_POSTGRES_PORT' and 'postgres' not in SELECTED)}
        my = SQL_TOOLS / 'mysqldata'
        if not (my / 'mysql').exists():
            with startup_logs('MySQL initialization', SQL_TOOLS / 'mysql-init-console.log', SQL_TOOLS / 'mysql-init.log'), (SQL_TOOLS / 'mysql-init-console.log').open('wb') as log:
                sql_command([SQL_BIN / 'mysqld', '--no-defaults', '--initialize-insecure', f'--datadir={my}', f'--log-error={SQL_TOOLS / "mysql-init.log"}'], stdout=log, stderr=log)
        sock = SQL_TOOLS / 'mysql.sock'
        with startup_logs('MySQL', SQL_TOOLS / 'mysqld-console.log', SQL_TOOLS / 'mysql.log'):
            p = spawn('mysqld', ['--no-defaults', f'--datadir={my}', '--bind-address=127.0.0.1', f'--port={myport}', f'--socket={sock}', f'--pid-file={SQL_TOOLS / "mysql.pid"}', '--mysqlx=OFF', '--local-infile=ON', f'--ssl-cert={cert}', f'--ssl-key={key}', f'--ssl-ca={cert}', f'--log-error={SQL_TOOLS / "mysql.log"}'], 'mysqld-console.log')
            mysql = [SQL_BIN / 'mysql', '--no-defaults', f'--socket={sock}', '-u', 'root']
            ready(p, [*mysql, '-e', 'SELECT 1'])
            sql_command(mysql, input=MYSQL_USERS, text=True, stdout=subprocess.DEVNULL)
            native = subprocess.run([str(x) for x in mysql] + ['-e', "CREATE USER 'native_user'@'127.0.0.1' IDENTIFIED WITH mysql_native_password BY 'fixture-password'"], capture_output=True, text=True)
            (SQL_TOOLS / 'mysql-native-auth.txt').write_text(native.stdout + native.stderr)
        return {k:v for k,v in state['env'].items() if not (k == 'TURNLOOP_TEST_MYSQL_PORT' and 'mysql' not in SELECTED) and not (k == 'TURNLOOP_TEST_POSTGRES_PORT' and 'postgres' not in SELECTED)}
    except BaseException as error:
        cleanup_after_failure(error, sql_stop)
        raise



import random

REDIS_ROOT = Path(__file__).resolve().parents[1]
REDIS_DATA = REDIS_ROOT / '.tools' / 'redis'
REDIS_STATE = REDIS_DATA / 'instances.json'
REDIS_SERVER = 'redis-server'
REDIS_CLI = 'redis-cli'
REDIS_CHILDREN = {}
REDIS_FIXTURES = REDIS_ROOT / 'protocols/turnloop-smtp/tests/fixtures'
TURNLOOP_TEST_REDIS_PASSWORD = 'turnloop-test-password'


def redis_port():
    while True:
        candidate = random.randrange(30000, 45000)
        probes = []
        try:
            for value in (candidate, candidate + 10000):
                sock = socket.socket()
                probes.append(sock)
                sock.bind(('127.0.0.1', value))
            return candidate
        except OSError:
            pass
        finally:
            for sock in probes:
                sock.close()


def redis_start():
    REDIS_DATA.mkdir(parents=True, exist_ok=True)
    if REDIS_STATE.exists():
        raise RuntimeError('Private state exists; run stop before start')
    records = []
    ports = []
    try:
        for label in ('single', 'cluster0', 'cluster1', 'cluster2', 'cluster3', 'cluster4', 'cluster5', 'sentinel'):
            value = redis_port()
            while value in ports or (ports and value == tls_port):
                value = redis_port()
            ports.append(value)
            directory = REDIS_DATA / label
            directory.mkdir(exist_ok=True)
            config = directory / 'redis.conf'
            lines = [f'port {value}', 'bind 127.0.0.1', 'protected-mode yes',
                     f'dir {directory}', 'save ""', 'appendonly no', 'daemonize no',
                     'logfile ""']
            if label == 'single':
                tls_port = redis_port()
                while tls_port in ports:
                    tls_port = redis_port()
                lines += [f'requirepass {TURNLOOP_TEST_REDIS_PASSWORD}', f'user lane on >{TURNLOOP_TEST_REDIS_PASSWORD} ~* &* +@all',
                          f'tls-port {tls_port}', 'tls-auth-clients no',
                          f'tls-cert-file {REDIS_FIXTURES / "server.pem"}',
                          f'tls-key-file {REDIS_FIXTURES / "server-key.pem"}',
                          f'tls-ca-cert-file {REDIS_FIXTURES / "ca.pem"}']
            elif label.startswith('cluster'):
                # Old nodes.conf contains stale node endpoints and must not be reused.
                node_file = directory / 'nodes.conf'
                node_file.unlink(missing_ok=True)
                lines += ['cluster-enabled yes', 'cluster-config-file nodes.conf',
                          'cluster-node-timeout 1000', 'cluster-announce-ip 127.0.0.1',
                          f'cluster-announce-port {value}']
            else:
                lines += [f'sentinel monitor turnloop 127.0.0.1 {ports[0]} 1',
                          f'sentinel auth-pass turnloop {TURNLOOP_TEST_REDIS_PASSWORD}']
            config.write_text('\n'.join(lines) + '\n')
            with startup_logs(f'Redis {label}', directory / 'server.log'):
                process = private_process([REDIS_SERVER, config] + (['--sentinel'] if label == 'sentinel' else []), directory / 'server.log')
                REDIS_CHILDREN[process.pid] = process
                records.append({'pid': process.pid, 'port': value, 'config': str(config)})
                REDIS_STATE.write_text(json.dumps(records))
                wait_port(f'Redis {label}', value, process)
        with startup_logs('Redis cluster', *[REDIS_DATA / f'cluster{i}' / 'server.log' for i in range(6)]):
            subprocess.run([REDIS_CLI, '--cluster', 'create', *[f'127.0.0.1:{p}' for p in ports[1:7]],
                            '--cluster-replicas', '1', '--cluster-yes'], check=True)
            for _ in range(100):
                info = subprocess.check_output([REDIS_CLI, '-p', str(ports[1]), 'CLUSTER', 'INFO'])
                if b'cluster_state:ok' in info:
                    break
                time.sleep(.05)
            else:
                raise RuntimeError('Cluster did not become healthy')
        env = dict(TURNLOOP_TEST_REDIS_PORT=str(ports[0]), TURNLOOP_TEST_REDIS_PASSWORD=TURNLOOP_TEST_REDIS_PASSWORD, TURNLOOP_TEST_REDIS_TLS_PORT=str(tls_port),
                   TURNLOOP_TEST_REDIS_CLUSTER_PORT=str(ports[1]), TURNLOOP_TEST_REDIS_SENTINEL_PORT=str(ports[7]))
        (REDIS_DATA / 'env.json').write_text(json.dumps(env))
        return env
    except BaseException as error:
        cleanup_after_failure(error, redis_stop)
        raise


def redis_stop_record(record):
    child = REDIS_CHILDREN.get(record['pid'])
    if child is not None:
        # Popen owns this exact child, including a start that never opened a port.
        # Reap crashes before probing ports: a reused port is not our instance.
        if child.poll() is None:
            try:
                child.terminate()
            except ProcessLookupError:
                pass  # Exited between poll and terminate; still reap it below.
        child.wait(timeout=10)
        REDIS_CHILDREN.pop(record['pid'])
        return
    # Separate invocations never signal a stored PID. An existence check only
    # helps recognize crashes; protocol identity must match before shutdown.
    try:
        os.kill(record['pid'], 0)
    except ProcessLookupError:
        return
    except PermissionError:
        pass  # The sandbox can deny even existence checks; use the private port.
    env = dict(os.environ)
    if Path(record['config']).parent.name == 'single':
        env['REDISCLI_AUTH'] = TURNLOOP_TEST_REDIS_PASSWORD
    command = [REDIS_CLI, '-h', '127.0.0.1', '-p', str(record['port'])]
    try:
        with socket.create_connection(('127.0.0.1', record['port']), timeout=.2):
            pass
    except ConnectionRefusedError:
        return
    info = subprocess.run([*command, '--raw', 'INFO', 'server'],
                          capture_output=True, text=True, env=env, timeout=5)
    if info.returncode != 0 and 'Connection refused' in info.stderr:
        return
    expected = 'config_file:' + record['config']
    if expected not in info.stdout.splitlines():
        raise RuntimeError(f"Port {record['port']} does not identify our private config; left untouched")
    # Redis may close the connection during SHUTDOWN or crash after INFO. A
    # failed command is only a stop failure if the instance is still listening.
    try:
        subprocess.run([*command, 'SHUTDOWN', 'NOSAVE'], env=env,
                       capture_output=True, timeout=5)
    except subprocess.TimeoutExpired:
        pass
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(('127.0.0.1', record['port']), timeout=.2):
                pass
        except ConnectionRefusedError:
            return
        time.sleep(.05)
    raise RuntimeError(f"Private Redis on {record['port']} did not stop; instance file retained")


def redis_stop():
    if not REDIS_STATE.exists():
        return
    remaining, errors = [], []
    for record in json.loads(REDIS_STATE.read_text()):
        try:
            redis_stop_record(record)
        except Exception as error:
            remaining.append(record)
            errors.append(error)
    if remaining:
        REDIS_STATE.write_text(json.dumps(remaining))
        raise ExceptionGroup('Private Redis cleanup failed; live/unverified instances retained', errors)
    REDIS_STATE.unlink()
    (REDIS_DATA / 'env.json').unlink(missing_ok=True)



import json, os, pathlib, signal, socket, subprocess, sys, time, secrets
MONGO_ROOT = pathlib.Path(__file__).resolve().parent.parent
MONGO_RUN = MONGO_ROOT / '.tools' / 'mongodb'
MONGO_MANIFEST = MONGO_RUN / 'servers.json'
MONGO_PROCESSES = {}

def mongo_stop():
    if not MONGO_MANIFEST.exists():
        return
    state = json.loads(MONGO_MANIFEST.read_text())
    if (TOOLS / 'docker-fixtures.json').exists():
        # Stopping a docker-run client alone does not prove mongod has exited.
        docker_cleanup()
        for process in MONGO_PROCESSES.values():
            process.wait(timeout=40)
    elif not MONGO_PROCESSES and any(port_open(entry['port']) for entry in state['servers']):
        subprocess.run(['cargo', 'run', '-p', 'turnloop-mongodb', '--example', 'cleanup-private'], cwd=MONGO_ROOT, check=True, env={**os.environ, **mongo_env()})
    else:
        for process in MONGO_PROCESSES.values():
            if process.poll() is None:
                process.terminate()
        for process in MONGO_PROCESSES.values():
            process.wait(timeout=40)
    for entry in state['servers']:
        if port_open(entry['port']):
            raise RuntimeError('Private server still listening: ' + entry['name'])
    for entry in state['servers']:
        if 'dbpath' in entry:
            remove_private_data(entry['dbpath'], MONGO_RUN)
    MONGO_PROCESSES.clear()
    MONGO_MANIFEST.unlink()
    print('Private MongoDB servers stopped', flush=True)


def port_open(port):
    try:
        with socket.create_connection(('127.0.0.1', port), timeout=.2):
            return True
    except ConnectionRefusedError:
        return False

def mongo_start():
    if MONGO_MANIFEST.exists():
        raise RuntimeError('Existing private run manifest; stop it first')
    MONGO_RUN.mkdir(parents=True, exist_ok=True)
    key = MONGO_RUN / 'keyfile'
    key.write_text(secrets.token_urlsafe(384).replace('-', 'a').replace('_', 'b'))
    key.chmod(0o600)
    config = MONGO_RUN / 'openssl.cnf'
    config.write_text('[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')
    subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','2','-config',str(config),'-keyout',str(MONGO_RUN/'key.pem'),'-out',str(MONGO_RUN/'cert.pem')], check=True, capture_output=True)
    (MONGO_RUN/'server.pem').write_bytes((MONGO_RUN/'key.pem').read_bytes()+(MONGO_RUN/'cert.pem').read_bytes())
    state = {'servers': []}
    sockets=[]
    for _ in range(5):
        s=socket.socket(); s.bind(('127.0.0.1',0)); sockets.append(s)
    try:
        for i,name in enumerate(['standalone','rs0','rs1','rs2','tls']):
            port=sockets[i].getsockname()[1];sockets[i].close()
            directory=MONGO_RUN/name
            directory.mkdir(exist_ok=True)
            # New run DB path preserves earlier logs and prevents leftover users.
            db=directory/secrets.token_hex(6);db.mkdir()
            entry = {'name': name, 'port': port, 'dbpath': str(db)}
            state['servers'].append(entry)
            MONGO_MANIFEST.write_text(json.dumps(state))
            cmd=[find_binary('mongod'),'--bind_ip','127.0.0.1','--port',str(port),'--dbpath',str(db),'--logpath',str(directory/'mongod.log'),'--logappend','--nounixsocket','--setParameter','enableTestCommands=1','--wiredTigerCacheSizeGB','0.25']
            if name.startswith('rs'):
                cmd += ['--replSet','turnloop_test','--keyFile',str(key)]
            else:
                cmd += ['--auth']
            if name=='tls':
                cmd += ['--tlsMode','requireTLS','--tlsCertificateKeyFile',str(MONGO_RUN/'server.pem'),'--tlsCAFile',str(MONGO_RUN/'cert.pem'),'--tlsAllowConnectionsWithoutCertificates']
            with startup_logs(f'MongoDB {name}', directory / 'process.log', directory / 'mongod.log'):
                p = private_process(cmd, directory / 'process.log')
                MONGO_PROCESSES[name] = p
                entry['pid'] = p.pid
                MONGO_MANIFEST.write_text(json.dumps(state))
                wait_port(f'MongoDB {name}', port, p, timeout=30)
        print('Private MongoDB ready: '+json.dumps(state),flush=True)
        return mongo_env()
    except BaseException as error:
        cleanup_after_failure(error, mongo_stop)
        raise
    finally:
        for s in sockets:s.close()


def mongo_env():
    state = json.loads(MONGO_MANIFEST.read_text())
    ports = {s['name']: str(s['port']) for s in state['servers']}
    return {'TURNLOOP_TEST_MONGODB_PORT': ports['standalone'],
            'TURNLOOP_TEST_MONGODB_REPLICA_PORTS': ','.join(ports['rs' + str(i)] for i in range(3)),
            'TURNLOOP_TEST_MONGODB_TLS_PORT': ports['tls'],
            'TURNLOOP_TEST_MONGODB_TOOLS': str(MONGO_RUN)}

SMTP_CHILD = None

def smtp_supervisor(directory):
    """Own smtp-sink and accept authenticated local stop requests across invocations."""
    config = json.loads((directory / 'control.json').read_text())
    control = socket.socket(socket.AF_UNIX)
    control.bind(str(directory / 'control.sock'))
    control.listen(1)
    child = None
    try:
        # Inherit the supervisor's private server.log for both output streams.
        child = subprocess.Popen([find_binary('smtp-sink'), '-4', '-d',
            str(directory / 'message-%Y%m%d%H%M%S'), f"127.0.0.1:{config['port']}", '10'])
        control.settimeout(0.5)
        while child.poll() is None:
            try:
                request, _ = control.accept()
            except TimeoutError:
                continue
            with request:
                request.settimeout(2)
                if request.recv(256).decode() == config['token']:
                    child.terminate()
                    child.wait(timeout=10)
                    request.sendall(b'stopped')
                    return
        raise RuntimeError('smtp-sink exited before shutdown')
    finally:
        if child is not None:
            if child.poll() is None:
                child.terminate()
            child.wait(timeout=10)
        control.close()
        (directory / 'control.sock').unlink(missing_ok=True)


def smtp_start():
    global SMTP_CHILD
    directory = TOOLS / 'smtp'
    directory.mkdir(parents=True, exist_ok=True)
    for old in directory.glob('message-*'):
        old.unlink()
    value = sql_port()
    config = directory / 'control.json'
    config.write_text(json.dumps({'port': value, 'token': secrets.token_hex(32)}))
    config.chmod(0o600)
    with startup_logs('SMTP', directory / 'server.log'):
        SMTP_CHILD = private_process([sys.executable, Path(__file__).resolve(),
                                      'smtp-supervisor', directory], directory / 'server.log')
        wait_port('SMTP', value, SMTP_CHILD)
    return {'TURNLOOP_TEST_SMTP_PORT': str(value), 'TURNLOOP_TEST_SMTP_TOOLS': str(directory)}


def smtp_stop():
    directory = TOOLS / 'smtp'
    control = directory / 'control.sock'
    if SMTP_CHILD is not None and SMTP_CHILD.poll() is not None:
        control.unlink(missing_ok=True)
    if control.exists():
        config = json.loads((directory / 'control.json').read_text())
        with socket.socket(socket.AF_UNIX) as request:
            request.settimeout(15)
            request.connect(str(control))
            request.sendall(config['token'].encode())
            if request.recv(32) != b'stopped':
                raise RuntimeError('SMTP supervisor did not confirm shutdown')
    if SMTP_CHILD is not None:
        SMTP_CHILD.wait(timeout=15)
    (directory / 'control.json').unlink(missing_ok=True)

HTTP_CHILD = None


def http_start():
    global HTTP_CHILD
    directory = TOOLS / 'http'
    directory.mkdir(parents=True, exist_ok=True)
    state_path = directory / 'state.json'
    if state_path.exists():
        raise RuntimeError('Private HTTP state exists; stop it first')
    token = secrets.token_hex(24)
    log_path = directory / 'server.log'
    with startup_logs('HTTP', log_path):
        HTTP_CHILD = private_process([find_binary('node'), ROOT / 'scripts/fixtures/http-server.mjs'],
            log_path, env={**os.environ, 'TURNLOOP_TEST_HTTP_TOKEN': token})
        def stop_failed_child():
            if HTTP_CHILD.poll() is None:
                HTTP_CHILD.terminate()
            HTTP_CHILD.wait(timeout=10)
        try:
            deadline = time.monotonic() + 10
            while True:
                if HTTP_CHILD.poll() is not None:
                    raise RuntimeError(f'HTTP fixture exited with status {HTTP_CHILD.returncode} before readiness')
                lines = log_path.read_text().splitlines()
                if lines:
                    ports = json.loads(lines[0])
                    if set(ports) != {'h1', 'h2'} or any(type(p) is not int or not 1024 < p < 65536 for p in ports.values()):
                        raise RuntimeError('Invalid HTTP fixture ports')
                    state_path.write_text(json.dumps({'ports': ports, 'token': token}))
                    return {'TURNLOOP_TEST_HTTP_PORT': str(ports['h1']),
                            'TURNLOOP_TEST_HTTP2_PORT': str(ports['h2'])}
                if time.monotonic() >= deadline:
                    raise RuntimeError('HTTP fixture startup timed out')
                time.sleep(.02)
        except BaseException as error:
            cleanup_after_failure(error, stop_failed_child)
            raise


def http_stop():
    state_path = TOOLS / 'http/state.json'
    if not state_path.exists():
        return
    state = json.loads(state_path.read_text())
    ports = state['ports']
    if any(type(p) is not int or not 1024 < p < 65536 for p in ports.values()):
        raise RuntimeError('Invalid private HTTP state')
    request = urllib.request.Request(f'http://127.0.0.1:{ports["h1"]}/__turnloop_shutdown',
        method='POST', headers={'x-turnloop-test-token': state['token']})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=5) as response:
        if response.read() != b'stopping':
            raise RuntimeError('Private HTTP identity was not verified')
    deadline = time.monotonic() + 10
    for port in ports.values():
        while True:
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=.1):
                    pass
            except OSError:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('Private HTTP listener did not close')
            time.sleep(.02)
    if HTTP_CHILD is not None:
        HTTP_CHILD.wait(timeout=10)
    state_path.unlink()


def stop_all():
    errors = []
    for stop in (http_stop, smtp_stop, mongo_stop, redis_stop, sql_stop, docker_cleanup):
        try:
            stop()
        except Exception as error:
            errors.append(error)
    if errors:
        raise ExceptionGroup('Private fixture cleanup failed', errors)
    STATE.unlink(missing_ok=True)

def start_all():
    if STATE.exists():
        raise RuntimeError('Private state exists: run stop first')
    TOOLS.mkdir(exist_ok=True)
    env = {'TURNLOOP_TEST_REQUIRED': '1'}
    state = {'services': sorted(SELECTED), 'env': env}
    STATE.write_text(json.dumps(state))
    try:
        if SELECTED & {'postgres', 'mysql'}:
            env.update(sql_ci_start() if CI_SERVICES else sql_start())
        if 'redis' in SELECTED:
            env.update(redis_start())
        if 'mongodb' in SELECTED:
            env.update(mongo_start())
        if 'smtp' in SELECTED:
            env.update(smtp_start())
            state['smtp_pid'] = SMTP_CHILD.pid
        if 'http' in SELECTED:
            env.update(http_start())
        STATE.write_text(json.dumps(state))
        return env
    except BaseException as error:
        cleanup_after_failure(error, stop_all)
        raise


def collect_logs():
    """Stage only known log files; artifact upload must never walk database data."""
    destination = TOOLS / 'protocol-logs'
    destination.mkdir(parents=True, exist_ok=True)
    paths = list(SQL_TOOLS.glob('*.log'))
    paths += list(REDIS_DATA.glob('*/server.log'))
    for name in ('standalone', 'rs0', 'rs1', 'rs2', 'tls'):
        paths += [MONGO_RUN / name / log for log in ('process.log', 'mongod.log')]
    paths += [TOOLS / name / 'server.log' for name in ('smtp', 'http')]
    for source in sorted(paths):
        try:
            if not stat.S_ISREG(source.lstat().st_mode):
                continue
            # Flatten filenames so the destination can contain no server dirs.
            target = destination / '-'.join(source.relative_to(TOOLS).parts)
            shutil.copyfile(source, target)
            target.chmod(0o644)
            with source.open('rb') as log:
                log.seek(0, os.SEEK_END)
                log.seek(max(0, log.tell() - 16384))
                tail = b'\n'.join(log.read().splitlines()[-40:]).decode(errors='replace')
            print(f'--- {source} ---\n{tail}', file=sys.stderr, flush=True)
        except FileNotFoundError:
            continue
        except OSError as error:
            print(f'Log unavailable: {source}: {error}', file=sys.stderr, flush=True)


def main():
    global SELECTED, CI_SERVICES
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ci-services', action='store_true', help='Linux CI: provision PostgreSQL/MySQL service containers; run native Redis and private Mongo containers')
    parser.add_argument('--services', default='postgres,mysql,redis,mongodb,smtp,http')
    parser.add_argument('--postgres-proxy', action='store_true', help='run only: forward PostgreSQL TCP, including separate CancelRequests, like docker-proxy')
    parser.add_argument('action', choices=['start', 'stop', 'run', 'logs'])
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    SELECTED = set(args.services.split(','))
    CI_SERVICES = args.ci_services
    if CI_SERVICES:
        prepare_docker_wrappers()
    if SELECTED - {'postgres', 'mysql', 'redis', 'mongodb', 'smtp', 'http'}:
        parser.error('unknown service')
    if args.postgres_proxy and (args.action != 'run' or 'postgres' not in SELECTED):
        parser.error('--postgres-proxy requires run with the postgres service')
    os.chdir(ROOT)
    if args.action == 'logs':
        collect_logs()
    elif args.action == 'stop':
        stop_all()
    elif args.action == 'start':
        for key, value in start_all().items():
            print('export ' + key + '=' + shlex.quote(value))
    else:
        if not args.command:
            parser.error('run needs a command')
        env = start_all()
        try:
            # Loaded here so importlib-based fixture tests need no sys.path changes.
            if args.postgres_proxy:
                import importlib.util
                spec = importlib.util.spec_from_file_location('tcp_proxy', ROOT / 'scripts/fixtures/tcp_proxy.py')
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                context = module.TcpProxy(int(env['TURNLOOP_TEST_POSTGRES_PORT']))
            else:
                context = nullcontext()
            with context as proxy:
                if proxy is not None:
                    print(f'PostgreSQL proxy: {proxy.port} -> {env["TURNLOOP_TEST_POSTGRES_PORT"]}', flush=True)
                    env['TURNLOOP_TEST_POSTGRES_PORT'] = str(proxy.port)
                result = subprocess.run(args.command, env={**os.environ, **env})
            if proxy is not None:
                message = (f'PostgreSQL proxy forwarded {proxy.bytes_forwarded} bytes on '
                           f'{proxy.connections} connections; {proxy.cancel_requests} CancelRequests')
                print(message, flush=True)
                (SQL_TOOLS / 'postgres-proxy.log').write_text(message + '\n')
            if result.returncode:
                raise subprocess.CalledProcessError(result.returncode, args.command)
        except BaseException as error:
            collect_logs()
            cleanup_after_failure(error, stop_all)
            if isinstance(error, subprocess.CalledProcessError):
                return error.returncode
            raise
        stop_all()
        return 0
    return 0



def prepare_docker_wrappers():
    """Use ordinary fixture commands in isolated, named Linux containers.

    Only the two job-owned SQL service containers are reconfigured. Mongo uses
    private host-network ports and the same repository paths as native fixtures.
    Redis uses the pinned TLS-enabled native build installed by CI.
    Wrappers and their exact container IDs are recorded under .tools.
    """
    directory = TOOLS / 'test-bin'
    directory.mkdir(parents=True, exist_ok=True)
    wrapper = '''#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys, uuid
root = pathlib.Path(__file__).resolve().parents[1]
name = pathlib.Path(sys.argv[0]).name
image = 'mongo:8'
container = 'turnloop-fixture-' + uuid.uuid4().hex
record = root / 'docker-fixtures.json'
existing = json.loads(record.read_text()) if record.exists() else []
existing.append(container)
record.write_text(json.dumps(existing))
args = ['docker', 'run', '--rm', '--name', container, '--label', 'turnloop.fixture=' + str(root),
        '--user', str(os.getuid()) + ':' + str(os.getgid()),
        '--network', 'host', '--volume', str(root.parent) + ':' + str(root.parent),
        '--workdir', str(root.parent), '--entrypoint', name]
if 'REDISCLI_AUTH' in os.environ:
    args += ['--env', 'REDISCLI_AUTH']
args += [image, *sys.argv[1:]]
os.execvp(args[0], args)
'''
    # Wrapper filenames are fixed and never supplied by test inputs.
    # Remove wrappers left by older runner versions so they cannot shadow CI's build.
    for name in ('redis-server', 'redis-cli'):
        (directory / name).unlink(missing_ok=True)
    for name in ('mongod',):
        path = directory / name
        path.write_text(wrapper)
        path.chmod(0o755)
    os.environ['PATH'] = str(directory) + os.pathsep + os.environ['PATH']


def docker_cleanup():
    record = TOOLS / 'docker-fixtures.json'
    if not record.exists():
        return
    for name in json.loads(record.read_text()):
        if not name.startswith('turnloop-fixture-'):
            raise RuntimeError('Invalid private container record')
        probe = subprocess.run(['docker', 'inspect', '--format', '{{index .Config.Labels "turnloop.fixture"}}', name], capture_output=True, text=True)
        if probe.returncode:
            continue  # --rm already removed this exact container.
        if probe.stdout.strip() != str(TOOLS):
            raise RuntimeError('Container ownership mismatch; left untouched')
        subprocess.run(['docker', 'logs', name], check=True)
        subprocess.run(['docker', 'rm', '--force', name], check=True)
    record.unlink()


def sql_ci_start():
    """Provision the explicit GitHub SQL containers with the native fixture contract."""
    pg = os.environ['POSTGRES_CONTAINER']
    mysql = os.environ['MYSQL_CONTAINER']
    cert, key = sql_certificates()
    psql = ['docker', 'exec', '-i', pg, 'psql', '-U', 'turnloop', '-d', 'postgres', '-v', 'ON_ERROR_STOP=1']
    pgdata = subprocess.check_output([*psql, '-Atc', 'SHOW data_directory'], text=True).strip()
    for source, name in ((cert, 'server.crt'), (key, 'server.key')):
        subprocess.run(['docker', 'cp', str(source), pg + ':' + pgdata + '/' + name], check=True)
    subprocess.run(['docker', 'exec', '--user', 'root', pg, 'chown', 'postgres:postgres', pgdata + '/server.crt', pgdata + '/server.key'], check=True)
    subprocess.run(['docker', 'exec', '--user', 'root', pg, 'chmod', '600', pgdata + '/server.key'], check=True)
    hba = SQL_TOOLS / 'pg_hba.conf'
    hba.write_text(POSTGRES_HBA.replace('127.0.0.1/32', '0.0.0.0/0'))
    subprocess.run(['docker', 'cp', str(hba), pg + ':' + pgdata + '/pg_hba.conf'], check=True)
    postgres_configure(psql)
    # MySQL's default data-directory certificate names are picked up on restart.
    for source, name in ((cert, 'ca.pem'), (cert, 'server-cert.pem'), (key, 'server-key.pem')):
        subprocess.run(['docker', 'cp', str(source), mysql + ':/var/lib/mysql/' + name], check=True)
    subprocess.run(['docker', 'exec', '--user', 'root', mysql, 'chown', 'mysql:mysql', '/var/lib/mysql/ca.pem', '/var/lib/mysql/server-cert.pem', '/var/lib/mysql/server-key.pem'], check=True)
    subprocess.run(['docker', 'exec', '--user', 'root', mysql, 'chmod', '600', '/var/lib/mysql/server-key.pem'], check=True)
    subprocess.run(['docker', 'restart', mysql], check=True)
    mycli = ['docker', 'exec', '-i', '-e', 'MYSQL_PWD=turnloop-root', mysql, 'mysql', '-uroot']
    for _ in range(90):
        if subprocess.run([*mycli, '-e', 'SELECT 1'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            break
        time.sleep(1)
    else:
        raise RuntimeError('MySQL service did not restart')
    # Docker NAT's client address differs; these users exist only in this disposable job.
    subprocess.run(mycli, input=MYSQL_USERS.replace("@'127.0.0.1'", "@'%'") + '\nSET GLOBAL local_infile=ON;\n', text=True, check=True)
    native = subprocess.run([*mycli, '-e', "CREATE USER 'native_user'@'%' IDENTIFIED WITH mysql_native_password BY 'fixture-password'"], capture_output=True, text=True)
    (SQL_TOOLS / 'mysql-native-auth.txt').write_text(native.stdout + native.stderr)
    # Independent verified TLS probes; Rust suites assert protocol results afterward.
    subprocess.run(['docker', 'exec', '-e', 'PGPASSWORD=fixture-password', pg, 'psql', 'host=localhost hostaddr=127.0.0.1 user=tls_user dbname=postgres sslmode=verify-full sslrootcert=' + pgdata + '/server.crt', '-v', 'ON_ERROR_STOP=1', '-c', 'SELECT 1'], check=True)
    subprocess.run(['docker', 'exec', '-e', 'MYSQL_PWD=fixture-password', mysql, 'mysql', '-h127.0.0.1', '-utls_user', '--ssl-mode=VERIFY_IDENTITY', '--ssl-ca=/var/lib/mysql/ca.pem', '-e', 'SELECT 1'], check=True)
    return {'TURNLOOP_TEST_POSTGRES_PORT': '5432', 'TURNLOOP_TEST_MYSQL_PORT': '3306',
            'TURNLOOP_TEST_SQL_TOOLS': str(SQL_TOOLS)}


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == 'smtp-supervisor':
        smtp_supervisor(Path(sys.argv[2]))
    else:
        sys.exit(main())
