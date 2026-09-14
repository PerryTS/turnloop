export function guard(promise) {
  let timer;
  return Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('host never scheduled a turn')),5000);})]).finally(()=>clearTimeout(timer));
}
export function sleep(ms) {return new Promise(resolve=>setTimeout(resolve,ms));}
export async function stats(base) {return await (await fetch(base+'/stats')).json();}
export function producers(descriptor,count) {
  const source=`const SharedPoster=${descriptor.producerSource};
    const run=({buffer,capacity,start,count})=>{
      const poster=new SharedPoster(buffer,capacity);let i=0;
      const batch=()=>{let budget=128;while(i<count && budget--){const n=BigInt(start+i);if(!poster.post(0xf123456700000000n+n,n^0xffffffffffffffffn))break;i++;}
        if(i<count)setTimeout(batch,0);else done();};batch();};`;
  const workers=[];
  const done=Promise.all([0,count].map(start=>new Promise(async(resolve,reject)=>{
    try {
      const data={buffer:descriptor.buffer,capacity:descriptor.capacity,start,count};
      let worker;
      if(typeof process!=='undefined' && process.versions?.node) {
        const {Worker}=await import('node:worker_threads');
        worker=new Worker(source+`const {parentPort,workerData}=require('node:worker_threads');const done=()=>parentPort.postMessage('done');run(workerData);`,{eval:true,workerData:data});
        worker.once('message',resolve);worker.once('error',reject);
      } else {
        const url=URL.createObjectURL(new Blob([source+`const done=()=>postMessage('done');onmessage=e=>run(e.data);`],{type:'text/javascript'}));
        worker=new Worker(url);URL.revokeObjectURL(url);
        worker.onmessage=resolve;worker.onerror=reject;worker.postMessage(data);
      }
      workers.push(worker);
    } catch(e){reject(e);}
  })));
  return {done:guard(done),stop:()=>{for(const w of workers)w.terminate();}};
}
export function stopProducers(group) {group.stop();}
export function producersDone(group) {return group.done;}
export function fillRing(descriptor) {
  const Poster=Function('return '+descriptor.producerSource)();
  const p=new Poster(descriptor.buffer,descriptor.capacity);
  for(let i=0;i<descriptor.capacity;i++)if(!p.post(BigInt(i),BigInt(i)))throw new Error('early full');
  if(p.post(99n,99n))throw new Error('overflow accepted');
}

export async function conditionWorker(descriptor) {
  const source=`const Condition=${descriptor.producerSource};let condition;
    const receive=data=>{
      if(data.buffer){condition=new Condition(data.buffer,data.capacity);reply('ready');return;}
      const ok=data.store?condition.store(BigInt(data.value)):condition.notify();
      if(!ok)throw new Error('condition queue rejected sequential update');reply('accepted');};`;
  let worker, resolve, reject;
  const response=()=>guard(new Promise((a,b)=>{resolve=a;reject=b;}));
  const ready=response();
  if(typeof process!=='undefined' && process.versions?.node) {
    const {Worker}=await import('node:worker_threads');
    worker=new Worker(source+`const {parentPort}=require('node:worker_threads');const reply=v=>parentPort.postMessage(v);parentPort.on('message',receive);`,{eval:true});
    worker.on('message',value=>resolve(value));worker.on('error',e=>reject(e));
  } else {
    const url=URL.createObjectURL(new Blob([source+`const reply=v=>postMessage(v);onmessage=e=>receive(e.data);`],{type:'text/javascript'}));
    worker=new Worker(url);URL.revokeObjectURL(url);
    worker.onmessage=e=>resolve(e.data);worker.onerror=e=>reject(e);
  }
  worker.postMessage({buffer:descriptor.buffer,capacity:descriptor.capacity});
  await ready;
  return {send:(store,value)=>{const p=response();worker.postMessage({store,value});return p;},stop:()=>worker.terminate()};
}
export function updateCondition(worker,store,value) { return worker.send(store,value); }
export function stopCondition(worker) { worker.stop(); }
export function conditionBackpressure(descriptor) {
  const Condition=Function('return '+descriptor.producerSource)();
  const c=new Condition(descriptor.buffer,descriptor.capacity);
  for(let i=0;i<descriptor.capacity;i++)if(!c.notify())throw new Error('early condition full');
  if(c.notify())throw new Error('condition overflow accepted');
}
export function conditionClosed(descriptor) {
  const Condition=Function('return '+descriptor.producerSource)();
  if(new Condition(descriptor.buffer,descriptor.capacity).notify())throw new Error('closed condition accepted');
}
