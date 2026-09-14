#!/usr/bin/env python3
"""Fail if a resolved package is an async runtime or lettre has extra features."""
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'], cwd=root))
forbidden = {'tokio', 'tokio-util', 'async-std', 'smol', 'async-io', 'async-executor', 'futures-executor'}
found = forbidden & {package['name'] for package in metadata['packages']}
assert not found, f'Runtime dependencies: {found}'
lettre = next(package for package in metadata['packages'] if package['name'] == 'lettre')
node = next(node for node in metadata['resolve']['nodes'] if node['id'] == lettre['id'])
assert node['features'] == ['builder'], f'Unexpected lettre features: {node["features"]}'
print(f'PASS: {len(metadata["packages"])} resolved packages; no async runtimes; lettre features = builder')
