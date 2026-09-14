#!/usr/bin/env python3
"""Build checksum-pinned h2spec and require all 147 strict conformance tests to pass."""
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile
import time
import urllib.request
import xml.etree.ElementTree as ET
from common import ROOT, cargo, entrypoint, fail, metadata, run

EXPECTED_TESTS = 147


def extract_source(archive, pin, destination):
    digest = hashlib.sha256(archive).hexdigest()
    if digest != pin['sha256']:
        fail(f'h2spec source checksum mismatch: {digest}')
    prefix = 'h2spec-' + pin['commit']
    with tarfile.open(fileobj=io.BytesIO(archive), mode='r:gz') as bundle:
        for member in bundle.getmembers():
            path = Path(member.name)
            if path.is_absolute() or '..' in path.parts or path.parts[0] != prefix:
                fail('Invalid h2spec archive path')
            if member.isdir():
                continue
            if not member.isfile():
                fail('h2spec archive contains a non-regular file')
            target = destination.joinpath(*path.parts[1:])
            target.parent.mkdir(parents=True, exist_ok=True)
            with bundle.extractfile(member) as source:
                target.write_bytes(source.read())
    print('PASS h2spec source sha256:' + digest)


def install():
    pin = json.loads((ROOT / 'scripts/ci/tools.json').read_text())['h2spec-source']
    destination = ROOT / '.tools/bin'
    destination.mkdir(parents=True, exist_ok=True)
    binary = destination / ('h2spec.exe' if os.name == 'nt' else 'h2spec')
    with urllib.request.urlopen(pin['url'], timeout=60) as response:
        archive = response.read()
    with tempfile.TemporaryDirectory(dir=ROOT / '.tools', prefix='h2spec-') as folder:
        source = Path(folder)
        extract_source(archive, pin, source)
        env = os.environ.copy()
        for key in ('GOMODCACHE', 'GOCACHE', 'GOPATH'):
            env[key] = str(ROOT / '.tools' / key.lower())
        env['GOTOOLCHAIN'] = 'local'
        # go.sum belongs to the checksum-verified commit; never rewrite dependency pins.
        run(['go', 'mod', 'download'], cwd=source, env=env)
        run(['go', 'mod', 'verify'], cwd=source, env=env)
        command = ['go', 'build', '-mod=readonly', '-trimpath']
        if platform.system() == 'Darwin':
            command += ['-ldflags=-linkmode=external']
        run(command + ['-o', str(binary), './cmd/h2spec'], cwd=source, env=env)
    if platform.system() == 'Darwin':
        run(['codesign', '--force', '--sign', '-', str(binary)])
    print('h2spec executable sha256:' + hashlib.sha256(binary.read_bytes()).hexdigest())
    return binary


def check_report(path):
    report = ET.parse(path).getroot()
    cases = report.findall('.//testcase')
    identities = {(c.get('package'), c.get('classname'), c.get('name')) for c in cases}
    bad = report.findall('.//failure') + report.findall('.//error') + report.findall('.//skipped')
    suites = report.findall('.//testsuite')
    if len(cases) != EXPECTED_TESTS or len(identities) != EXPECTED_TESTS or bad:
        fail(f'h2spec requires {EXPECTED_TESTS} distinct tests, 0 failed/errors/skipped; got {len(cases)} tests, {len(bad)} unsuccessful')
    if sum(int(s.get('tests', '0')) for s in suites) != EXPECTED_TESTS or any(
        int(s.get(key, '0')) != 0 for s in suites for key in ('failures', 'errors', 'skipped')):
        fail('h2spec suite counters disagree with complete successful execution')
    print(f'PASS h2spec strict: {len(cases)} tests, {len(cases)} passed, 0 failed, 0 skipped')


def main():
    binary = install()
    data = metadata()
    run(cargo() + ['build', '--locked', '-p', 'turnloop-http', '--example', 'h2spec_server', '--features', 'turnloop'], cwd=ROOT)
    server_binary = Path(data['target_directory']) / 'debug/examples/h2spec_server'
    if os.name == 'nt':
        server_binary = server_binary.with_suffix('.exe')
    report = ROOT / '.tools/h2spec.xml'
    report.unlink(missing_ok=True)  # stale success cannot satisfy this run
    stdout_path = ROOT / '.tools/h2spec-server.stdout'
    with stdout_path.open('w') as stdout, (ROOT / '.tools/h2spec-server.log').open('w') as stderr:
        server = subprocess.Popen([str(server_binary)], stdout=stdout, stderr=stderr)
        try:
            deadline = time.monotonic() + 10
            while not stdout_path.read_text().strip():
                if server.poll() is not None or time.monotonic() >= deadline:
                    fail('h2spec server failed to start')
                time.sleep(.02)
            port = int(stdout_path.read_text().strip())
            if not 1024 < port < 65536:
                fail('h2spec server returned invalid port')
            result = subprocess.run([str(binary), '-h', '127.0.0.1', '-p', str(port),
                '-o', '1', '--strict', '-j', str(report)], stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT, text=True, timeout=240)
            (ROOT / '.tools/h2spec.log').write_text(result.stdout)
            print(result.stdout)
            result.check_returncode()
            check_report(report)
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


if __name__ == '__main__':
    entrypoint(main)
