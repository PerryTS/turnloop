#!/usr/bin/env python3
"""Own the local fixture lifecycle; leave no persistent service running."""
import os,pathlib,subprocess,sys,urllib.request,json,time
root=pathlib.Path(__file__).resolve().parents[3]
mode=sys.argv[1]
driver=None
fixture=subprocess.Popen(['node',str(root/'spikes/web/tests/fixture.mjs')],cwd=root,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
try:
    assert 'fixture ready' in fixture.stdout.readline(), 'fixture failed to start'
    env=os.environ.copy();env['WASM_PACK_CACHE']=str(root/'.tools/wasm-pack-cache')
    env['WASM_BINDGEN_TEST_TIMEOUT']='30'
    env['CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE']='7 days'
    env['CARGO_UNSTABLE_MIN_PUBLISH_AGE']='true'
    env['PATH']=str(root/'.tools/wasm-bindgen-source/target/release')+os.pathsep+env['PATH']
    cmd=['wasm-pack','test','--mode','no-install','--headless','--chrome','spikes/web','--features','browser'] if mode=='chrome' else ['wasm-pack','test','--mode','no-install','--node','spikes/web']
    if mode=='chrome':
        binary=next((root/'.tools/wasm-pack-cache').glob('chromedriver-*/chromedriver'))
        driver=subprocess.Popen([str(binary),'--port=9517'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:urllib.request.urlopen('http://127.0.0.1:9517/status',timeout=1);break
            except urllib.error.URLError:time.sleep(.05)
        env['CHROMEDRIVER_REMOTE']='http://127.0.0.1:9517'
        env['CHROMEDRIVER']=str(binary)
        config=root/'.tools/webdriver.json'
        config.write_text(json.dumps({'goog:chromeOptions':{'args':['--no-first-run','--disable-crash-reporter','--user-data-dir='+str(root/'.tools/chrome-contract-profile')]}}))
        env['WASM_BINDGEN_TEST_WEBDRIVER_JSON']=str(config)
    cmd += ['--', '--nocapture']
    print('command:', ' '.join(cmd), flush=True)
    proc=subprocess.run(cmd,cwd=root,env=env,timeout=300)
    if proc.returncode:sys.exit(proc.returncode)
    stats=json.load(urllib.request.urlopen('http://127.0.0.1:18765/stats'))
    assert stats['fetches']>=2 and stats['aborted']>=1 and stats['websockets']>=1 and stats['echoed']>=257,stats
    print('fixture subjects PASS',stats)
finally:
    if driver:driver.terminate();driver.wait(timeout=10)
    fixture.terminate();fixture.wait(timeout=10)
