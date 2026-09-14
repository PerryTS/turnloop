import assert from 'node:assert/strict';
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { Poster } from '../worker-poster.js';
if(isMainThread){
  const poster=new Poster();
  assert(poster.post(0xffffffffffffffffn,123n));assert.equal(Atomics.load(poster.words,5),0,'notify while running');
  assert.deepEqual(poster.take(),[0xffffffffffffffffn,123n]);
  // Exercise full queue and backpressure without overwriting unread entries.
  for(let i=0;i<256;i++)assert(poster.post(BigInt(i),BigInt(i+1)));
  assert(!poster.post(999n,999n));
  for(let i=0;i<256;i++)assert.deepEqual(poster.take(),[BigInt(i),BigInt(i+1)]);
  const all=[];
  const consumer=new Worker(new URL(import.meta.url),{workerData:{kind:'consumer',buffer:poster.words.buffer}});all.push(consumer);
  const timeout=setTimeout(()=>{for(const w of all)w.terminate();throw new Error('worker timeout');},15000);
  try {
    const result=new Promise((resolve,reject)=>{consumer.on('error',reject);consumer.on('message',m=>{if(m.done)resolve(m);});});
    await new Promise(resolve=>consumer.once('message',resolve));
    // Wait for its actual PARKED store, not merely the message preceding it.
    while(Atomics.load(poster.words,4)!==1)await new Promise(r=>setImmediate(r));
    for(let producer=0;producer<2;producer++){
      const worker=new Worker(new URL(import.meta.url),{workerData:{kind:'producer',producer,buffer:poster.words.buffer}});all.push(worker);
    }
    const done=await result;assert.equal(done.count,2000);assert.equal(done.unique,2000);assert(Atomics.load(poster.words,5)>0);
    console.log(`worker Poster PASS producers=2 completions=${done.count} unique=${done.unique} notify_calls=${Atomics.load(poster.words,5)} hot_notify_calls=0`);
  } finally {clearTimeout(timeout);await Promise.all(all.map(w=>w.terminate()));}
} else {
  const p=new Poster(workerData.buffer);
  if(workerData.kind==='consumer'){
    const seen=new Set();let count=0;parentPort.postMessage({ready:true});
    while(count<2000){const c=p.take();if(!c){p.wait();continue;}
      const [token,value]=c;assert.equal(value,token^0xfedcba9876543210n);assert(!seen.has(String(token)));seen.add(String(token));count++;
    }parentPort.postMessage({done:true,count,unique:seen.size});
  } else {
    for(let i=0;i<1000;i++){
      const token=(BigInt(workerData.producer+1)<<48n)|BigInt(i);
      while(!p.post(token,token^0xfedcba9876543210n))await new Promise(r=>setImmediate(r));
    }
  }
}
