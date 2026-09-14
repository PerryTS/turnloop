import http from 'node:http';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const stats={slow:0,fetches:0,aborted:0,websockets:0,echoed:0};
const server=http.createServer(async(req,res)=>{
  res.setHeader('Access-Control-Allow-Origin','*');
  res.setHeader('Cross-Origin-Opener-Policy','same-origin');
  res.setHeader('Cross-Origin-Embedder-Policy','require-corp');
  res.setHeader('Cross-Origin-Resource-Policy','cross-origin');
  if(req.url==='/bytes') { stats.fetches++; res.end(Buffer.from(Array.from({length:257},(_,i)=>(i*73+19)&255))); }
  else if(req.url==='/slow') { stats.slow++; stats.fetches++; const timer=setTimeout(()=>res.end('slow'),10000); res.on('close',()=>{clearTimeout(timer);if(!res.writableEnded)stats.aborted++;}); }
  else if(req.url==='/stats') res.end(JSON.stringify(stats));
  else {
    const paths={'/worker-poster.js':'worker-poster.js','/isolated.html':'tests/isolated.html','/worker-browser.js':'tests/worker-browser.js'};
    const path=paths[req.url];
    if(!path){res.writeHead(404);res.end();return;}
    try { res.setHeader('Content-Type',path.endsWith('.html')?'text/html':'text/javascript');res.end(await readFile(root+path)); }
    catch { res.writeHead(500);res.end(); }
  }
});
server.on('upgrade',(req,socket,head)=>{
  if(req.url!=='/echo'){socket.destroy();return;}
  stats.websockets++;
  const accept=createHash('sha1').update(req.headers['sec-websocket-key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
  socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+accept+'\r\n\r\n');
  let pending=head;
  const parse=()=>{
    while(pending.length>=2){
      const op=pending[0]&15,masked=pending[1]&128;let len=pending[1]&127,off=2;
      if(len===127){socket.destroy();return;}
      if(len===126){if(pending.length<4)return;len=pending.readUInt16BE(2);off=4;}
      if(!masked){socket.destroy();return;}
      if(pending.length<off+4+len)return;
      const mask=pending.subarray(off,off+4);off+=4;
      const data=Buffer.from(pending.subarray(off,off+len));pending=pending.subarray(off+len);
      for(let i=0;i<data.length;i++)data[i]^=mask[i%4];
      if(op===8){socket.end(Buffer.from([0x88,0]));return;}
      if(op!==2){socket.destroy();return;}
      const header=len<126?Buffer.from([0x82,len]):Buffer.from([0x82,126,len>>8,len&255]);
      socket.write(Buffer.concat([header,data]));stats.echoed+=len;
    }
  };
  socket.on('data',data=>{pending=Buffer.concat([pending,data]);parse();});socket.on('error',()=>{});parse();
});
server.listen(18765,'127.0.0.1',()=>console.log('fixture ready 18765'));
process.on('SIGTERM',()=>{server.close();process.exit(0);});
