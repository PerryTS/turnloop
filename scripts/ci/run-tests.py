#!/usr/bin/env python3
"""Metadata-driven test entry points; missing suites and zero passed tests are errors."""
import argparse
from contextlib import nullcontext
from browser_driver import BrowserDriver
import os
from pathlib import Path
import re
import subprocess
import sys
from feature_modes import native_modes
from web_fixture import WebFixture
from common import PIN, P3_PIN, ROOT, cargo, entrypoint, fail, members, metadata, role, run, select, settings


def checked_tests(command, *, cwd, env=None, minimum_groups=1, input_text=None):
    # Preserve the output and exit code; never turn ignored/filtered tests into passes.
    print('+ ' + ' '.join(map(str, command)), flush=True)
    process = subprocess.Popen(command, cwd=cwd, env=env, text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               stdin=subprocess.PIPE if input_text is not None else None)
    if input_text is not None:
        try:
            process.stdin.write(input_text)
        except BrokenPipeError:
            pass # The exit code and positive subject counts below remain mandatory.
        finally:
            process.stdin.close()
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
    return passed


def member_features(package, features):
    """Cargo accepts package/feature only for this member or a direct dependency.

    Sans-IO members without a turnloop dependency have no backend mode. Keep the
    complete selection on the workspace run; project it for independent positive
    count checks so an unrelated member cannot mask an empty suite.
    """
    if not features or features == ['--all-features']:
        return list(features)
    available = {package['name']}
    available.update(d.get('rename') or d['name'] for d in package['dependencies'])
    selected = [f for f in features[1].split(',') if f.split('/')[0] in available]
    return ['--features', ','.join(selected)] if selected else []


def protocol_tests(packages, base, root, env, features=()):
    """Finish every declared suite, then fail the job if any one failed."""
    results = []
    for package in packages:
        name = package['name']
        targets = settings(package).get('integration-tests', [])
        if not targets:
            results.append((name, 'FAIL', 'Missing integration-tests metadata'))
            continue
        available = {t['name'] for t in package['targets'] if 'test' in t['kind']}
        for target in targets:
            suite = f'{name}/{target}'
            try:
                if target not in available:
                    fail(f'Unknown integration test target: {suite}')
                required = next(t.get('required-features', []) for t in package['targets']
                                if t['name'] == target and 'test' in t['kind'])
                target_features = ['--features', ','.join(required)] if required else []
                count = checked_tests(base + member_features(package, features) + target_features + ['-p', name, '--test', target,
                    '--', '--include-ignored', '--test-threads=1', '--nocapture'], cwd=root, env=env)
            except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
                print(f'FAIL {suite}: {error}', file=sys.stderr, flush=True)
                results.append((suite, 'FAIL', str(error)))
            else:
                results.append((suite, 'PASS', f'{count} tests passed'))
    if not results:
        results.append(('protocol', 'FAIL', 'No executable suites selected'))
    def cell(value):
        return value.replace('|', '&#124;').replace('\n', ' ').replace('\r', ' ')
    table = '\n'.join(['## Protocol suites', '', '| Suite | Result | Details |',
                       '| --- | --- | --- |'] +
                      ['| ' + ' | '.join(cell(value) for value in row) + ' |' for row in results]) + '\n'
    print(table, flush=True)
    if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(summary, 'a', encoding='utf-8') as output:
            output.write(table + '\n')
    if any(status == 'FAIL' for _, status, _ in results):
        fail('Protocol suites failed; see the per-suite results above')
    return results


def native_tests(data, base, root, *, windows, modes):
    contracts = select(data, 'contract')
    pending = []
    for package in contracts:
        handoff = settings(package).get('windows-contracts-pending')
        if windows and handoff:
            if handoff != 'WINDOWS_HANDOFF.md' or not (root / handoff).is_file():
                fail(f'{package["name"]}: pending Windows contracts need WINDOWS_HANDOFF.md')
            pending.append(package['name'])
            message = (f'PENDING Windows backend contracts: {package["name"]}. '
                       'IOCP bounded waits/no-spin, socket I/O, cancel/close ordering, '
                       'wake/integration, allocation and lifetime tests await the production provider. '
                       'See [WINDOWS_HANDOFF.md](https://github.com/PerryTS/turnloop/blob/main/WINDOWS_HANDOFF.md), phase 2. '
                       'Remove windows-contracts-pending metadata when IOCP lands.\n')
            print(message, flush=True)
            if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
                with open(summary, 'a', encoding='utf-8') as output:
                    output.write(message + '\n')
    for mode, features in modes:
        print(f'Native mode: {mode} ({" ".join(features) or "default features"})', flush=True)
        checked_tests(base + ['--workspace'] + features + ['--', '--test-threads=1'], cwd=root)
        # Every portable member must execute independently; another crate's tests
        # cannot hide a cfg-excluded core, protocol codec or fixture suite.
        for package in select(data, 'core') + select(data, 'protocol') + [p for p in members(data) if role(p) == 'adapter'] + contracts:
            if package['name'] not in pending:
                checked_tests(base + ['-p', package['name']] + member_features(package, features) + ['--', '--test-threads=1'], cwd=root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('suite', choices=['native', 'wasi', 'web', 'node', 'loom', 'miri', 'protocol', 'protocol-wasi', 'interop'])
    parser.add_argument('--manifest-path', default='Cargo.toml')
    parser.add_argument('--target')
    parser.add_argument('--real-servers', action='store_true', help='Require WASI real-server adapter suites using the unified fixture environment')
    parser.add_argument('--package', action='append', dest='packages', help='Select protocol members for a local fixture run; CI omits this filter')
    parser.add_argument('--mode', help='One applicable native CI matrix mode; omitted runs all applicable modes')
    parser.add_argument('--browser', choices=['chrome', 'firefox'])
    args = parser.parse_args()
    pin = P3_PIN if args.target == 'wasm32-wasip3' else PIN
    data = metadata(args.manifest_path, toolchain=pin)
    root = Path(data['workspace_root'])
    base = cargo(pin) + ['test', '--locked', '--manifest-path', str(Path(args.manifest_path).resolve())]
    env = os.environ.copy()
    if args.packages:
        known = {p['name'] for p in members(data)}
        if set(args.packages) - known:
            fail('Unknown selected protocol package')
    if args.real_servers and (args.suite != 'protocol-wasi' or args.target != 'wasm32-wasip2'):
        fail('--real-servers applies to protocol-wasi on WASI 0.2')
    if args.target:
        base += ['--target', args.target]
    if args.mode and args.suite not in ('native', 'interop'):
        fail('--mode applies only to native and interop suites')
    platform = sys.platform
    if args.target:
        platform = ('win32' if 'windows' in args.target else
                    'darwin' if 'apple' in args.target else
                    'linux' if 'linux' in args.target else args.target)
    if args.suite == 'native':
        native_tests(data, base, root, windows=platform == 'win32',
                     modes=native_modes(platform, args.mode))
    elif args.suite in ('wasi', 'protocol-wasi'):
        if args.target not in ('wasm32-wasip2', 'wasm32-wasip3'):
            fail('wasi requires --target wasm32-wasip2 or wasm32-wasip3')
        env['CARGO_TARGET_' + args.target.upper().replace('-', '_') + '_RUNNER'] = str(ROOT / 'scripts/ci/wasmtime-runner.sh')
        if args.suite == 'protocol-wasi':
            selected = [(p, settings(p).get('wasi-tests', [])) for p in members(data)
                        if role(p) in ('protocol', 'codec', 'adapter') and settings(p).get('wasi-tests')
                        and (not args.packages or p['name'] in args.packages)]
            if not selected:
                fail('No wasi-tests metadata: protocol runtime coverage is required')
            for package, targets in selected:
                if args.real_servers:
                    env['TURNLOOP_TEST_REQUIRED'] = '1'
                    for setup in settings(package).get('wasi-setup-tests', []):
                        checked_tests(cargo(PIN) + ['test', '--locked', '-p', package['name'], '--features', 'turnloop', '--test', setup,
                            '--', '--include-ignored', '--test-threads=1', '--nocapture'], cwd=root, env=env)
                    targets = targets + settings(package).get('wasi-integration-tests', [])
                available = {t['name'] for t in package['targets'] if 'test' in t['kind']}
                for target in targets:
                    if target not in available:
                        fail(f'Unknown WASI test target: {package["name"]}/{target}')
                    required = next(t.get('required-features', []) for t in package['targets']
                                    if t['name'] == target and 'test' in t['kind'])
                    target_features = ['--features', ','.join(required)] if required else []
                    if args.target == 'wasm32-wasip3' and (required or role(package) == 'adapter'):
                        target_features = ['--all-features']
                    checked_tests(base + ['-p', package['name'], '--test', target] + target_features +
                        ['--', '--test-threads=1'] + (['--include-ignored', '--nocapture'] if target in settings(package).get('wasi-integration-tests', []) else []), cwd=root, env=env)
        else:
            for package in select(data, 'core'):
                if settings(package).get('wasi-lib-tests'):
                    features = ['--all-features']
                    checked_tests(base + ['-p', package['name'], '--lib', '--release'] + features
                                  + ['--', '--nocapture', '--test-threads=1'], cwd=root, env=env)
            for package in select(data, 'contract'):
                features = ['--all-features']
                available = {t['name'] for t in package['targets'] if 'test' in t['kind']}
                failures = []
                for key, profile in [('wasi-tests', []), ('wasi-tests', ['--release']),
                                     ('wasi-allocation-tests', ['--release'])]:
                    targets = settings(package).get(key, [])
                    if not targets:
                        fail(f'{package["name"]}: declare {key}; each binary must execute real tests')
                    for target in targets:
                        if target not in available:
                            fail(f'{package["name"]}: missing {key} target {target}')
                        try:
                            checked_tests(base + ['-p', package['name'], '--test', target] + features + profile
                                          + ['--', '--nocapture', '--test-threads=1'], cwd=root, env=env,
                                          input_text='turnloop revision two stdin fixture\n')
                        except (subprocess.CalledProcessError, RuntimeError, OSError) as error:
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
                    command = ['wasm-pack', 'test', '--mode', 'no-install']
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
                            manager = nullcontext(None) if browser == 'node' else BrowserDriver(browser, env)
                            with manager as driver:
                                launch = command if driver is None else command[:4] + driver.driver_args + command[4:]
                                checked_tests(launch, cwd=root, env=env if driver is None else driver.env)
                                fixture.verify(minimum=1)
                    except (subprocess.CalledProcessError, RuntimeError, OSError) as error:
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
        packages = [p for p in select(data, 'protocol') if not args.packages or p['name'] in args.packages]
        if args.suite == 'interop':
            packages = [p for p in packages if settings(p).get('service-group') == 'http']
            if not packages:
                fail('HTTP interop group must contain executable suites')
        modes = native_modes(platform, args.mode) if args.suite == 'interop' and args.mode else [('default', [])]
        for mode, features in modes:
            print(f'Protocol mode: {mode}', flush=True)
            protocol_tests(packages, base, root, env, features)


if __name__ == '__main__':
    entrypoint(main)
