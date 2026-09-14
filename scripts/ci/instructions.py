#!/usr/bin/env python3
"""Run fresh gungraun processes and compare nonzero Ir counts to reviewed baselines."""
import argparse
import json
import os
from pathlib import Path
import platform
import tempfile
from common import PIN, cargo, entrypoint, fail, metadata, run, select, settings

THRESHOLD_PERCENT = 3


def read_counts(directory):
    counts = {}
    for path in directory.rglob('summary.json'):
        record = json.loads(path.read_text())
        if record['version'] != '6':
            fail(f'Unsupported iai summary schema {record["version"]}: {path}')
        key = record['module_path'] + ('/' + record['id'] if record['id'] is not None else '')
        profiles = [p for p in record['profiles'] if p['tool'] == 'Callgrind']
        if len(profiles) != 1 or key in counts:
            fail(f'Ambiguous Callgrind measurements for {key}')
        metric = profiles[0]['summaries']['total']['summary']['Callgrind']['Ir']['metrics']
        # Each invocation has a fresh output directory. Old/cached counts cannot pass.
        if set(metric) != {'Left'}:
            fail(f'{key}: expected fresh counts, got {metric}')
        count = metric['Left']['Int']
        if type(count) is not int or count <= 0:
            fail(f'{key}: zero/invalid instruction count')
        counts[key] = count
    if not counts:
        fail('No gungraun summary.json files; benchmark did not run')
    return counts


def compare(baseline, rounds):
    expected = baseline['counts']
    controls = baseline['controls']
    if not expected or not controls or not set(controls) < set(expected):
        fail('Baseline needs both control and workload cases')
    for key, count in expected.items():
        if type(count) is not int or count <= 0:
            fail(f'Invalid baseline for {key}')
    for measured in rounds:
        if set(expected) != set(measured):
            fail(f'Benchmark set changed: missing={set(expected)-set(measured)}, new={set(measured)-set(expected)}')
        for key, count in measured.items():
            if count <= 0:
                fail(f'{key}: benchmark did not execute')
            if key in controls and count != expected[key]:
                fail(f'Control {key} changed: {expected[key]} -> {count}; investigate toolchain/runner')
            if count * 100 > expected[key] * (100 + THRESHOLD_PERCENT):
                fail(f'Instruction regression {key}: {expected[key]} -> {count} exceeds {THRESHOLD_PERCENT}%')
    for key in sorted(expected):
        values = [r[key] for r in rounds]
        print(f'PASS {key}: [{min(values)}, {max(values)}] Ir; baseline {expected[key]}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', default='Cargo.toml')
    parser.add_argument('--record', type=Path, help='Write candidate counts for review; never updates committed baselines')
    args = parser.parse_args()
    data = metadata(args.manifest_path)
    packages = select(data, 'bench')
    for package in packages:
        targets = [t for t in package['targets'] if 'bench' in t['kind']]
        if not targets or not any(d['name'] == 'gungraun' and d['req'] == '=0.19.4' for d in package['dependencies']):
            fail(f'{package["name"]}: add a gungraun =0.19.4 benchmark target; timings/perf counters cannot satisfy this gate')
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        fail('Instruction gate requires the ubuntu-24.04 x86_64/Valgrind runner')
    version = run(['valgrind', '--version'], capture=True).strip()
    env = os.environ.copy()
    env.update({'CARGO_PROFILE_BENCH_CODEGEN_UNITS': '1', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '1',
                'RUSTFLAGS': '-C codegen-units=1', 'GUNGRAUN_SAVE_SUMMARY': 'json',
                'GUNGRAUN_NOCAPTURE': 'true'})
    root = Path(data['workspace_root'])
    results = {}
    for package in packages:
        baseline_path = settings(package).get('instruction-baseline')
        if not baseline_path and not args.record:
            fail(f'{package["name"]}: mark a reviewed instruction-baseline in package.metadata.turnloop-ci')
        rounds = []
        for _ in range(3):
            # No cache restore: summaries from another commit can never satisfy execution.
            with tempfile.TemporaryDirectory(prefix='turnloop-iai-') as directory:
                env['GUNGRAUN_HOME'] = directory
                command = cargo(PIN) + ['bench', '--locked', '--manifest-path', package['manifest_path']]
                for target in package['targets']:
                    if 'bench' in target['kind']:
                        command += ['--bench', target['name']]
                run(command, cwd=root, env=env)
                rounds.append(read_counts(Path(directory)))
        results[package['name']] = {'toolchain': PIN, 'valgrind': version, 'rounds': rounds}
        if not args.record:
            baseline = json.loads((Path(package['manifest_path']).parent / baseline_path).read_text())
            if baseline['toolchain'] != PIN or baseline['valgrind'] != version:
                fail('Instruction baseline toolchain/Valgrind differs; remeasure and review explicitly')
            compare(baseline, rounds)
    if args.record:
        args.record.write_text(json.dumps(results, indent=2) + '\n')
        print(f'Candidate measurements saved to {args.record}; this was NOT a regression-gate pass')


if __name__ == '__main__':
    entrypoint(main)
