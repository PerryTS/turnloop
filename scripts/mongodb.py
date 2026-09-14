#!/usr/bin/env python3
"""Private lane servers only. start/stop/run; never uses default MongoDB ports."""
import json, os, pathlib, signal, socket, subprocess, sys, time, secrets
ROOT = pathlib.Path(__file__).resolve().parent.parent
RUN = ROOT / '.tools' / 'mongodb'
MANIFEST = RUN / 'servers.json'
PROCESSES = {}

def stop():
    if not MANIFEST.exists():
        return
    state = json.loads(MANIFEST.read_text())
    if not PROCESSES:
        subprocess.run(['cargo', 'test', '--test', 'cleanup_private', '--', '--ignored', '--nocapture'], cwd=ROOT, check=True)
    else:
        for process in PROCESSES.values():
            if process.poll() is None:
                process.terminate()
        for process in PROCESSES.values():
            process.wait(timeout=40)
    for entry in state['servers']:
        try:
            with socket.create_connection(('127.0.0.1', entry['port']), timeout=.2):
                raise RuntimeError('Private server still listening: ' + entry['name'])
        except OSError:
            pass
    MANIFEST.unlink()
    print('Private MongoDB servers stopped', flush=True)

def start():
    if MANIFEST.exists():
        raise RuntimeError('Existing private run manifest; stop it first')
    RUN.mkdir(parents=True, exist_ok=True)
    key = RUN / 'keyfile'
    key.write_text(secrets.token_urlsafe(384).replace('-', 'a').replace('_', 'b'))
    key.chmod(0o600)
    config = RUN / 'openssl.cnf'
    config.write_text('[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n[dn]\nCN=localhost\n[ext]\nsubjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')
    subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','2','-config',str(config),'-keyout',str(RUN/'key.pem'),'-out',str(RUN/'cert.pem')], check=True, capture_output=True)
    (RUN/'server.pem').write_bytes((RUN/'key.pem').read_bytes()+(RUN/'cert.pem').read_bytes())
    state = {'servers': []}
    sockets=[]
    for _ in range(5):
        s=socket.socket(); s.bind(('127.0.0.1',0)); sockets.append(s)
    try:
        for i,name in enumerate(['standalone','rs0','rs1','rs2','tls']):
            port=sockets[i].getsockname()[1];sockets[i].close()
            directory=RUN/name
            directory.mkdir(exist_ok=True)
            # New run DB path preserves earlier logs and prevents leftover users.
            db=directory/secrets.token_hex(6);db.mkdir()
            cmd=['/opt/homebrew/bin/mongod','--bind_ip','127.0.0.1','--port',str(port),'--dbpath',str(db),'--logpath',str(directory/'mongod.log'),'--logappend','--nounixsocket','--setParameter','enableTestCommands=1','--wiredTigerCacheSizeGB','0.25']
            if name.startswith('rs'):
                cmd += ['--replSet','turnloop_test','--keyFile',str(key)]
            else:
                cmd += ['--auth']
            if name=='tls':
                cmd += ['--tlsMode','requireTLS','--tlsCertificateKeyFile',str(RUN/'server.pem'),'--tlsCAFile',str(RUN/'cert.pem'),'--tlsAllowConnectionsWithoutCertificates']
            with open(directory/'process.log','ab') as log:
                p=subprocess.Popen(cmd,stdout=log,stderr=log,start_new_session=True)
            PROCESSES[name] = p
            state['servers'].append({'name':name,'port':port,'pid':p.pid})
            MANIFEST.write_text(json.dumps(state))
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                if p.poll() is not None:
                    raise RuntimeError(f'{name} exited: '+(directory/'process.log').read_text()[-2000:])
                try:
                    with socket.create_connection(('127.0.0.1',port),timeout=.2):pass
                    break
                except OSError:time.sleep(.1)
            else:raise RuntimeError(name+' did not listen')
        print('Private MongoDB ready: '+json.dumps(state),flush=True)
    except BaseException:
        stop();raise
    finally:
        for s in sockets:s.close()

def main():
    action=sys.argv[1] if len(sys.argv)>1 else 'run'
    if action=='stop':stop()
    elif action=='start':start()
    elif action=='run':
        start()
        try:
            result=subprocess.run(['cargo','test','--test','real_mongodb','--','--ignored','--nocapture','--test-threads=1'],cwd=ROOT)
        finally:stop()
        sys.exit(result.returncode)
    else:raise SystemExit('usage: mongodb.py start|stop|run')
if __name__=='__main__':main()
