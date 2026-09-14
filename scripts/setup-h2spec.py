import urllib.request,json,tarfile,io,hashlib
from pathlib import Path
commit='70ac2294010887f48b18e2d64f5cccd48421fad1'
tree=json.load(urllib.request.urlopen(f'https://api.github.com/repos/summerwind/h2spec/git/trees/{commit}?recursive=1'))
expected={a['path']:a['sha'] for a in tree['tree'] if a['type']=='blob'}
archive=urllib.request.urlopen(f'https://codeload.github.com/summerwind/h2spec/tar.gz/{commit}').read()
root=Path('.tools/h2spec-source');root.mkdir(exist_ok=True)
with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz')as tar:
 for member in tar.getmembers():
  if member.isdir():continue
  assert member.isfile()
  relative=member.name.split('/',1)[1];assert '..' not in Path(relative).parts
  raw=tar.extractfile(member).read();assert hashlib.sha1(b'blob '+str(len(raw)).encode()+b'\0'+raw).hexdigest()==expected[relative],relative
  path=root/relative;path.parent.mkdir(exist_ok=True,parents=True);path.write_bytes(raw)
Path('.tools/h2spec-source-provenance.txt').write_text(f'commit {commit}\narchive sha256 {hashlib.sha256(archive).hexdigest()}\nevery blob verified against commit tree\n')
print(root)

import os,subprocess
root=Path.cwd()
env=os.environ.copy()
for key,path in [('GOMODCACHE','gomodcache'),('GOCACHE','gocache'),('GOPATH','gopath')]:env[key]=str(root/'.tools'/path)
env['GOTOOLCHAIN']='local'
args=['go','-C','.tools/h2spec-source','build']
if os.uname().sysname=='Darwin':args+=['-ldflags=-linkmode=external']
subprocess.run(args+['-o','../h2spec','./cmd/h2spec'],env=env,check=True)
if os.uname().sysname=='Darwin':subprocess.run(['codesign','--force','--sign','-','.tools/h2spec'],check=True)
print('built h2spec SHA-256',hashlib.sha256(Path('.tools/h2spec').read_bytes()).hexdigest())
