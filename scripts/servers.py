#!/usr/bin/env python3
"""Private Redis/cluster/Sentinel lifecycle. Never discovers or touches default servers.
Usage: python3 scripts/servers.py test|start|stop
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

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / '.tools' / 'redis'
STATE = DATA / 'instances.json'
SERVER = '/opt/homebrew/bin/redis-server'
CLI = '/opt/homebrew/bin/redis-cli'
FIXTURES = ROOT / 'protocols/turnloop-smtp/tests/fixtures'
PASSWORD = 'turnloop-test-password'


def port():
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


def wait_port(value, process):
    for _ in range(200):
        if process.poll() is not None:
            raise RuntimeError(f'Redis exited: {process.returncode}; inspect {DATA}')
        try:
            with socket.create_connection(('127.0.0.1', value), timeout=.1):
                return
        except OSError:
            time.sleep(.025)
    raise RuntimeError(f'Redis did not start on {value}')


def start():
    DATA.mkdir(parents=True, exist_ok=True)
    if STATE.exists():
        raise RuntimeError('Private state exists; run stop before start')
    records = []
    ports = []
    try:
        for label in ('single', 'cluster0', 'cluster1', 'cluster2', 'sentinel'):
            value = port()
            while value in ports:
                value = port()
            ports.append(value)
            directory = DATA / label
            directory.mkdir(exist_ok=True)
            config = directory / 'redis.conf'
            lines = [f'port {value}', 'bind 127.0.0.1', 'protected-mode yes',
                     f'dir {directory}', 'save ""', 'appendonly no', 'daemonize no',
                     f'logfile {directory / "server.log"}']
            if label == 'single':
                tls_port = port()
                lines += [f'requirepass {PASSWORD}', f'user lane on >{PASSWORD} ~* &* +@all',
                          f'tls-port {tls_port}', 'tls-auth-clients no',
                          f'tls-cert-file {FIXTURES / "server.pem"}',
                          f'tls-key-file {FIXTURES / "server-key.pem"}',
                          f'tls-ca-cert-file {FIXTURES / "ca.pem"}']
            elif label.startswith('cluster'):
                # Old nodes.conf contains stale node endpoints and must not be reused.
                node_file = directory / 'nodes.conf'
                node_file.unlink(missing_ok=True)
                lines += ['cluster-enabled yes', 'cluster-config-file nodes.conf',
                          'cluster-node-timeout 1000', 'cluster-announce-ip 127.0.0.1',
                          f'cluster-announce-port {value}']
            else:
                lines += [f'sentinel monitor turnloop 127.0.0.1 {ports[0]} 1',
                          f'sentinel auth-pass turnloop {PASSWORD}']
            config.write_text('\n'.join(lines) + '\n')
            process = subprocess.Popen([SERVER, str(config)] + (['--sentinel'] if label == 'sentinel' else []),
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            records.append({'pid': process.pid, 'port': value, 'config': str(config)})
            STATE.write_text(json.dumps(records))
            wait_port(value, process)
        subprocess.run([CLI, '--cluster', 'create', *[f'127.0.0.1:{p}' for p in ports[1:4]],
                        '--cluster-replicas', '0', '--cluster-yes'], check=True)
        for _ in range(100):
            info = subprocess.check_output([CLI, '-p', str(ports[1]), 'CLUSTER', 'INFO'])
            if b'cluster_state:ok' in info:
                break
            time.sleep(.05)
        else:
            raise RuntimeError('Cluster did not become healthy')
        env = dict(REDIS_PORT=str(ports[0]), REDIS_PASSWORD=PASSWORD, REDIS_TLS_PORT=str(tls_port),
                   CLUSTER_PORT=str(ports[1]), SENTINEL_PORT=str(ports[4]))
        (DATA / 'env.json').write_text(json.dumps(env))
        return env
    except BaseException:
        stop()
        raise


def stop():
    if not STATE.exists():
        return
    for record in json.loads(STATE.read_text()):
        # Redis INFO identifies the exact configuration loaded by this instance.
        # Use the protocol instead of ps/lsof (unavailable in managed sandboxes).
        env = dict(os.environ)
        if Path(record['config']).parent.name == 'single':
            env['REDISCLI_AUTH'] = PASSWORD
        info = subprocess.run([CLI, '-h', '127.0.0.1', '-p', str(record['port']),
                               '--raw', 'INFO', 'server'], capture_output=True, text=True, env=env)
        if info.returncode != 0 and 'Connection refused' in info.stderr:
            continue
        expected = 'config_file:' + record['config']
        if expected not in info.stdout.splitlines():
            raise RuntimeError(f"Port {record['port']} does not identify our private config; left untouched")
        subprocess.run([CLI, '-h', '127.0.0.1', '-p', str(record['port']),
                        'SHUTDOWN', 'NOSAVE'], check=True, env=env, capture_output=True)
    for _ in range(100):
        alive = False
        for record in json.loads(STATE.read_text()):
            try:
                with socket.create_connection(('127.0.0.1', record['port']), timeout=.05):
                    alive = True
            except OSError:
                pass
        if not alive:
            STATE.unlink()
            return
        time.sleep(.05)
    raise RuntimeError('Private Redis did not stop; instance file retained')


if __name__ == '__main__':
    action = sys.argv[1] if len(sys.argv) > 1 else 'test'
    if action == 'stop':
        stop()
    elif action == 'start':
        print(json.dumps(start(), indent=2))
    elif action == 'test':
        try:
            env = start()
            result = subprocess.run(['cargo', 'test', '--workspace', '--', '--include-ignored'],
                                    cwd=ROOT, env={**os.environ, **env})
        finally:
            stop()
        sys.exit(result.returncode)
    else:
        raise SystemExit('usage: servers.py start|stop|test')
