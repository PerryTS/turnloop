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
    if len(rounds) != 3:
        fail('Instruction gate requires exactly three fresh measurement rounds')
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
            if type(count) is not int or count <= 0:
                fail(f'{key}: benchmark did not execute')
            if key in controls and count != expected[key]:
                fail(f'Control {key} changed: {expected[key]} -> {count}; investigate toolchain/runner')
            if count * 100 > expected[key] * (100 + THRESHOLD_PERCENT):
                fail(f'Instruction regression {key}: {expected[key]} -> {count} exceeds {THRESHOLD_PERCENT}%')
    for key in sorted(expected):
        values = [r[key] for r in rounds]
        print(f'PASS {key}: [{min(values)}, {max(values)}] Ir; baseline {expected[key]}')


def validate_rounds(package, rounds):
    if len(rounds) != 3:
        fail('Instruction gate requires exactly three fresh measurement rounds')
    cases = settings(package).get('instruction-cases', [])
    if not cases or len(set(cases)) != len(cases) or any(set(r) != set(cases) for r in rounds):
        fail(f'{package["name"]}: every declared instruction case must execute in every round')


def candidate(package, version, rounds):
    validate_rounds(package, rounds)
    cases = settings(package)['instruction-cases']
    controls = settings(package).get('instruction-controls', [])
    baseline = {'toolchain': PIN, 'valgrind': version,
                'counts': {key: min(r[key] for r in rounds) for key in cases},
                'controls': controls}
    # The smallest observed counts are conservative. All rounds must satisfy the
    # unchanged ceiling and exact controls before a candidate can be committed.
    compare(baseline, rounds)
    return baseline


def baseline_relative(package, root):
    value = settings(package).get('instruction-baseline')
    if not value:
        fail(f'{package["name"]}: mark instruction-baseline in package.metadata.turnloop-ci')
    return (Path(package['manifest_path']).parent / value).resolve().relative_to(root.resolve())


def write_baselines(directory, root, packages, results):
    directory.mkdir(parents=True, exist_ok=True)
    # Keep raw rounds even if candidate validation fails; never invent counts.
    (directory / 'measurements.json').write_text(json.dumps(results, indent=2) + '\n')
    paths = []
    for package in packages:
        measured = results[package['name']]
        baseline = candidate(package, measured['valgrind'], measured['rounds'])
        relative = baseline_relative(package, root)
        destination = directory / relative
        if destination.resolve() == (root / relative).resolve():
            fail('Recording must not overwrite the committed baseline')
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(baseline, indent=2) + '\n')
        paths.append(relative.as_posix())
    instructions = (
        'Linux instruction baseline candidates (not a regression-gate pass).\n'
        'Review measurements.json: three fresh rounds, positive counts, exact controls.\n'
        'From the repository root, download the artifact and commit the reviewed files:\n\n'
        'gh run download <RUN_ID> --repo PerryTS/turnloop --name instruction-baselines --dir .tools/instruction-baselines\n'
    )
    for path in paths:
        instructions += f'mkdir -p {Path(path).parent.as_posix()}\ncp .tools/instruction-baselines/{path} {path}\n'
    instructions += 'git add ' + ' '.join(paths) + '\n'
    instructions += 'git commit -m "Record Linux instruction baselines"\n'
    (directory / 'README.txt').write_text(instructions)
    print(instructions, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest-path', default='Cargo.toml')
    recording = parser.add_mutually_exclusive_group()
    recording.add_argument('--record', type=Path, help='Write raw candidate rounds for review')
    recording.add_argument('--record-baselines', action='store_true', help='Record reviewable Linux baselines without failing for missing baselines')
    parser.add_argument('--artifact-dir', type=Path, default=Path('.tools/instruction-baselines'))
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
    missing = [p['name'] for p in packages if not (root / baseline_relative(p, root)).is_file()]
    bootstrap = bool(missing) or args.record_baselines
    results = {}
    for package in packages:
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
        if not args.record and not bootstrap:
            validate_rounds(package, rounds)
            baseline = json.loads((root / baseline_relative(package, root)).read_text())
            if baseline['controls'] != settings(package).get('instruction-controls'):
                fail('Baseline controls differ from declared instruction-controls')
            if baseline['toolchain'] != PIN or baseline['valgrind'] != version:
                fail('Instruction baseline toolchain/Valgrind differs; remeasure and review explicitly')
            compare(baseline, rounds)
    if args.record:
        for package in packages:
            candidate(package, version, results[package['name']]['rounds'])
        args.record.parent.mkdir(parents=True, exist_ok=True)
        args.record.write_text(json.dumps(results, indent=2) + '\n')
        print(f'Candidate measurements saved to {args.record}; this was NOT a regression-gate pass')
    elif bootstrap:
        write_baselines(args.artifact_dir, root, packages, results)
        if not args.record_baselines:
            fail(f'Missing instruction baselines for {missing}. Download artifact instruction-baselines; '
                 'follow its README.txt commands printed above to review and commit the measured JSON files.')


if __name__ == '__main__':
    entrypoint(main)
