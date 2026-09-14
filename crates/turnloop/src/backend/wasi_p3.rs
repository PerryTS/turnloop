//! Experimental direct WASI 0.3 component async backend. A persistent wait-set
//! and fixed request return areas avoid a fresh block_on or executor per turn.
mod abi;
mod wait_set;
use crate::{backend::{Backend,Event,Operation,Outcome,PollInfo,Request,Wake},*};
use std::{collections::VecDeque,net::{SocketAddr,SocketAddrV6,Ipv4Addr,Ipv6Addr},sync::Arc,time::Duration};
use wasip3::{clocks::monotonic_clock as clock,sockets::types::{TcpSocket,UdpSocket,ErrorCode,IpAddressFamily,IpSocketAddress,Ipv4SocketAddress,Ipv6SocketAddress},wit_stream::StreamPayload,wit_future::FuturePayload};
use wait_set::WaitSet;
type SocketResult=std::result::Result<(),ErrorCode>;
#[derive(Clone,Copy,Debug,Eq,PartialEq)] enum Kind {Tcp,Listener,Udp}
#[derive(Debug)] enum Socket {Tcp(TcpSocket),Udp(UdpSocket)}
#[derive(Debug)]
struct ResultFuture(u32);
impl Drop for ResultFuture {fn drop(&mut self){
    // SAFETY: owned future is dropped only after any raw read has been cancelled.
    unsafe {(SocketResult::VTABLE.drop_readable)(self.0);}
}}
#[derive(Debug)]
struct Streams {reader:wasip3::wit_stream::StreamReader<u8>,writer:Option<wasip3::wit_stream::StreamWriter<u8>>,read_done:ResultFuture,write_done:ResultFuture}
#[derive(Debug)]
pub struct Detached {streams:Option<Streams>,incoming:Option<wasip3::wit_stream::StreamReader<TcpSocket>>,socket:Socket,kind:Kind}
struct Resource {handle:Handle,transport:Detached,connect:Option<SocketAddr>,connecting:bool,ready:[bool;2],heads:[Option<usize>;2],tails:[Option<usize>;2],queued:bool}
#[derive(Clone,Copy)]
enum WaitKind {Connect,Accept,Read,Write,ReadDone,WriteDone,Send,Receive}
struct Pending {request:Request,next:Option<usize>,offset:usize,wait:Option<(u32,WaitKind)>,code:Option<u32>,area:[u32;32],lease:Option<BufLease>}
#[derive(Default)] pub struct WasiWake;
impl Wake for WasiWake {fn wake(&self)->Result<()>{Ok(())}fn syscall_count(&self)->u64{0}}
pub struct WasiP3 {resources:Vec<Option<Resource>>,ops:Vec<Option<Pending>>,ready:VecDeque<Handle>,cancelled:VecDeque<OpId>,pool:BufferPool,wake:Arc<WasiWake>,wait_set:WaitSet}
fn direction(op:&Operation)->usize {usize::from(!matches!(op,Operation::Accept{..}|Operation::Read{..}|Operation::RecvFrom(_)))}
fn address(a: SocketAddr) -> IpSocketAddress {
    match a {
        SocketAddr::V4(a) => {
            let b = a.ip().octets();
            IpSocketAddress::Ipv4(Ipv4SocketAddress {
                port: a.port(),
                address: (b[0], b[1], b[2], b[3]),
            })
        }
        SocketAddr::V6(a) => {
            let b = a.ip().segments();
            IpSocketAddress::Ipv6(Ipv6SocketAddress {
                port: a.port(),
                flow_info: a.flowinfo(),
                scope_id: a.scope_id(),
                address: (b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]),
            })
        }
    }
}
fn native(a: IpSocketAddress) -> SocketAddr {
    match a {
        IpSocketAddress::Ipv4(a) => (
            Ipv4Addr::new(a.address.0, a.address.1, a.address.2, a.address.3),
            a.port,
        )
            .into(),
        IpSocketAddress::Ipv6(a) => {
            let b = a.address;
            SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::new(b.0, b.1, b.2, b.3, b.4, b.5, b.6, b.7),
                a.port,
                a.flow_info,
                a.scope_id,
            ))
        }
    }
}

fn error(e:ErrorCode)->Error {Error::new(match e {ErrorCode::NotSupported=>ErrorKind::Unsupported,ErrorCode::InvalidArgument|ErrorCode::InvalidState=>ErrorKind::InvalidInput,ErrorCode::ConnectionRefused=>ErrorKind::ConnectionRefused,ErrorCode::ConnectionReset|ErrorCode::ConnectionAborted=>ErrorKind::ConnectionReset,ErrorCode::ConnectionBroken=>ErrorKind::BrokenPipe,ErrorCode::Timeout=>ErrorKind::TimedOut,ErrorCode::OutOfMemory=>ErrorKind::ResourceLimit,_=>ErrorKind::Other})}
fn streams(socket:&TcpSocket)->Streams {
    let (reader,read_done)=socket.receive();let (writer,send)=wasip3::wit_stream::new::<u8>();let write_done=socket.send(send);
    Streams{reader,writer:Some(writer),read_done:ResultFuture(read_done.take_handle()),write_done:ResultFuture(write_done.take_handle())}
}
impl WasiP3 {
    fn get(&self, h: Handle) -> Result<&Resource> {
        self.resources
            .get(h.index())
            .and_then(Option::as_ref)
            .filter(|r| r.handle == h)
            .ok_or(Error::new(ErrorKind::NotFound))
    }
    fn install(
        &mut self,
        h: Handle,
        transport: Detached,
        connect: Option<SocketAddr>,
    ) -> Result<()> {
        if self.resources.get(h.index()).is_none_or(Option::is_some) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        self.resources[h.index()] = Some(Resource {
            handle: h,
            transport,
            connect,
            connecting: false,
            ready: [true; 2],
            heads: [None; 2],
            tails: [None; 2],
            queued: false,
        });
        Ok(())
    }
    fn schedule(&mut self, h: Handle) {
        let Some(r) = self.resources.get_mut(h.index()).and_then(Option::as_mut) else {
            return;
        };
        if r.handle == h && !r.queued && (0..2).any(|d| r.ready[d] && r.heads[d].is_some()) {
            r.queued = true;
            self.ready.push_back(h);
        }
    }
    fn unlink(&mut self, h: Handle, i: usize, d: usize) {
        let r = self.resources[h.index()]
            .as_mut()
            .expect("registered resource");
        let next = self.ops[i].as_ref().expect("pending op").next;
        if r.heads[d] == Some(i) {
            r.heads[d] = next;
        } else {
            let mut at = r.heads[d];
            while let Some(p) = at {
                let previous = self.ops[p].as_mut().expect("queued op");
                if previous.next == Some(i) {
                    previous.next = next;
                    if r.tails[d] == Some(i) {
                        r.tails[d] = Some(p);
                    }
                    break;
                }
                at = previous.next;
            }
        }
        if r.tails[d] == Some(i) {
            r.tails[d] = None;
        }
    }
    fn run_ready(&mut self, events: &mut Vec<Event<Detached>>) {
        let budget = events.capacity().saturating_sub(events.len());
        for _ in 0..budget {
            if events.len() == events.capacity() {
                break;
            }
            let Some(h) = self.ready.pop_front() else {
                break;
            };
            let Some(r) = self.resources.get_mut(h.index()).and_then(Option::as_mut) else {
                continue;
            };
            if r.handle != h {
                continue;
            }
            r.queued = false;
            for d in 0..2 {
                if events.len() == events.capacity() {
                    break;
                }
                let r = self.resources[h.index()].as_mut().expect("resource");
                if !r.ready[d] {
                    continue;
                }
                let Some(i) = r.heads[d] else {
                    continue;
                };
                let p = self.ops[i].as_mut().expect("queued op");
                let result = execute(r, p, &self.pool, &self.wait_set);
                let event = match result {
                    Ok(Some((result, terminal))) => Some(Event {
                        op: p.request.op,
                        result: Ok(result),
                        terminal,
                    }),
                    Ok(None) => { if p.wait.is_some() && p.code.is_none() { r.ready[d] = false; } None },
                    Err(e) if e.kind == ErrorKind::WouldBlock => {
                        r.ready[d] = false;
                        None
                    }
                    Err(e) => Some(Event {
                        op: p.request.op,
                        result: Err(e),
                        terminal: true,
                    }),
                };
                if let Some(e) = event {
                    if e.terminal {
                        self.unlink(h, i, d);
                        self.ops[i] = None;
                    }
                    events.push(e);
                }
            }
            self.schedule(h);
        }
    }
}


// SAFETY: fixed Vec storage pins every canonical return area until its native
// acknowledgement. Cancellation removes wait-set membership, synchronously
// cancels buffer access, then retires. Drop quiesces operations before resources.
unsafe impl Backend for WasiP3 {
    type Wake=WasiWake;type Detached=Detached;
    fn new(config:&Config,pool:BufferPool)->Result<Self>{Ok(Self{resources:(0..config.max_handles).map(|_|None).collect(),ops:(0..config.max_operations).map(|_|None).collect(),ready:VecDeque::with_capacity(config.max_handles),cancelled:VecDeque::with_capacity(config.max_operations),pool,wake:Arc::new(WasiWake),wait_set:WaitSet::new()})}
    fn now(&self)->Instant {Instant::now()}
    fn waker(&self)->Arc<WasiWake>{self.wake.clone()}
    fn open(&mut self,h:Handle,spec:Open)->Result<()> {
        let (addr,kind,reuse,backlog)=match spec{Open::Tcp{addr,..}=>(addr,Kind::Tcp,false,0),Open::Listener{addr,opts}=>(addr,Kind::Listener,opts.reuse_port,opts.backlog),Open::Udp{addr,opts}=>(addr,Kind::Udp,opts.reuse_port,0)};
        if reuse {return Err(Error::new(ErrorKind::Unsupported));}
        let family=if addr.is_ipv4(){IpAddressFamily::Ipv4}else{IpAddressFamily::Ipv6};
        let transport=if kind==Kind::Udp {let s=UdpSocket::create(family).map_err(error)?;s.bind(address(addr)).map_err(error)?;Detached{streams:None,incoming:None,socket:Socket::Udp(s),kind}}
        else{let s=TcpSocket::create(family).map_err(error)?;let incoming=if kind==Kind::Listener{s.set_listen_backlog_size(u64::from(backlog)).map_err(error)?;s.bind(address(addr)).map_err(error)?;Some(s.listen().map_err(error)?)}else{None};Detached{streams:None,incoming,socket:Socket::Tcp(s),kind}};
        self.install(h,transport,(kind==Kind::Tcp).then_some(addr))
    }
    fn local_addr(&self,h:Handle)->Result<SocketAddr>{match &self.get(h)?.transport.socket{Socket::Tcp(s)=>s.get_local_address(),Socket::Udp(s)=>s.get_local_address()}.map(native).map_err(error)}
    fn submit(&mut self, request: Request) -> Result<()> {
        let h = request.handle;
        let r = self.get(h)?;
        let valid = match &request.operation {
            Operation::Accept { .. } => r.transport.kind == Kind::Listener,
            Operation::RecvFrom(_) | Operation::SendTo { .. } => r.transport.kind == Kind::Udp,
            Operation::Connect => r.transport.kind == Kind::Tcp && r.connect.is_some(),
            _ => r.transport.kind == Kind::Tcp,
        };
        if !valid || self.ops.get(request.op.index()).is_none_or(Option::is_some) {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        if matches!(&request.operation, Operation::Read { buf: ReadBuf::Provided(b), .. } if b.is_empty())
        {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let d = direction(&request.operation);
        let i = request.op.index();
        let r = self.resources[h.index()].as_mut().expect("validated");
        if let Some(tail) = r.tails[d] {
            self.ops[tail].as_mut().expect("tail").next = Some(i);
        } else {
            r.heads[d] = Some(i);
        }
        r.tails[d] = Some(i);
        self.ops[i] = Some(Pending {
            request,
            next: None,
            offset: 0,
            wait:None,code:None,area:[0;32],lease:None,
        });
        self.schedule(h);
        Ok(())
    }
    fn cancel(&mut self, op: OpId) -> Result<()> {
        let p = self
            .ops
            .get(op.index())
            .and_then(Option::as_ref)
            .filter(|p| p.request.op == op)
            .ok_or(Error::new(ErrorKind::NotFound))?;
        let h = p.request.handle;
        let d = direction(&p.request.operation);
        quiesce(self.ops[op.index()].as_mut().expect("pending"), &self.wait_set);
        self.unlink(h, op.index(), d);
        self.cancelled.push_back(op);
        self.schedule(h);
        Ok(())
    }
    fn has_work(&self) -> bool {
        !self.ready.is_empty() || !self.cancelled.is_empty()
    }


    fn poll(&mut self,timeout:Option<Duration>,events:&mut Vec<Event<Detached>>)->Result<PollInfo>{
        while events.len()<events.capacity(){let Some(op)=self.cancelled.pop_front()else{break;};self.ops[op.index()]=None;events.push(Event{op,terminal:true,result:Ok(Outcome::Cancelled)});}
        self.run_ready(events);
        if self.has_work()||!events.is_empty()||events.len()==events.capacity(){return Ok(PollInfo::default());}
        let deadline=timeout.filter(|d|!d.is_zero()).map(|d|{
            let at=clock::now().saturating_add(d.as_nanos().min(u128::from(u64::MAX)) as u64);
            // SAFETY: scalar async import, no guest buffer can outlive this turn.
            let packed=unsafe {abi::deadline(at)};let task=packed>>4;
            if task!=0 && packed&15<2 {self.wait_set.join(task);} (task,packed&15)
        });
        let blocking=timeout!=Some(Duration::ZERO) && deadline.is_none_or(|(_,status)|status<2);
        if deadline.is_none() && timeout.is_none() && !self.ops.iter().flatten().any(|p|p.wait.is_some()){return Err(Error::new(ErrorKind::Unsupported));}
        let (kind,waitable,code)=self.wait_set.step(blocking);
        if let Some((task,initial))=deadline && task!=0 {
            self.wait_set.remove(task);
            if initial<2 && !(kind!=0 && waitable==task && code>=2){
                // SAFETY: synchronously cancel this turn-owned deadline subtask before dropping it.
                unsafe {wait_set::subtask_cancel(task);}
            }
            // SAFETY: deadline returned or cancellation acknowledged, and membership removed.
            unsafe {wait_set::subtask_drop(task);}
        }
        if kind!=0 {
            for i in 0..self.ops.len(){
                let Some(p)=&mut self.ops[i]else{continue;};
                if p.wait.is_some_and(|(w,_)|w==waitable){p.code=Some(code);let h=p.request.handle;let d=direction(&p.request.operation);self.resources[h.index()].as_mut().expect("owner").ready[d]=true;self.schedule(h);break;}
            }
        }
        self.run_ready(events);
        Ok(PollInfo{waits:1,zero_event_waits:u32::from(kind==0)})
    }
    fn release(&mut self,h:Handle){if self.get(h).is_ok(){self.ready.retain(|&at|at!=h);self.resources[h.index()]=None;}}
    fn detach(&mut self,_:Handle)->Result<Detached>{Err(Error::new(ErrorKind::Unsupported))}
    fn attach(&mut self,h:Handle,transport:Detached)->Result<()>{self.install(h,transport,None)}
    fn integration(&mut self)->Result<Integration>{Ok(Integration::RuntimeOwned)}
}
impl Drop for WasiP3 {fn drop(&mut self){for p in self.ops.iter_mut().flatten(){quiesce(p,&self.wait_set);}}}
fn is_subtask(kind:WaitKind)->bool{matches!(kind,WaitKind::Connect|WaitKind::Send|WaitKind::Receive)}
fn finish_wait(p:&mut Pending,set:&WaitSet)->Option<(WaitKind,u32)> {
    let (waitable,kind)=p.wait?;let code=p.code.take()?;
    if is_subtask(kind)&&code<2{return None;}
    set.remove(waitable);p.wait=None;
    if is_subtask(kind){
        // SAFETY: this subtask returned; its fixed return area remains live.
        unsafe {wait_set::subtask_drop(waitable);}
    }
    Some((kind,code))
}
fn start(p:&mut Pending,set:&WaitSet,kind:WaitKind,waitable:u32,code:u32){
    p.wait=Some((waitable,kind));
    if (is_subtask(kind)&&code<2)||code==u32::MAX {p.code=None;set.join(waitable);}else{p.code=Some(code);}
}
fn result(p:&mut Pending)->Result<()> {
    // SAFETY: a completed subtask/future initialized a SocketResult at word 16;
    // lifting transfers and releases any owned error text exactly once.
    unsafe {(SocketResult::VTABLE.lift)(p.area.as_mut_ptr().add(16).cast())}.map_err(error)
}
fn read_outcome(p:&mut Pending,n:usize)->(Outcome<Detached>,bool){
    if let Some(b)=&mut p.lease{b.set_len(n);}
    let multishot=matches!(p.request.operation,Operation::Read{multishot:true,..});
    (Outcome::Read{n,lease:p.lease.take()},!multishot)
}
fn execute(r:&mut Resource,p:&mut Pending,pool:&BufferPool,set:&WaitSet)->Result<Option<(Outcome<Detached>,bool)>>{
    if let Some((kind,code))=finish_wait(p,set){
        match kind {
            WaitKind::Connect=>{
                result(p)?;
                let Socket::Tcp(socket)=&r.transport.socket else{unreachable!()};
                r.transport.streams=Some(streams(socket));r.connect=None;
                return Ok(Some((Outcome::Connected,true)));
            }
            WaitKind::Accept=>{
                if code&15!=0 {return Err(Error::new(ErrorKind::Other));}
                assert_eq!(code>>4,1);
                // SAFETY: one canonical owned socket handle was transferred into area[0].
                let socket=unsafe {TcpSocket::from_handle(p.area[0])};
                let peer=native(socket.get_remote_address().map_err(error)?);
                let transport=Detached{streams:Some(streams(&socket)),incoming:None,socket:Socket::Tcp(socket),kind:Kind::Tcp};
                let multishot=matches!(p.request.operation,Operation::Accept{multishot:true});
                return Ok(Some((Outcome::Accepted{transport,peer},!multishot)));
            }
            WaitKind::Read=>{
                let n=(code>>4) as usize;
                if n>0 {return Ok(Some(read_outcome(p,n)));}
                if code&15==1 {
                    let s=r.transport.streams.as_ref().expect("connected");
                    let f=s.read_done.0;
                    // SAFETY: this future is read once at EOF into the pinned result area.
                    let code=unsafe {(SocketResult::VTABLE.start_read)(f,p.area.as_mut_ptr().add(16).cast())};
                    start(p,set,WaitKind::ReadDone,f,code);
                    return Ok(None);
                }
            }
            WaitKind::ReadDone=>{result(p)?;return Ok(Some((Outcome::Eof,true)));}
            WaitKind::Write=>{
                if code&15!=0 {return Err(Error::new(ErrorKind::BrokenPipe));}
                p.offset+=(code>>4) as usize;
            }
            WaitKind::WriteDone=>{result(p)?;return Ok(Some((Outcome::Shutdown,true)));}
            WaitKind::Send=>{result(p)?;return Ok(Some((Outcome::Wrote(p.offset),true)));}
            WaitKind::Receive=>{
                if p.area[0]&255!=0 {
                    p.area.copy_within(0..5,16);result(p)?;unreachable!();
                }
                let len=p.area[2] as usize;
                // SAFETY: canonical return transfers one owned byte list with len/cap len.
                let bytes=unsafe {Vec::from_raw_parts(p.area[1] as *mut u8,len,len)};
                let from=abi::decode_addr(&p.area[3..11]);
                let output=read_buffer(p,pool).ok_or(Error::new(ErrorKind::ResourceLimit))?;
                let n=output.len().min(bytes.len());output[..n].copy_from_slice(&bytes[..n]);
                if let Some(b)=&mut p.lease{b.set_len(n);}
                return Ok(Some((Outcome::RecvFrom{n,from,lease:p.lease.take()},true)));
            }
        }
    }else if p.wait.is_some(){return Ok(None);}
    let t=&mut r.transport;
    match &p.request.operation {
        Operation::Connect=>{
            let Socket::Tcp(s)=&t.socket else{unreachable!()};
            p.area[0]=s.handle();abi::encode_addr(&mut p.area[1..9],r.connect.ok_or(Error::new(ErrorKind::InvalidInput))?);
            // SAFETY: pinned initialized parameters and separate pinned SocketResult
            // area survive until this subtask returns or synchronous cancellation.
            let packed=unsafe {abi::connect(p.area.as_mut_ptr(),p.area.as_mut_ptr().add(16))};
            let task=packed>>4;
            if task==0 {result(p)?;t.streams=Some(streams(s));r.connect=None;return Ok(Some((Outcome::Connected,true)));}
            start(p,set,WaitKind::Connect,task,packed&15);
        }
        Operation::Accept{..}=>{
            let stream=t.incoming.as_ref().expect("listener").handle();
            // SAFETY: accepts one owned handle into this pinned four-byte slot.
            let code=unsafe {(TcpSocket::VTABLE.start_read)(stream,p.area.as_mut_ptr().cast(),1)};
            start(p,set,WaitKind::Accept,stream,code);
        }
        Operation::Read{..}=>{
            let stream=t.streams.as_ref().ok_or(Error::new(ErrorKind::InvalidInput))?.reader.handle();
            let Some(output)=read_buffer(p,pool)else{return Ok(None);};
            // SAFETY: provided memory follows the core lifetime contract, or the
            // lease remains owned by this pinned request until acknowledgement.
            let code=unsafe {(u8::VTABLE.start_read)(stream,output.as_mut_ptr(),output.len().min((1<<28)-1))};
            start(p,set,WaitKind::Read,stream,code);
        }
        Operation::Write(_)|Operation::Writev(_)=>{
            let bytes=write_slice(&p.request.operation,p.offset);
            if bytes.is_empty(){return Ok(Some((Outcome::Wrote(p.offset),true)));}
            let stream=t.streams.as_ref().ok_or(Error::new(ErrorKind::InvalidInput))?.writer.as_ref().ok_or(Error::new(ErrorKind::BrokenPipe))?.handle();
            // SAFETY: the accepted request owns stable bytes until terminal delivery;
            // partial writes retain offset and all buffers, including across cancel.
            let code=unsafe {(u8::VTABLE.start_write)(stream,bytes.as_ptr(),bytes.len().min((1<<28)-1))};
            start(p,set,WaitKind::Write,stream,code);
        }
        Operation::Shutdown=>{
            let s=t.streams.as_mut().ok_or(Error::new(ErrorKind::InvalidInput))?;
            s.writer.take();let future=s.write_done.0;
            // SAFETY: close send stream once, then wait for its actual send-result
            // acknowledgement before returning Shutdown. Area stays pinned.
            let code=unsafe {(SocketResult::VTABLE.start_read)(future,p.area.as_mut_ptr().add(16).cast())};
            start(p,set,WaitKind::WriteDone,future,code);
        }
        Operation::SendTo{buf,to}=>{
            let Socket::Udp(s)=&t.socket else{unreachable!()};let bytes=buf.as_slice();
            if bytes.len()>65535{return Err(Error::new(ErrorKind::InvalidInput));}
            p.offset=bytes.len();p.area[0]=s.handle();p.area[1]=bytes.as_ptr() as u32;p.area[2]=bytes.len() as u32;p.area[3]=1;abi::encode_addr(&mut p.area[4..12],*to);
            // SAFETY: canonical borrowed list and address parameters remain valid
            // until this async import returns; no generated Vec ownership transfer.
            let packed=unsafe {abi::send(p.area.as_mut_ptr(),p.area.as_mut_ptr().add(16))};
            if packed>>4==0 {result(p)?;return Ok(Some((Outcome::Wrote(p.offset),true)));}
            start(p,set,WaitKind::Send,packed>>4,packed&15);
        }
        Operation::RecvFrom(_)=>{
            let Socket::Udp(s)=&t.socket else{unreachable!()};
            if matches!(p.request.operation,Operation::RecvFrom(ReadBuf::Pooled)) && p.lease.is_none(){let Some(b)=pool.acquire()else{return Ok(None);};p.lease=Some(b);}
            // SAFETY: pinned 44-byte return record survives through completion.
            let packed=unsafe {abi::receive(s.handle(),p.area.as_mut_ptr())};
            start(p,set,WaitKind::Receive,packed>>4,packed&15);
        }
    }
    Ok(None)
}
fn read_buffer<'a>(p:&'a mut Pending,pool:&BufferPool)->Option<&'a mut [u8]>{
    let buf=match &p.request.operation{Operation::Read{buf,..}|Operation::RecvFrom(buf)=>buf,_=>unreachable!()};
    match buf{
        ReadBuf::Provided(b)=>{
            // SAFETY: core retains exclusive provided memory until acknowledgement.
            Some(unsafe {std::slice::from_raw_parts_mut(b.as_mut_ptr(),b.len())})
        }
        ReadBuf::Pooled=>{if p.lease.is_none(){p.lease=pool.acquire();}p.lease.as_mut().map(BufLease::writable)}
    }
}
fn write_slice(op:&Operation,mut offset:usize)->&[u8]{match op{
    Operation::Write(b)=>&b.as_slice()[offset..],
    Operation::Writev(bufs)=>{for b in bufs.bufs.iter().flatten(){let bytes=b.as_slice();if offset<bytes.len(){return &bytes[offset..];}offset-=bytes.len();}&[]}
    _=>unreachable!(),
}}
fn quiesce(p:&mut Pending,set:&WaitSet){
    let Some((waitable,kind))=p.wait.take()else{return;};
    set.remove(waitable);
    let code=if let Some(code)=p.code.take().filter(|&c|!is_subtask(kind)||c>=2){code}else{
        // SAFETY: matching cancellation intrinsic stops all native access to this
        // request's pinned return area and buffers before returning its status.
        unsafe {match kind{
            WaitKind::Connect|WaitKind::Send|WaitKind::Receive=>wait_set::subtask_cancel(waitable),
            WaitKind::Accept=>(TcpSocket::VTABLE.cancel_read)(waitable),
            WaitKind::Read=>(u8::VTABLE.cancel_read)(waitable),
            WaitKind::Write=>(u8::VTABLE.cancel_write)(waitable),
            WaitKind::ReadDone|WaitKind::WriteDone=>(SocketResult::VTABLE.cancel_read)(waitable),
        }}
    };
    match kind{
        WaitKind::Accept if code>>4==1=>{
            // SAFETY: cancellation raced a successful accept; discard its owned socket.
            drop(unsafe {TcpSocket::from_handle(p.area[0])});
        }
        WaitKind::Connect|WaitKind::Send if code==2=>{let _=result(p);}
        WaitKind::ReadDone|WaitKind::WriteDone if code&15==0=>{let _=result(p);}
        WaitKind::Receive if code==2=>{
            if p.area[0]&255==0{let len=p.area[2] as usize;
                // SAFETY: returned owned canonical list must be freed even when cancel wins.
                drop(unsafe {Vec::from_raw_parts(p.area[1] as *mut u8,len,len)});
            }else{p.area.copy_within(0..5,16);let _=result(p);}
        }
        _=>{}
    }
    if is_subtask(kind){
        // SAFETY: return/cancellation acknowledged; membership already removed.
        unsafe {wait_set::subtask_drop(waitable);}
    }
}
