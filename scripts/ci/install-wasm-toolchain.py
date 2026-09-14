#!/usr/bin/env python3
"""Install checksum-pinned wasi-sdk clang/llvm-ar for ring on WASI and web."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

from common import ROOT, entrypoint, fail


def extract(archive, destination, digest):
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        fail('wasi-sdk: SHA-256 mismatch; refusing extraction')
    with tarfile.open(archive, 'r:gz') as bundle:
        bundle.extractall(destination, filter='data')


def environment(sdk):
    env = {}
    for target in ('wasm32_wasip2', 'wasm32_wasip3', 'wasm32_unknown_unknown'):
        env['CC_' + target] = str(sdk / 'bin/clang')
        env['AR_' + target] = str(sdk / 'bin/llvm-ar')
    return env


def main():
    host = f'{platform.system()}-{platform.machine()}'
    if host == 'Darwin-arm64':
        host = 'Darwin-aarch64'
    pins = json.loads((ROOT / 'scripts/ci/tools.json').read_text())['wasi-sdk']
    if host not in pins:
        fail(f'wasi-sdk: no reviewed artifact for {host}')
    pin = pins[host]
    destination = ROOT / '.tools'
    destination.mkdir(exist_ok=True)
    archive = destination / (pin['directory'] + '.tar.gz')
    if not archive.exists():
        # Never leave a partial download at the cache path.
        with tempfile.NamedTemporaryFile(dir=destination) as download:
            with urllib.request.urlopen(pin['url'], timeout=60) as response:
                shutil.copyfileobj(response, download)
            download.flush()
            extract(Path(download.name), destination, pin['sha256'])
            shutil.copyfile(download.name, archive)
    else:
        extract(archive, destination, pin['sha256'])
    sdk = destination / pin['directory']
    # Prove that both tools handle actual wasm objects, including on Apple hosts
    # whose system clang lacks the wasm target. Rust supplies its own WASI libc.
    with tempfile.TemporaryDirectory(dir=destination) as folder:
        obj = Path(folder) / 'probe.o'
        lib = Path(folder) / 'probe.a'
        for target in ('wasm32-wasip2', 'wasm32-wasip3', 'wasm32-unknown-unknown'):
            subprocess.run([str(sdk / 'bin/clang'), '--target=' + target,
                            '-x', 'c', '-c', '-o', str(obj), '-'],
                           input=b'int wasm_probe(void) { return 42; }', check=True)
            if obj.read_bytes()[:4] != b'\0asm':
                fail(f'{target}: compiler did not emit a wasm object')
            subprocess.run([str(sdk / 'bin/llvm-ar'), 'crs', str(lib), str(obj)], check=True)
            listing = subprocess.check_output([str(sdk / 'bin/llvm-ar'), 't', str(lib)], text=True)
            if listing.strip() != 'probe.o':
                fail(f'{target}: archiver did not retain the probe')
    env = environment(sdk)
    local = destination / 'wasm-env.sh'
    local.write_text(''.join(f'export {key}={shlex.quote(value)}\n' for key, value in env.items()))
    if github_env := os.environ.get('GITHUB_ENV'):
        with open(github_env, 'a', encoding='utf-8') as output:
            output.writelines(f'{key}={value}\n' for key, value in env.items())
    print(f'PASS wasi-sdk {pin["version"]}: sha256:{pin["sha256"]}; 3 compiler/archive probes')
    print(f'Local builds: source {shlex.quote(str(local))}')


if __name__ == '__main__':
    entrypoint(main)
