#!/usr/bin/env python3
"""Private protocol fixtures. start/stop/run; only instances under .tools are touched.

Default: all services, fail on missing capability. Select a subset explicitly with
--services postgres,mysql,redis,mongodb,smtp; never silently bypass unavailable tests.
"""
import argparse
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

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / '.tools'
STATE = TOOLS / 'test-servers.json'
SELECTED = set()

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
    SQL_STATE.unlink()


def sql_start():
    if SQL_STATE.exists():
        raise RuntimeError('state exists; run stop first')
    SQL_TOOLS.mkdir(parents=True, exist_ok=True)
    cert = SQL_TOOLS / 'server.crt'
    key = SQL_TOOLS / 'server.key'
    if not cert.exists():
        sql_command([SQL_BIN / 'openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-sha256', '-days', '7',
                 '-subj', '/CN=localhost', '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1',
                 '-addext', 'basicConstraints=critical,CA:FALSE', '-keyout', key, '-out', cert], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        key.chmod(0o600)
        sql_command([SQL_BIN / 'openssl', 'x509', '-in', cert, '-outform', 'DER', '-out', SQL_TOOLS / 'server.der'])
    only_mysql = "postgres" not in SELECTED
    pg = SQL_TOOLS / 'pgdata'
    if not only_mysql and not (pg / 'PG_VERSION').exists():
        with open(SQL_TOOLS / 'postgres-init.log', 'ab') as log:
            sql_command([SQL_BIN / 'initdb', '-D', pg, '--username=postgres', '--auth=trust', '--encoding=UTF8', '--locale=C'], stdout=log, stderr=log)
    pgport, myport = sql_port(), sql_port()
    while myport == pgport:
        myport = sql_port()
    if not only_mysql:
        (pg / 'pg_hba.conf').write_text('local all all trust\nhostssl all tls_user 127.0.0.1/32 scram-sha-256\nhostnossl all tls_user 127.0.0.1/32 reject\nhost all scram_user 127.0.0.1/32 scram-sha-256\nhost all md5_user 127.0.0.1/32 md5\nhost all clear_user 127.0.0.1/32 password\nhost all postgres 127.0.0.1/32 trust\n')
        (pg / 'postgresql.conf').write_text(f"listen_addresses='127.0.0.1'\nport={pgport}\nunix_socket_directories='{SQL_TOOLS}'\nssl=on\nssl_cert_file='{cert}'\nssl_key_file='{key}'\nmax_connections=30\n")
    state = {'servers': [], 'env': {'TURNLOOP_TEST_POSTGRES_PORT': str(pgport), 'TURNLOOP_TEST_MYSQL_PORT': str(myport), 'TURNLOOP_TEST_SQL_TOOLS': str(SQL_TOOLS)}}
    SQL_STATE.write_text(json.dumps(state))
    def spawn(binary, args, log):
        with open(SQL_TOOLS / log, 'ab') as f:
            p = subprocess.Popen([str(SQL_BIN / binary), *[str(x) for x in args]], stdout=f, stderr=f, start_new_session=True)
        SQL_CHILDREN[p.pid] = p
        state['servers'].append({'pid': p.pid, 'binary': binary})
        SQL_STATE.write_text(json.dumps(state))
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
            psql = [SQL_BIN / 'psql', '-h', '127.0.0.1', '-p', pgport, '-U', 'postgres', '-d', 'postgres', '-v', 'ON_ERROR_STOP=1']
            ready(p, [*psql, '-c', 'SELECT 1'])
            sql_command([*psql], input="""
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
        if 'mysql' not in SELECTED:
            return {k:v for k,v in state['env'].items() if not (k == 'TURNLOOP_TEST_MYSQL_PORT' and 'mysql' not in SELECTED) and not (k == 'TURNLOOP_TEST_POSTGRES_PORT' and 'postgres' not in SELECTED)}
        my = SQL_TOOLS / 'mysqldata'
        if not (my / 'mysql').exists():
            sql_command([SQL_BIN / 'mysqld', '--no-defaults', '--initialize-insecure', f'--datadir={my}', f'--log-error={SQL_TOOLS / "mysql-init.log"}'], stdout=subprocess.DEVNULL)
        sock = SQL_TOOLS / 'mysql.sock'
        p = spawn('mysqld', ['--no-defaults', f'--datadir={my}', '--bind-address=127.0.0.1', f'--port={myport}', f'--socket={sock}', f'--pid-file={SQL_TOOLS / "mysql.pid"}', '--mysqlx=OFF', '--local-infile=ON', f'--ssl-cert={cert}', f'--ssl-key={key}', f'--ssl-ca={cert}', f'--log-error={SQL_TOOLS / "mysql.log"}'], 'mysqld-console.log')
        mysql = [SQL_BIN / 'mysql', '--no-defaults', f'--socket={sock}', '-u', 'root']
        ready(p, [*mysql, '-e', 'SELECT 1'])
        sql_command(mysql, input="""
CREATE DATABASE IF NOT EXISTS turnloop_test;
CREATE USER IF NOT EXISTS 'auth_rsa_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password';
GRANT ALL ON turnloop_test.* TO 'auth_rsa_user'@'127.0.0.1';
CREATE USER IF NOT EXISTS 'sql_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password';
CREATE USER IF NOT EXISTS 'tls_user'@'127.0.0.1' IDENTIFIED WITH caching_sha2_password BY 'fixture-password' REQUIRE SSL;
GRANT ALL ON turnloop_test.* TO 'sql_user'@'127.0.0.1';
GRANT ALL ON turnloop_test.* TO 'tls_user'@'127.0.0.1';
""", text=True, stdout=subprocess.DEVNULL)
        native = subprocess.run([str(x) for x in mysql] + ['-e', "CREATE USER 'native_user'@'127.0.0.1' IDENTIFIED WITH mysql_native_password BY 'fixture-password'"], capture_output=True, text=True)
        (SQL_TOOLS / 'mysql-native-auth.txt').write_text(native.stdout + native.stderr)
        return {k:v for k,v in state['env'].items() if not (k == 'TURNLOOP_TEST_MYSQL_PORT' and 'mysql' not in SELECTED) and not (k == 'TURNLOOP_TEST_POSTGRES_PORT' and 'postgres' not in SELECTED)}
    except BaseException:
        sql_stop()
        raise



#!/usr/bin/env python3
"""Private Redis/cluster/Sentinel lifecycle. Never discovers or touches default servers.
Usage: python3 scripts/test-servers.py run cargo test --workspace -- --include-ignored|start|stop
`test` always stops its instances, including on cargo failure or interrupt.
"""
import json
import os
from pathlib import Path
import random
import socket
import subprocess
import sys
import time

REDIS_ROOT = Path(__file__).resolve().parents[1]
REDIS_DATA = REDIS_ROOT / '.tools' / 'redis'
REDIS_STATE = REDIS_DATA / 'instances.json'
REDIS_SERVER = 'redis-server'
REDIS_CLI = 'redis-cli'
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


def redis_wait_port(value, process):
    for _ in range(200):
        if process.poll() is not None:
            raise RuntimeError(f'Redis exited: {process.returncode}; inspect {REDIS_DATA}')
        try:
            with socket.create_connection(('127.0.0.1', value), timeout=.1):
                return
        except OSError:
            time.sleep(.025)
    raise RuntimeError(f'Redis did not start on {value}')


def redis_start():
    REDIS_DATA.mkdir(parents=True, exist_ok=True)
    if REDIS_STATE.exists():
        raise RuntimeError('Private state exists; run stop before start')
    records = []
    ports = []
    try:
        for label in ('single', 'cluster0', 'cluster1', 'cluster2', 'cluster3', 'cluster4', 'cluster5', 'sentinel'):
            value = redis_port()
            while value in ports:
                value = redis_port()
            ports.append(value)
            directory = REDIS_DATA / label
            directory.mkdir(exist_ok=True)
            config = directory / 'redis.conf'
            lines = [f'port {value}', 'bind 127.0.0.1', 'protected-mode yes',
                     f'dir {directory}', 'save ""', 'appendonly no', 'daemonize no',
                     f'logfile {directory / "server.log"}']
            if label == 'single':
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
            process = subprocess.Popen([REDIS_SERVER, str(config)] + (['--sentinel'] if label == 'sentinel' else []),
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            records.append({'pid': process.pid, 'port': value, 'config': str(config)})
            REDIS_STATE.write_text(json.dumps(records))
            redis_wait_port(value, process)
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
    except BaseException:
        redis_stop()
        raise


def redis_stop():
    if not REDIS_STATE.exists():
        return
    for record in json.loads(REDIS_STATE.read_text()):
        # Redis INFO identifies the exact configuration loaded by this instance.
        # Use the protocol instead of ps/lsof (unavailable in managed sandboxes).
        env = dict(os.environ)
        if Path(record['config']).parent.name == 'single':
            env['REDISCLI_AUTH'] = TURNLOOP_TEST_REDIS_PASSWORD
        info = subprocess.run([REDIS_CLI, '-h', '127.0.0.1', '-p', str(record['port']),
                               '--raw', 'INFO', 'server'], capture_output=True, text=True, env=env)
        if info.returncode != 0 and 'Connection refused' in info.stderr:
            continue
        expected = 'config_file:' + record['config']
        if expected not in info.stdout.splitlines():
            raise RuntimeError(f"Port {record['port']} does not identify our private config; left untouched")
        subprocess.run([REDIS_CLI, '-h', '127.0.0.1', '-p', str(record['port']),
                        'SHUTDOWN', 'NOSAVE'], check=True, env=env, capture_output=True)
    for _ in range(100):
        alive = False
        for record in json.loads(REDIS_STATE.read_text()):
            try:
                with socket.create_connection(('127.0.0.1', record['port']), timeout=.05):
                    alive = True
            except OSError:
                pass
        if not alive:
            REDIS_STATE.unlink()
            return
        time.sleep(.05)
    raise RuntimeError('Private Redis did not stop; instance file retained')



#!/usr/bin/env python3
"""Private lane servers only. start/stop/run; never uses default MongoDB ports."""
import json, os, pathlib, signal, socket, subprocess, sys, time, secrets
MONGO_ROOT = pathlib.Path(__file__).resolve().parent.parent
MONGO_RUN = MONGO_ROOT / '.tools' / 'mongodb'
MONGO_MANIFEST = MONGO_RUN / 'servers.json'
MONGO_PROCESSES = {}

def mongo_stop():
    if not MONGO_MANIFEST.exists():
        return
    state = json.loads(MONGO_MANIFEST.read_text())
    if not MONGO_PROCESSES:
        subprocess.run(['cargo', 'run', '-p', 'turnloop-mongodb', '--example', 'cleanup-private'], cwd=MONGO_ROOT, check=True, env={**os.environ, **mongo_env()})
    else:
        for process in MONGO_PROCESSES.values():
            if process.poll() is None:
                process.terminate()
        for process in MONGO_PROCESSES.values():
            process.wait(timeout=40)
    for entry in state['servers']:
        try:
            with socket.create_connection(('127.0.0.1', entry['port']), timeout=.2):
                raise RuntimeError('Private server still listening: ' + entry['name'])
        except OSError:
            pass
    MONGO_MANIFEST.unlink()
    print('Private MongoDB servers stopped', flush=True)

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
            cmd=[find_binary('mongod'),'--bind_ip','127.0.0.1','--port',str(port),'--dbpath',str(db),'--logpath',str(directory/'mongod.log'),'--logappend','--nounixsocket','--setParameter','enableTestCommands=1','--wiredTigerCacheSizeGB','0.25']
            if name.startswith('rs'):
                cmd += ['--replSet','turnloop_test','--keyFile',str(key)]
            else:
                cmd += ['--auth']
            if name=='tls':
                cmd += ['--tlsMode','requireTLS','--tlsCertificateKeyFile',str(MONGO_RUN/'server.pem'),'--tlsCAFile',str(MONGO_RUN/'cert.pem'),'--tlsAllowConnectionsWithoutCertificates']
            with open(directory/'process.log','ab') as log:
                p=subprocess.Popen(cmd,stdout=log,stderr=log,start_new_session=True)
            MONGO_PROCESSES[name] = p
            state['servers'].append({'name':name,'port':port,'pid':p.pid})
            MONGO_MANIFEST.write_text(json.dumps(state))
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                if p.poll() is not None:
                    raise RuntimeError(f'{name} exited: '+(directory/'process.log').read_text()[-2000:])
                try:
                    with socket.create_connection(('127.0.0.1',port),timeout=.2):pass
                    break
                except OSError:time.sleep(.1)
            else:raise RuntimeError(name+' did not listen')
        print('Private MongoDB ready: '+json.dumps(state),flush=True)
        return mongo_env()
    except BaseException:
        mongo_stop();raise
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

def smtp_start():
    global SMTP_CHILD
    directory = TOOLS / 'smtp'
    directory.mkdir(parents=True, exist_ok=True)
    for old in directory.glob('message-*'):
        old.unlink()
    value = sql_port()
    with (directory / 'server.log').open('ab') as log:
        SMTP_CHILD = subprocess.Popen([find_binary('smtp-sink'), '-4', '-d',
            str(directory / 'message-%Y%m%d%H%M%S'), f'127.0.0.1:{value}', '10'],
            stdout=log, stderr=log, start_new_session=True)
    redis_wait_port(value, SMTP_CHILD)
    return {'TURNLOOP_TEST_SMTP_PORT': str(value), 'TURNLOOP_TEST_SMTP_TOOLS': str(directory)}

def smtp_stop():
    if SMTP_CHILD is not None:
        if SMTP_CHILD.poll() is None:
            SMTP_CHILD.terminate()
        SMTP_CHILD.wait(timeout=10)
    elif STATE.exists() and 'smtp' in json.loads(STATE.read_text())['services']:
        # Separate invocations identify the exact private dump directory before signalling.
        state = json.loads(STATE.read_text())
        pid = state['smtp_pid']
        result = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True, check=True)
        if str(TOOLS / 'smtp' / 'message-') not in result.stdout or 'smtp-sink' not in result.stdout:
            raise RuntimeError('Cannot identify private SMTP process; left untouched')
        os.kill(pid, signal.SIGTERM)

def stop_all():
    errors = []
    for stop in (smtp_stop, mongo_stop, redis_stop, sql_stop):
        try:
            stop()
        except Exception as error:
            errors.append(str(error))
    if errors:
        raise RuntimeError('; '.join(errors))
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
            env.update(sql_start())
        if 'redis' in SELECTED:
            env.update(redis_start())
        if 'mongodb' in SELECTED:
            env.update(mongo_start())
        if 'smtp' in SELECTED:
            env.update(smtp_start())
            state['smtp_pid'] = SMTP_CHILD.pid
        STATE.write_text(json.dumps(state))
        return env
    except BaseException:
        stop_all()
        raise

def main():
    global SELECTED
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--services', default='postgres,mysql,redis,mongodb,smtp')
    parser.add_argument('action', choices=['start', 'stop', 'run'])
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    SELECTED = set(args.services.split(','))
    if SELECTED - {'postgres', 'mysql', 'redis', 'mongodb', 'smtp'}:
        parser.error('unknown service')
    os.chdir(ROOT)
    if args.action == 'stop':
        stop_all()
    elif args.action == 'start':
        for key, value in start_all().items():
            print('export ' + key + '=' + shlex.quote(value))
    else:
        if not args.command:
            parser.error('run needs a command')
        env = start_all()
        try:
            return subprocess.run(args.command, env={**os.environ, **env}).returncode
        finally:
            stop_all()
    return 0

if __name__ == '__main__':
    sys.exit(main())
