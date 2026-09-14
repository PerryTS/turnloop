#!/usr/bin/env python3
"""Metadata-driven test entry points; missing suites and zero passed tests are errors."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
from web_fixture import WebFixture
from common import PIN, P3_PIN, ROOT, cargo, entrypoint, fail, members, metadata, role, run, select, settings


def checked_tests(command, *, cwd, env=None, minimum_groups=1):
    # Preserve the output and exit code; never turn ignored/filtered tests into passes.
    print('+ ' + ' '.join(map(str, command)), flush=True)
    process = subprocess.Popen(command, cwd=cwd, env=env, text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    output = []
    for line in process.stdout:
        print(line, end='', flush=True)
        output.append(line)
    process.stdout.close()
    if process.wait():
        raise subprocess.CalledProcessError(process.returncode, command)
    groups = [int(n) for n in re.findall(r'test result: ok\. (\d+) passed;', ''.join(output))]
    passed = sum(groups)
    if sum(count > 0 for count in groups) < minimum_groups:
        fail('Suite completed without a positive passed-test count. Add real executable tests.')
    print(f'PASS executed {passed} tests')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('suite', choices=['native', 'wasi', 'web', 'node', 'loom', 'miri', 'protocol'])
    parser.add_argument('--manifest-path', default='Cargo.toml')
    parser.add_argument('--target')
    parser.add_argument('--browser', choices=['chrome', 'firefox'])
    args = parser.parse_args()
    pin = P3_PIN if args.target == 'wasm32-wasip3' else PIN
    data = metadata(args.manifest_path, toolchain=pin)
    root = Path(data['workspace_root'])
    base = cargo(pin) + ['test', '--locked', '--manifest-path', str(Path(args.manifest_path).resolve())]
    env = os.environ.copy()
    if args.target:
        base += ['--target', args.target]
    if args.suite == 'native':
        # Run all normal suites, then every contract member independently: zero
        # platform contracts cannot be hidden by another member's passing tests.
        for features in ([], ['--all-features']):
            checked_tests(base + ['--workspace'] + features + ['--', '--test-threads=1'], cwd=root)
            for package in select(data, 'contract'):
                checked_tests(base + ['-p', package['name']] + features + ['--', '--test-threads=1'], cwd=root)
    elif args.suite == 'wasi':
        if args.target not in ('wasm32-wasip2', 'wasm32-wasip3'):
            fail('wasi requires --target wasm32-wasip2 or wasm32-wasip3')
        env['CARGO_TARGET_' + args.target.upper().replace('-', '_') + '_RUNNER'] = str(ROOT / 'scripts/ci/wasmtime-runner.sh')
        for package in select(data, 'contract'):
            features = ['--features', 'wasi-p3-experimental'] if args.target == 'wasm32-wasip3' else []
            available = {t['name'] for t in package['targets'] if 'test' in t['kind']}
            failures = []
            for key, profile in [('wasi-tests', []), ('wasi-allocation-tests', ['--release'])]:
                targets = settings(package).get(key, [])
                if not targets:
                    fail(f'{package["name"]}: declare {key}; each binary must execute real tests')
                for target in targets:
                    if target not in available:
                        fail(f'{package["name"]}: missing {key} target {target}')
                    try:
                        checked_tests(base + ['-p', package['name'], '--test', target] + features + profile
                                      + ['--', '--nocapture', '--test-threads=1'], cwd=root, env=env)
                    except (subprocess.CalledProcessError, RuntimeError) as error:
                        failures.append(str(error))
            if failures:
                fail('WASI gates failed: ' + '; '.join(failures))
    elif args.suite in ('web', 'node'):
        env['RUSTUP_TOOLCHAIN'] = PIN
        for package in select(data, 'contract'):
            key = 'web-tests' if args.suite == 'web' else 'node-tests'
            targets = settings(package).get(key, [])
            if not targets:
                fail(f'{package["name"]}: declare package.metadata.turnloop-ci.{key} (test target names)')
            available = {t['name'] for t in package['targets'] if 'test' in t['kind']}
            for target in targets:
                if target not in available:
                    fail(f'{package["name"]}: {key} target {target} absent from cargo metadata')
                browsers = ([args.browser] if args.browser else ['chrome', 'firefox']) if args.suite == 'web' else ['node']
                failures = []
                for browser in browsers:
                    command = ['wasm-pack', 'test']
                    command += ['--node'] if browser == 'node' else ['--headless', '--' + browser]
                    command += [str(Path(package['manifest_path']).parent), '--locked', '--test', target]
                    features = settings(package).get(key + '-features', [])
                    if features:
                        command += ['--features', ','.join(features)]
                    fixture_path = settings(package).get('web-fixture')
                    if not fixture_path:
                        fail(f'{package["name"]}: declare web-fixture for actual HTTP/WebSocket traffic')
                    try:
                        with WebFixture(Path(package['manifest_path']).parent / fixture_path) as fixture:
                            env['TURNLOOP_WEB_FIXTURE'] = fixture.url
                            checked_tests(command, cwd=root, env=env)
                            fixture.verify(minimum=1)
                    except (subprocess.CalledProcessError, RuntimeError) as error:
                        failures.append(browser + ': ' + str(error))
                if failures:
                    fail('Web gates failed: ' + '; '.join(failures))
    elif args.suite == 'loom':
        env['RUSTFLAGS'] = '--cfg loom'
        for package in select(data, 'core'):
            filters = settings(package).get('loom-filters', ['models'])
            if not filters:
                fail('loom-filters cannot be empty')
            for test_filter in filters:
                checked_tests(base + ['-p', package['name'], '--lib', test_filter, '--', '--test-threads=1'], cwd=root, env=env)
    elif args.suite == 'miri':
        if 'MIRI_SYSROOT' in env:
            env['MIRI_SYSROOT'] = str(Path(env['MIRI_SYSROOT']).resolve())
        found = False
        for package in select(data, 'core'):
            for test_filter in settings(package).get('miri-filters', []):
                found = True
                checked_tests(cargo(PIN) + ['miri', 'test', '--locked', '--manifest-path',
                    package['manifest_path'], '--lib', test_filter, '--', '--test-threads=1'], cwd=root, env=env)
        if not found:
            fail('Core must mark pure-Rust tests with package.metadata.turnloop-ci.miri-filters')
    else:
        env['TURNLOOP_TEST_REQUIRED'] = '1'
        for package in select(data, 'protocol'):
            targets = settings(package).get('integration-tests', [])
            if not targets:
                fail(f'{package["name"]}: integration-tests metadata must identify real-server test targets')
            available = {t['name'] for t in package['targets'] if 'test' in t['kind']}
            for target in targets:
                if target not in available:
                    fail(f'Unknown integration test target: {package["name"]}/{target}')
                checked_tests(base + ['-p', package['name'], '--test', target,
                    '--', '--include-ignored', '--test-threads=1', '--nocapture'], cwd=root, env=env)


if __name__ == '__main__':
    entrypoint(main)
