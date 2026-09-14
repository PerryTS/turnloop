#!/usr/bin/env python3
"""Drive the cross-origin-isolated Worker contract using a local ChromeDriver."""
import json,pathlib,subprocess,time,urllib.request,urllib.error
root=pathlib.Path(__file__).resolve().parents[3]
base='http://127.0.0.1:9516'
def request(path,data=None,method=None):
    req=urllib.request.Request(base+path,data=None if data is None else json.dumps(data).encode(),headers={'Content-Type':'application/json'},method=method)
    try:return json.load(urllib.request.urlopen(req,timeout=20))['value']
    except urllib.error.HTTPError as e:raise RuntimeError(e.read().decode()) from e
fixture=subprocess.Popen(['node',str(root/'spikes/web/tests/fixture.mjs')],stdout=subprocess.PIPE,text=True)
driver=None;session=None
try:
    assert 'fixture ready' in fixture.stdout.readline()
    binary=next((root/'.tools/wasm-pack-cache').glob('chromedriver-*/chromedriver'))
    driver=subprocess.Popen([str(binary),'--port=9516'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    for _ in range(100):
        try:request('/status');break
        except urllib.error.URLError:time.sleep(.05)
    value=request('/session',{'capabilities':{'alwaysMatch':{'browserName':'chrome','goog:chromeOptions':{'args':['--headless=new','--no-first-run','--disable-crash-reporter','--user-data-dir='+str(root/'.tools/chrome-worker-profile')]}}}})
    session=value['sessionId']
    request('/session/'+session+'/url',{'url':'http://127.0.0.1:18765/isolated.html'})
    for _ in range(150):
        result=request('/session/'+session+'/execute/sync',{'script':'return document.getElementById("result")?.textContent','args':[]})
        if result and result!='RUNNING':break
        time.sleep(.1)
    assert result.startswith('PASS '),result
    print('Chrome isolated Worker',result)
finally:
    if session:request('/session/'+session,method='DELETE')
    if driver:driver.terminate();driver.wait(timeout=10)
    fixture.terminate();fixture.wait(timeout=10)
