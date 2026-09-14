#!/usr/bin/env python3
"""Check lane-owned source hygiene and runtime dependency graphs."""
import pathlib,re,subprocess
root=pathlib.Path(__file__).resolve().parent.parent
packages=[('wasi-p2','wasm32-wasip2'),('wasi-p3','wasm32-wasip2'),('web','wasm32-unknown-unknown'),('backend_draft','wasm32-wasip2')]
for package,target in packages:
    path=root/'spikes'/package
    for source in list((path/'src').glob('*.rs'))+list((path/'examples').glob('*.rs')):
        text=source.read_text()
        if source.name in ['lib.rs','main.rs'] or source.parent.name=='examples':
            assert '#![deny(unsafe_op_in_unsafe_fn)]' in text,source
        assert not re.search(r'\.unwrap\s*\(',text),source
    cmd=['cargo','tree','--manifest-path',str(path/'Cargo.toml'),'--target',target,'--locked','--prefix','none','--edges','normal,build']
    proc=subprocess.run(cmd,cwd=root,capture_output=True,text=True)
    assert proc.returncode==0,proc.stderr
    assert not re.search(r'^tokio(?:[- ]|$)',proc.stdout,re.M),proc.stdout
    print('PASS',package,'no unwrap(), unsafe lint declared, runtime/build dependency tree has no tokio')
assert subprocess.run(['git','diff','--quiet','--','DESIGN.md','LANES.md','rust-toolchain.toml','.cargo/config.toml'],cwd=root).returncode==0
print('PASS specification, lane rules, toolchain pin and project soak unchanged')
