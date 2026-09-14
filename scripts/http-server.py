"""Start/stop only our token-authenticated private HTTP test instance; no process scanning."""
from pathlib import Path
import json, os, secrets, socket, subprocess, sys, time
root=Path(__file__).resolve().parent.parent
os.chdir(root)
state_path=root/'.tools/http-server.json'
root.joinpath('.tools').mkdir(exist_ok=True)
action=sys.argv[1] if len(sys.argv)>1 else ''
if action=='start':
    if state_path.exists():raise SystemExit('Private server state already exists; stop it first')
    mode=sys.argv[2] if len(sys.argv)>2 else 'h1'
    if mode not in ['h1','h2']:raise SystemExit('mode must be h1 or h2')
    token=secrets.token_hex(24)
    env=os.environ.copy();env['TURNLOOP_HTTP_TEST_TOKEN']=token
    with open('.tools/http-server.log','w')as log:
        child=subprocess.Popen(['node','scripts/private-http-server.mjs',mode],env=env,stdout=log,stderr=log,start_new_session=True)
    try:
        deadline=time.monotonic()+5
        while True:
            line=Path('.tools/http-server.log').read_text().splitlines()
            if line and line[0].isdigit():port=int(line[0]);break
            if child.poll() is not None or time.monotonic()>deadline:raise RuntimeError('server startup failed; see .tools/http-server.log')
            time.sleep(.02)
        state_path.write_text(json.dumps(dict(port=port,mode=mode,token=token)))
        print(f'Private {mode} server on 127.0.0.1:{port}')
    except BaseException:
        child.terminate();child.wait();raise
elif action=='stop':
    if not state_path.exists():raise SystemExit(0)
    state=json.loads(state_path.read_text());port=state['port'];assert 1024<port<65536
    command=['curl','--silent','--show-error','--max-time','5','--noproxy','*','-X','POST','-H','x-turnloop-test-token: '+state['token']]
    if state['mode']=='h2':command+=['--http2-prior-knowledge']
    result=subprocess.run(command+[f'http://127.0.0.1:{port}/__turnloop_shutdown'],capture_output=True,text=True,check=True)
    if result.stdout!='stopping':raise SystemExit('Server identity was not verified; no process was killed')
    deadline=time.monotonic()+5
    while True:
        try:
            with socket.create_connection(('127.0.0.1',port),timeout=.1):pass
        except OSError:break
        if time.monotonic()>deadline:raise SystemExit('Server did not close listener')
        time.sleep(.02)
    state_path.unlink();print('Private server stopped')
else:raise SystemExit('usage: scripts/http-server.sh start [h1|h2] | stop')
