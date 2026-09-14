import http from 'node:http';
import http2 from 'node:http2';
import zlib from 'node:zlib';
const mode=process.argv[2]??'h1';
let sockets=0;
const server=mode==='h2'?http2.createServer():http.createServer();
server.on('connection',socket=>{socket.testId=++sockets;});
server.on('request',(req,res)=>{
  const path=req.url;
  if(path==='/redirect'){res.writeHead(302,{location:'/gzip'});res.end();return;}
  if(path==='/gzip'){const body=zlib.gzipSync(Buffer.from('compressed from node'));res.writeHead(200,{'content-encoding':'gzip','content-length':body.length});res.end(body);return;}
  if(path==='/trailers'){res.writeHead(200,{'trailer':'x-check'});res.write('chunk-one');res.addTrailers({'x-check':'verified'});res.end('chunk-two');return;}
  let body='';req.on('data',data=>body+=data);req.on('end',()=>{const data=`${req.method} ${path} ${body}`;res.writeHead(200,{'content-length':Buffer.byteLength(data),'x-socket':String(req.socket.testId)});res.end(data);});
});
server.on('sessionError',()=>{});
server.listen(0,'127.0.0.1',()=>console.log(server.address().port));
process.on('SIGTERM',()=>server.close(()=>process.exit(0)));
