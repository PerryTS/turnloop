from pathlib import Path
import subprocess,hashlib
binary=Path('target/debug/examples/h2spec_server').resolve()
print('h2spec binary sha256',hashlib.sha256(Path('.tools/h2spec').read_bytes()).hexdigest(),flush=True)
log=Path('.tools/h2spec-server.log').open('w')
server=subprocess.Popen([str(binary)],stdout=subprocess.PIPE,stderr=log,text=True)
try:
 port=server.stdout.readline().strip();print('port',port,flush=True)
 result=subprocess.run(['.tools/h2spec','-h','127.0.0.1','-p',port,'-o','1','--strict','-j','.tools/h2spec.xml'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=240)
 Path('.tools/h2spec.log').write_text(result.stdout)
 print(result.stdout[-15000:]);print('exit',result.returncode)
 assert '147 tests' in result.stdout, 'h2spec did not run the expected suite'
 assert result.returncode == 0, 'h2spec failures; see .tools/h2spec.log'
finally:
 server.terminate();server.wait(timeout=10);log.close()
