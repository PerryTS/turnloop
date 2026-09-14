#!/usr/bin/env python3
"""Build cgu=1 binaries, then interleave fresh-process core/timer measurements."""
import argparse
import json
import hashlib
import pathlib
import platform
import subprocess

root = pathlib.Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--rounds', type=int, default=5)
parser.add_argument('--output', default='benchmarks/core-macos-arm64.jsonl')
args = parser.parse_args()
assert args.rounds > 0
def fingerprint():
    paths = [root / 'Cargo.toml', root / 'Cargo.lock', root / 'rust-toolchain.toml', root / '.cargo/config.toml']
    paths += list((root / 'crates').rglob('*.rs')) + list((root / 'crates').rglob('Cargo.toml'))
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()
source_fingerprint = fingerprint()
binaries = {}
for mode in ['heap', 'btree']:
    target = root / 'target' / f'bench-{mode}'
    command = ['cargo', '+nightly-2026-08-20', 'build', '--release', '-p', 'turnloop-bench', '--target-dir', str(target)]
    if mode == 'btree':
        command += ['--features', 'timer-btree']
    subprocess.run(command, cwd=root, check=True)
    binaries[mode] = target / 'release' / 'turnloop-bench'
rows = []
for round_index in range(args.rounds):
    jobs = [('core', binaries['heap'], []), ('heap', binaries['heap'], ['--timers']), ('btree', binaries['btree'], ['--timers'])]
    shift = round_index % len(jobs)
    for mode, binary, options in jobs[shift:] + jobs[:shift]:
        output = subprocess.check_output([str(binary), *options], cwd=root, text=True)
        for line in output.splitlines():
            row = json.loads(line)
            assert row['operations'] > 0 and row['total'] > 0
            row.update(round=round_index + 1, configuration=mode)
            rows.append(row)
    print(f'Completed fresh-process round {round_index + 1}/{args.rounds}', flush=True)
assert fingerprint() == source_fingerprint, "source changed during measurement"
path = root / args.output
path.parent.mkdir(parents=True, exist_ok=True)
with path.open('w') as output:
    output.write(json.dumps({'metadata': {'platform': platform.platform(), 'machine': platform.machine(), 'rounds': args.rounds, 'profile': 'release codegen-units=1', 'toolchain': 'nightly-2026-08-20', 'source_sha256': source_fingerprint}}) + '\n')
    for row in rows:
        output.write(json.dumps(row) + '\n')
for mode in ['core', 'heap', 'btree']:
    for name in dict.fromkeys(r['name'] for r in rows if r['configuration'] == mode):
        group = [r for r in rows if r['configuration'] == mode and r['name'] == name]
        assert len(group) == args.rounds
        values = [r['per_operation'] for r in group]
        print(f'{mode:5} {name:40} [{min(values):.2f}, {max(values):.2f}] {group[0]["unit"]}')
print(f'Wrote {path}')
