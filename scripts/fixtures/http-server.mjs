import http from 'node:http';
import http2 from 'node:http2';
import zlib from 'node:zlib';
const token = process.env.TURNLOOP_TEST_HTTP_TOKEN;
if (!token) throw new Error('TURNLOOP_TEST_HTTP_TOKEN is required');
const servers = [http.createServer(), http2.createServer()];
const connections = new Set();
for (const server of servers) {
let sockets = 0;
server.on('connection', socket => {
  socket.testId = ++sockets;
  connections.add(socket);
  socket.on('close', () => connections.delete(socket));
});
server.on('request', (req, res) => {
  const path = req.url;
  if (path === '/__turnloop_shutdown') {
    if (req.method !== 'POST' || req.headers['x-turnloop-test-token'] !== token) {
      res.writeHead(403); res.end('forbidden'); return;
    }
    res.end('stopping');
    res.on('finish', () => {
      for (const s of servers) s.close();
      for (const socket of connections) socket.end();
      setTimeout(() => { for (const socket of connections) socket.destroy(); }, 100).unref();
    });
    return;
  }
  if(path==='/redirect'){res.writeHead(302,{location:'/gzip'});res.end();return;}
  if(path==='/gzip'){const body=zlib.gzipSync(Buffer.from('compressed from node'));res.writeHead(200,{'content-encoding':'gzip','content-length':body.length});res.end(body);return;}
  if(path==='/trailers'){res.writeHead(200,{'trailer':'x-check'});res.write('chunk-one');res.addTrailers({'x-check':'verified'});res.end('chunk-two');return;}
  let body='';req.on('data',data=>body+=data);req.on('end',()=>{const data=`${req.method} ${path} ${body}`;res.writeHead(200,{'content-length':Buffer.byteLength(data),'x-socket':String(req.socket.testId)});res.end(data);});
});
server.on('sessionError',()=>{});
}
await Promise.all(servers.map(server => new Promise((resolve, reject) => {
  server.on('error', reject);
  server.listen(0, '127.0.0.1', resolve);
})));
console.log(JSON.stringify({h1: servers[0].address().port, h2: servers[1].address().port}));
process.on('SIGTERM', () => {
  for (const server of servers) server.close();
  for (const socket of connections) socket.destroy();
});
