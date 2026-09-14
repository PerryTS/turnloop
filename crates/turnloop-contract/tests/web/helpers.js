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
