#!/usr/bin/env python3
"""Build the pinned test tool under the project soak; download ChromeDriver locally."""
import json,os,pathlib,shutil,subprocess,urllib.request,zipfile
root=pathlib.Path(__file__).resolve().parents[3]
version='0.2.108'
subprocess.run(['cargo','info','wasm-bindgen-cli@'+version],cwd=root,check=True)
cargo=pathlib.Path(os.environ.get('CARGO_HOME',str(pathlib.Path.home()/'.cargo')))
source=next((cargo/'registry/src').glob('*/wasm-bindgen-cli-'+version))
local=root/'.tools/wasm-bindgen-source'
if not local.exists():
    shutil.copytree(source,local)
    lock=local/'Cargo.lock'
    if lock.exists():lock.unlink()
    manifest=local/'Cargo.toml';manifest.write_text(manifest.read_text()+'\n[workspace]\n')
    subprocess.run(['cargo','generate-lockfile','--manifest-path',str(manifest)],cwd=root,check=True)
subprocess.run(['cargo','build','--release','--locked','--manifest-path',str(local/'Cargo.toml')],cwd=root,check=True)
# Pinned to the installed/tested Chrome major in this environment. Choose another
# official Chrome-for-Testing version explicitly if the local Chrome major changes.
driver_version='153.0.8010.36'
base=root/('.tools/wasm-pack-cache/chromedriver-'+driver_version)
binary=base/'chromedriver'
if not binary.exists():
    base.mkdir(parents=True,exist_ok=True)
    url=f'https://storage.googleapis.com/chrome-for-testing-public/{driver_version}/mac-arm64/chromedriver-mac-arm64.zip'
    archive=base/'archive.zip'
    with urllib.request.urlopen(url) as response:archive.write_bytes(response.read())
    with zipfile.ZipFile(archive) as z:
        binary.write_bytes(z.read('chromedriver-mac-arm64/chromedriver'))
    binary.chmod(0o755)
print('Tool and ChromeDriver ready inside .tools')
