// Bounded multiple-producer/single-consumer numeric Poster prototype.
// post() returns false on contention/full: caller retains ownership and retries later.
// Header: head, tail, producer lock, wake sequence, parked flag, notify call count.
export class Poster {
  constructor(buffer, capacity=256) {
    if(capacity<=0 || (capacity&(capacity-1))!==0) throw new Error('power-of-two capacity required');
    this.capacity=capacity;
    this.words=new Int32Array(buffer ?? new SharedArrayBuffer((6+capacity*4)*4));
    if(this.words.length !== 6+capacity*4) throw new Error('capacity mismatch');
  }
  post(token, value) {
    const w=this.words;
    if(Atomics.compareExchange(w,2,0,1)!==0)return false;
    try {
      const head=Atomics.load(w,0)>>>0,tail=Atomics.load(w,1)>>>0;
      if(((head-tail)>>>0)>=this.capacity)return false;
      const offset=6+(head&(this.capacity-1))*4;
      w[offset]=Number(BigInt.asIntN(32,token));w[offset+1]=Number(BigInt.asIntN(32,token>>32n));
      w[offset+2]=Number(BigInt.asIntN(32,value));w[offset+3]=Number(BigInt.asIntN(32,value>>32n));
      Atomics.store(w,0,(head+1)|0);
    } finally { Atomics.store(w,2,0); }
    Atomics.add(w,3,1);
    if(Atomics.exchange(w,4,0)===1){Atomics.add(w,5,1);Atomics.notify(w,3,1);}
    return true;
  }
  take() {
    const w=this.words,tail=Atomics.load(w,1)>>>0;
    if(tail===(Atomics.load(w,0)>>>0))return null;
    const offset=6+(tail&(this.capacity-1))*4;
    const token=BigInt(w[offset]>>>0)|(BigInt(w[offset+1]>>>0)<<32n);
    const value=BigInt(w[offset+2]>>>0)|(BigInt(w[offset+3]>>>0)<<32n);
    Atomics.store(w,1,(tail+1)|0);return [token,value];
  }
  wait() {
    // Worker only. Acquire the sequence before publishing PARKED, then recheck work.
    const w=this.words,sequence=Atomics.load(w,3);
    Atomics.store(w,4,1);
    if(Atomics.load(w,0)!==Atomics.load(w,1)){Atomics.store(w,4,0);return 'ready';}
    const result=Atomics.wait(w,3,sequence);
    Atomics.store(w,4,0);return result;
  }
}
