#!/usr/bin/env python3
"""Run a verification command and retain its exact argv, output and exit status."""
import datetime, json, pathlib, subprocess, sys
root = pathlib.Path(__file__).resolve().parent.parent
ledger = root / 'spikes' / 'verification.jsonl'
args = sys.argv[1:]
proc = subprocess.run(args, cwd=root, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
entry = dict(time=datetime.datetime.now(datetime.timezone.utc).isoformat(), command=args, result='PASS' if proc.returncode == 0 else 'FAIL', exit=proc.returncode, output=proc.stdout)
with ledger.open('a') as f:
    f.write(json.dumps(entry) + '\n')
print(proc.stdout, end='')
print(entry['result'], ' '.join(args))
sys.exit(proc.returncode)
