// Host imports never invoke user dispatch synchronously. Identity is (host id,
// full generational OpId); tokens stay in core and retain all 64 bits.
const hosts = new Map();
let nextHost = 1;
const get = id => { const h=hosts.get(id); if(!h) throw new Error('closed turnloop'); return h; };
export function createHost(capacity) {
  if(nextHost > 0xffffffff) throw new Error('host identity exhausted');
  const id=nextHost++;
  hosts.set(id,{ops:new Array(capacity).fill(null),resources:new Map(),schedule:()=>{},scheduled:false,epoch:0,timer:null,deadline:null,schedules:0});
  return id;
}
export function now() { return performance.now(); }
export function configure(id,schedule) { get(id).schedule=schedule; }
export function wake(id) {
  const h=get(id); if(h.scheduled)return;
  h.scheduled=true; h.schedules++;
  const epoch=++h.epoch;
  queueMicrotask(()=>{if(hosts.get(id)===h && h.scheduled && h.epoch===epoch){h.scheduled=false;h.schedule();}});
}
export function beginTurn(id) {const h=get(id);h.scheduled=false;h.epoch++;}
export function deadlineChanged(id,deadline) {
  const h=get(id);if(h.deadline===deadline)return;
  clearTimeout(h.timer);h.timer=null;h.deadline=deadline;
  if(deadline<0)return;
  const fire=()=>{
    if(hosts.get(id)!==h || h.deadline!==deadline)return;
    const remaining=deadline-now();
    if(remaining>0){h.timer=setTimeout(fire,Math.min(2147483647,Math.ceil(remaining)));return;}
    h.timer=null;wake(id);
  };
  h.timer=setTimeout(fire,Math.min(2147483647,Math.max(0,Math.ceil(deadline-now()))));
}
export function open(id,key,kind,url) { get(id).resources.set(key,{kind,url,socket:null,reader:null,messages:[],ended:false,error:false,controller:null}); }
function finish(id,op,kind,value=null) {
  const h=hosts.get(id);if(!h || h.ops[op.index]!==op || op.done)return;
  op.done=true;op.kind=kind;op.value=value;wake(id);
}
export function submit(id,key,handle,kind,bytes) {
  const h=get(id),r=h.resources.get(handle),index=Number(key&0xffffffffn);
  if(!r || h.ops[index])throw new Error('invalid operation');
  const op={key,index,handle,done:false,kind:0,value:null};h.ops[index]=op;
  const fail=()=>finish(id,op,5);
  try {
    if(kind===1) {
      const s=r.socket=new WebSocket(r.url);s.binaryType='arraybuffer';
      s.onopen=()=>finish(id,op,1);
      s.onmessage=event=>{
        // Bounded inbound queue. Overflow is an explicit transport error.
        if(r.messages.length>=h.ops.length){r.error=true;s.close();}
        else r.messages.push(new Uint8Array(event.data));
        pump(id,r);
      };
      s.onerror=()=>{r.error=true;fail();pump(id,r);};
      s.onclose=()=>{r.ended=true;fail();pump(id,r);};
    } else if(kind===2) {
      r.reader=op;
      if(r.kind===1) {
        const controller=r.controller=new AbortController();
        fetch(r.url,{signal:controller.signal}).then(async response=>{
          if(!response.ok)throw new Error(`HTTP ${response.status}`);
          const bytes=new Uint8Array(await response.arrayBuffer());
          if(h.ops[index]===op)finish(id,op,2,bytes);
        }).catch(fail);
      } else pump(id,r);
    } else if(kind===3) {r.socket.send(bytes);queueMicrotask(()=>finish(id,op,3,bytes.length));}
    else if(kind===4) {r.socket.close();queueMicrotask(()=>finish(id,op,4));}
    else throw new Error('unsupported operation');
  } catch(error) {h.ops[index]=null;if(r.reader===op)r.reader=null;throw error;}
}
function pump(id,r) {
  const op=r.reader;if(!op || op.done)return;
  if(r.messages.length)finish(id,op,2,r.messages.shift());
  else if(r.error)finish(id,op,5);
  else if(r.ended)finish(id,op,6);
}
export function status(id,key) {const op=get(id).ops[Number(key&0xffffffffn)];return op?.key===key && op.done?op.kind:0;}
export function value(id,key) {return get(id).ops[Number(key&0xffffffffn)].value;}
export function retire(id,key) {
  const h=get(id),index=Number(key&0xffffffffn),op=h.ops[index];
  if(op?.key!==key)return;
  const r=h.resources.get(op.handle);if(r?.reader===op)r.reader=null;
  h.ops[index]=null;
}
export function cancel(id,key) {
  const h=get(id),op=h.ops[Number(key&0xffffffffn)];if(op?.key!==key)return;
  const r=h.resources.get(op.handle);
  // Invalidate before abort/close, whose callbacks can arrive much later.
  retire(id,key);r?.controller?.abort();
  if(r?.socket?.readyState===0){r.socket.onopen=r.socket.onerror=r.socket.onclose=null;r.socket.close();}
}
export function release(id,key) {
  const h=get(id),r=h.resources.get(key);if(!r)return;
  r.controller?.abort();
  if(r.socket){r.socket.onopen=r.socket.onmessage=r.socket.onerror=r.socket.onclose=null;r.socket.close();}
  h.resources.delete(key);
}
export function dispose(id) {const h=get(id);clearTimeout(h.timer);for(const key of h.resources.keys())release(id,key);hosts.delete(id);}
export function schedules(id) {return get(id).schedules;}
