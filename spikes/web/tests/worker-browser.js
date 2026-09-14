import { Poster } from '/worker-poster.js';
onmessage=({data})=>{
  const p=new Poster(data.buffer);postMessage({ready:true});let count=0;const seen=new Set();
  while(count<data.count){const c=p.take();if(!c){p.wait();continue;}
    if(c[1] !== (c[0]^0xfedcba9876543210n)||seen.has(String(c[0])))throw new Error('bad completion');
    seen.add(String(c[0]));count++;
  }postMessage({count,unique:seen.size});
};
