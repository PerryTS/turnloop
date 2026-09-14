#!/usr/bin/env python3
"""Record an integration verification command and preserve its complete output."""
import pathlib, shlex, subprocess, sys, time
root = pathlib.Path(__file__).resolve().parents[1]
logs = root / '.tools/verification'
logs.mkdir(parents=True, exist_ok=True)
command = sys.argv[1:]
label = str(time.time_ns())
with (logs / (label + '.log')).open('w') as log:
    process = subprocess.Popen(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    for line in process.stdout:
        log.write(line)
        print(line, end='', flush=True)
    process.stdout.close()
    code = process.wait()
with (root / 'docs/integration-commands.md').open('a') as report:
    report.write(f"- {'PASS' if code == 0 else 'FAIL'} (exit {code}): `{shlex.join(command)}` — log `.tools/verification/{label}.log`.\n")
sys.exit(code)
