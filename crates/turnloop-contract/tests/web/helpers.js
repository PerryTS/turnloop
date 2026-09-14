export function guard(promise) {
  let timer;
  return Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error('host never scheduled a turn')),5000);})]).finally(()=>clearTimeout(timer));
}
export function sleep(ms) {return new Promise(resolve=>setTimeout(resolve,ms));}
export async function stats(base) {return await (await fetch(base+'/stats')).json();}
