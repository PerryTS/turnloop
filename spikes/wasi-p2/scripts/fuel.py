#!/usr/bin/env python3
"""Minimum-success fuel thresholds. Trap at T-1, success at T; no host instruction claim."""
import json, pathlib, subprocess
root = pathlib.Path(__file__).resolve().parents[3]
wasm = root / 'spikes/wasi-p2/target/wasm32-wasip2/release/windlass-wasi-p2-spike.wasm'
def run(mode, n, fuel):
    p = subprocess.run(['wasmtime','run','-W',f'fuel={fuel}',str(wasm),mode,str(n)], capture_output=True, text=True)
    if p.returncode == 0:
        assert f'iterations={n}' in p.stdout
        return True
    assert 'fuel' in p.stderr and 'consumed' in p.stderr, p.stderr
    return False
def threshold(mode, n):
    lo, hi = 0, 1000000
    while not run(mode,n,hi): hi *= 2
    while hi-lo > 1:
        mid = (lo+hi)//2
        if run(mode,n,mid): hi = mid
        else: lo = mid
    assert not run(mode,n,hi-1) and run(mode,n,hi)
    return hi
rows=[]
for round_id in range(3):
    for mode in ['control','idle','poll','timer-cancel']:
        a,b=threshold(mode,100),threshold(mode,200)
        row=dict(round=round_id,mode=mode,fuel_100=a,fuel_200=b,slope=(b-a)/100)
        rows.append(row); print(json.dumps(row),flush=True)
path=root/'spikes/wasi-p2/results/fuel.json'
path.write_text(json.dumps(rows,indent=2)+'\n')
