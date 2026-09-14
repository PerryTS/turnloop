#!/usr/bin/env python3
"""Build the local fixture's exact Redis release with TLS; cache only its binaries."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
VERSION = '8.4.0'
# Official release digest: https://github.com/redis/redis-hashes/blob/master/README
SHA256 = 'ca909aa15252f2ecb3a048cd086469827d636bf8334f50bb94d03fba4bfc56e8'
URL = f'https://download.redis.io/releases/redis-{VERSION}.tar.gz'
DESTINATION = ROOT / '.tools/redis-build'
BUILD_FLAGS = ['BUILD_TLS=yes', 'BUILD_WITH_MODULES=no', 'MALLOC=libc', 'USE_SYSTEMD=no']


def source_archive():
    with urllib.request.urlopen(URL, timeout=120) as response:
        archive = response.read()
    digest = hashlib.sha256(archive).hexdigest()
    if digest != SHA256:
        raise RuntimeError(f'Redis SHA-256 mismatch: expected {SHA256}, got {digest}')
    print(f'Verified Redis {VERSION} source SHA-256: {digest}', flush=True)
    return archive


def verify_binaries(directory):
    server = subprocess.check_output([directory / 'redis-server', '--version'], text=True)
    cli = subprocess.check_output([directory / 'redis-cli', '--version'], text=True)
    help_text = subprocess.check_output([directory / 'redis-cli', '--help'], text=True)
    if f'v={VERSION}' not in server.split() or cli.strip() != f'redis-cli {VERSION}':
        raise RuntimeError(f'Wrong Redis fixture version: {server.strip()}, {cli.strip()}')
    if '--tls' not in help_text.split():
        raise RuntimeError('Redis CLI lacks TLS support')
    print(server.strip(), flush=True)
    print(cli.strip(), flush=True)


def install(destination=DESTINATION):
    identity = {'version': VERSION, 'sha256': SHA256, 'flags': BUILD_FLAGS,
                'installer': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    marker = destination / 'build.json'
    if marker.exists() and json.loads(marker.read_text()) == identity:
        verify_binaries(destination / 'bin')
        print('Reusing cached Redis TLS build', flush=True)
    else:
        destination.parent.mkdir(parents=True, exist_ok=True)
        # Verify before extraction or executing any source. Build in a fresh tree;
        # a failed build never creates a successful cache marker.
        archive = source_archive()
        with tempfile.TemporaryDirectory(prefix='redis-source-', dir=destination.parent) as tmp:
            source = Path(tmp) / f'redis-{VERSION}'
            with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as bundle:
                bundle.extractall(tmp, filter='data')
            subprocess.run(['make', '-C', str(source / 'src'), '-j', str(min(os.cpu_count() or 2, 4)),
                            *BUILD_FLAGS, 'redis-server', 'redis-cli'], check=True)
            verify_binaries(source / 'src')
            marker.unlink(missing_ok=True)
            binaries = destination / 'bin'
            binaries.mkdir(parents=True, exist_ok=True)
            for name in ('redis-server', 'redis-cli'):
                shutil.copy2(source / 'src' / name, binaries / name)
            marker.write_text(json.dumps(identity) + '\n')
    binaries = (destination / 'bin').resolve()
    if os.environ.get('GITHUB_PATH'):
        with open(os.environ['GITHUB_PATH'], 'a', encoding='utf-8') as handle:
            handle.write(str(binaries) + '\n')
    print(f'Redis fixture binaries: {binaries}', flush=True)


if __name__ == '__main__':
    install()
