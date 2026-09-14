#!/usr/bin/env python3
"""Audit all resolved target graphs, including dev dependencies, without hiding cargo errors."""
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
for target in ["aarch64-apple-darwin", "wasm32-wasip2", "wasm32-unknown-unknown", "all"]:
    result = subprocess.run(["cargo", "tree", "--locked", "--target", target,
                             "--prefix", "none", "--format", "{p}"],
                            cwd=root, capture_output=True, text=True, check=True)
    names = {line.split()[0] for line in result.stdout.splitlines() if line.strip()}
    forbidden = names & {"tokio", "tokio-util", "hyper", "h2", "http-body", "http-body-util",
                         "async-std", "smol", "async-io", "async-global-executor",
                         "futures-executor", "futures-runtime"}
    assert not forbidden, (target, forbidden)
    for crate in ["tokio", "hyper"]:
        inverse = subprocess.run(["cargo", "tree", "--locked", "--target", target, "-i", crate],
                                 cwd=root, capture_output=True, text=True)
        assert not inverse.stdout, (target, crate, inverse.stdout)
        assert inverse.returncode == 101 and "did not match any packages" in inverse.stderr, inverse.stderr
    print(f"PASS {target}: {len(names)} packages, no forbidden dependencies; inverse tokio/hyper stdout empty")
