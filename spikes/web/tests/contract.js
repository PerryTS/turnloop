const assert = (ok, message) => { if (!ok) throw new Error(message); };
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const base = 'http://127.0.0.1:18765';
async function waitCompletion(loop, token) {
  const deadline = performance.now() + 10000;
  while (performance.now() < deadline) {
    const out = loop.turn(0);
    if (out.length) { assert(out.length === 1 && out[0][0] === token, 'unexpected completion'); return out[0]; }
    await delay(1);
  }
  throw new Error('completion deadline');
}
export async function runSuite(a, b) {
  try {
    assert(a.integration() === 'HostCallback', 'integration mode');
    let rejected = 0; for (const t of [-1,1,100]) { try { a.turn(t); } catch { rejected++; } }
    assert(rejected === 3, 'blocking timeouts must all be rejected');
    // Lossless 64-bit tokens, two completions in one scheduling burst.
    const token = 0xfffffffffffffff0n;
    const x = a.timer(60000, token), y = a.timer(60000, token+1n);
    const before = a.schedule_count(); a.inject(x,1); a.inject(y,1); a.inject(x,1);
    assert(a.schedule_count() === before+1, 'coalesce duplicate scheduling');
    await delay(0);
    let out = a.turn(0); assert(out.length === 2 && out[0][0] === token && out[1][0] === token+1n, 'exact tokens and once');
    assert(!a.alive(), 'completed timers not live'); a.turn(0);
    // A synchronous host turn invalidates a previously queued scheduling callback.
    const count = a.callback_count();
    const race1 = a.timer(60000,50n); a.inject(race1,1); assert(a.turn(0).length===1,'manual turn delivered');
    const race2 = a.timer(60000,51n); a.inject(race2,1); await delay(0);
    assert(a.callback_count()===count+1,'stale schedule callback ran after a manual turn');
    assert(a.turn(0).length===1,'second race completion'); a.turn(0);
    // Cancellation then Closed, no late success or duplicate Closed.
    const cancelled = a.timer(10,3n);
    assert(a.close(cancelled,4n) && !a.close(cancelled,4n), 'close accepted exactly once');
    assert(!a.cancel(cancelled), 'cancel already closed'); a.inject(cancelled,1);
    out = a.turn(0); assert(out.length===2 && out[0][0]===3n && out[0][1]===5 && out[1][0]===4n && out[1][1]===6, 'cancel/close ordering');
    await delay(20); assert(a.turn(0).length===0,'cancelled timer fired');
    // Reuse a slot then inject a stale generation; it cannot complete the new op.
    const reused = a.timer(5,5n); a.inject(cancelled,1); assert(a.turn(0).length===0,'stale callback accepted');
    assert((await waitCompletion(a,5n))[1]===1,'timer callback must run'); assert(reused !== cancelled,'generation changed'); a.turn(0);
    // Loop-local routing despite identical host token.
    a.timer(2,6n); b.timer(2,6n);
    assert((await waitCompletion(a,6n))[1]===1,'loop A timer'); assert((await waitCompletion(b,6n))[1]===1,'loop B timer');
    a.turn(0); b.turn(0);
    // Actual fetch bytes via a local server.
    a.fetch(base+'/bytes',7n); const fetched = await waitCompletion(a,7n);
    assert(fetched[1]===2 && fetched[2].length===257,'fetch completion bytes');
    for(let i=0;i<257;i++) assert(fetched[2][i]===((i*73+19)&255),'fetch byte mismatch'); a.turn(0);
    const f = a.fetch(base+'/slow',8n); await delay(20); assert(a.cancel(f),'fetch cancellation accepted');
    out=a.turn(0); assert(out.length===1 && out[0][0]===8n && out[0][1]===5,'fetch cancelled completion');
    await delay(30); assert(a.turn(0).length===0,'abort rejection caused duplicate');
    // Real WebSocket handshake, masking and server echo.
    const bytes=Uint8Array.from({length:257},(_,i)=>(i*73+19)&255);
    a.websocket('ws://127.0.0.1:18765/echo',bytes,9n);
    const echoed=await waitCompletion(a,9n); assert(echoed[1]===3 && echoed[2].length===bytes.length,'websocket message');
    for(let i=0;i<bytes.length;i++) assert(echoed[2][i]===bytes[i],'websocket byte mismatch'); a.turn(0);
    // Precision characterization, no fabricated sub-ms guarantee.
    const elapsed=[];
    for(let i=0;i<16;i++) {
      const start=performance.now(); a.timer(1,10n); const c=await waitCompletion(a,10n);
      assert(c[1]===1,'timer sample completion'); const ns=c[2]-start;
      assert(ns>=1 && ns<1000,'timer must not complete before deadline'); elapsed.push(ns); a.turn(0);
    }
    console.log(`web timer requested_ms=1 samples=16 callback_elapsed_ms min=${Math.min(...elapsed)} max=${Math.max(...elapsed)}`);
    // Capacity rejection leaves existing operations intact.
    const ids=[]; for(let i=0;i<128;i++) ids.push(a.timer(60000,100n+BigInt(i)));
    let full=false; try { a.timer(60000,999n); } catch { full=true; } assert(full,'capacity rejects rather than drops work');
    for(const id of ids) assert(a.cancel(id),'cancel capacity fixture');
    out=a.turn(0); assert(out.length===128 && out.every(c=>c[1]===5),'capacity cancellation count'); a.turn(0);
    assert(!a.alive() && !b.alive(),'all operations completed');
    console.log('web contract PASS cases=10 fetch_bytes=257 websocket_bytes=257 timer_samples=16 cancelled_at_capacity=128');
    return 10;
  } finally { a.free(); b.free(); }
}
