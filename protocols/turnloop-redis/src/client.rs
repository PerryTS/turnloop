//! Redis client, pipelines and subscriber reads with timer-driven reconnects.
use std::{collections::VecDeque,io,net::SocketAddr,time::Duration};
use turnloop_io::{AsyncIo,Backend,ExecutorHandle,Instant,Output,SansIo,Driver,deadline};
use turnloop_tls::asynchronous::{ClientTls,Transport};
use crate::{Config,Event,State,resp::Value};
#[derive(Clone)]
pub struct ConnectOptions {
    pub address:SocketAddr,
    pub protocol:Config,
    pub tls:Option<ClientTls>,
    pub retry_delay:Duration,
    pub max_reconnects:u32,
}
struct Core(crate::Connection);
impl Output for Core {
    fn output(&self)->&[u8] {self.0.output()}
    fn consume_output(&mut self,n:usize)->io::Result<()> {self.0.consume_output(n);Ok(())}
}
impl SansIo for Core {
    type Event<'a>=Event;
    fn event(&mut self,mut receive:impl FnMut(Event)->io::Result<()>)->io::Result<bool> { if let Some(e)=self.0.poll_event() {receive(e)?;Ok(true)}else{Ok(false)} }
    fn ingest(&mut self,bytes:&[u8],_:Instant)->io::Result<usize> {self.0.receive(bytes).map_err(io::Error::other)?;Ok(bytes.len())}
    fn disconnected(&mut self) {self.0.transport_lost();}
}
pub struct Client<B:Backend> {
    executor:ExecutorHandle<B>,
    driver:Driver<Transport<AsyncIo<B>>,Core>,
    options:ConnectOptions,
    messages:VecDeque<Event>,
    deferred:VecDeque<Event>,
    token:u64,
    reconnects:u64,
}
pub(super) fn time(now:Instant)->io::Result<std::time::Instant> {
    #[cfg(all(target_arch="wasm32",target_os="unknown"))]
    {let _=now;Err(io::ErrorKind::Unsupported.into())}
    #[cfg(not(all(target_arch="wasm32",target_os="unknown")))]
    {Ok(now)}
}
impl<B:Backend> Client<B> {
    pub async fn connect(executor:&ExecutorHandle<B>,options:&ConnectOptions,at:Instant)->io::Result<Self> {
        let mut core=crate::Connection::new(options.protocol.clone());
        core.connect(time(executor.now())?).map_err(io::Error::other)?;
        if !matches!(core.poll_event(),Some(Event::Connect)) {return Err(io::Error::other("missing Redis connect action"));}
        let stream=deadline(executor,at,async {executor.connect(options.address,Default::default()).await.map_err(turnloop_io::error)}).await?;
        core.transport_connected().map_err(io::Error::other)?;
        let mut client=Self {executor:executor.clone(),driver:Driver::new(Transport::Plain(stream),Core(core)),options:options.clone(),messages:VecDeque::new(),deferred:VecDeque::new(),token:0,reconnects:0};
        while !matches!(client.next_event(at).await?,Event::Ready{..}) {}
        Ok(client)
    }
    pub fn is_connected(&self)->bool {self.driver.is_connected() && self.driver.core().0.state()==State::Ready}
    pub fn reconnect_count(&self)->u64 {self.reconnects}
    pub fn close(&mut self) {self.driver.core_mut().0.close();self.driver.abort();}
    async fn reconnect(&mut self,at:Instant)->io::Result<()> {
        loop {
            while let Some(event)=self.driver.core_mut().0.poll_event() {
                match event {
                    Event::Retry{attempt}=>{
                        let delay=if attempt<=self.options.max_reconnects {Some(self.options.retry_delay)}else{None};
                        self.driver.core_mut().0.retry(time(self.executor.now())?,delay).map_err(io::Error::other)?;
                    }
                    Event::Connect=>{
                        match deadline(&self.executor,at,async {self.executor.connect(self.options.address,Default::default()).await.map_err(turnloop_io::error)}).await {
                            Ok(stream)=>{
                                self.driver.replace_stream(Transport::Plain(stream))?;
                                self.driver.core_mut().0.transport_connected().map_err(io::Error::other)?;
                                self.reconnects+=1;return Ok(());
                            }
                            Err(e)=>{
                                self.driver.core_mut().0.transport_lost();
                                if self.executor.now()>=at {return Err(e);}
                            }
                        }
                    }
                    Event::CloseTransport=>{},
                    Event::Closed=>return Err(io::ErrorKind::NotConnected.into()),
                    event=>self.deferred.push_back(event),
                }
            }
            let until=self.driver.core().0.next_timeout().ok_or_else(||io::Error::other("Redis reconnect has no deadline"))?;
            // Browser raw TCP is Unsupported before reaching this point.
            #[cfg(not(all(target_arch="wasm32",target_os="unknown")))]
            let sleep_at=until.min(at);
            #[cfg(all(target_arch="wasm32",target_os="unknown"))]
            let sleep_at={let _=until;at};
            self.executor.sleep_until(sleep_at).await.map_err(turnloop_io::error)?;
            if self.executor.now()>=at {return Err(io::ErrorKind::TimedOut.into());}
            self.driver.core_mut().0.handle_timeout(time(self.executor.now())?);
        }
    }
    async fn next_event(&mut self,at:Instant)->io::Result<Event> {
        loop {
            if let Some(e)=self.deferred.pop_front() {return Ok(e);}
            if !self.driver.is_connected() {self.reconnect(at).await?;continue;}
            let mut event=None;
            let result=deadline(&self.executor,at,self.driver.next(&self.executor,|e|{event=Some(e);Ok(())})).await;
            if let Err(e)=result {
                if e.kind()==io::ErrorKind::TimedOut || self.options.max_reconnects==0 {return Err(e);}
                self.reconnect(at).await?;continue;
            }
            match event.ok_or_else(||io::Error::other("missing Redis event"))? {
                Event::UpgradeTls=>{
                    let tls=self.options.tls.as_ref().ok_or_else(||io::Error::new(io::ErrorKind::InvalidInput,"TLS configuration required"))?;
                    self.driver.upgrade_stream()?.upgrade(tls,&self.executor,at).await?;
                    self.driver.core_mut().0.tls_established().map_err(io::Error::other)?;
                }
                Event::CloseTransport=>self.driver.abort(),
                Event::Error(e)=>return Err(io::Error::other(e)),
                e=>return Ok(e),
            }
        }
    }
    /// All requests are encoded before reading; ordered replies include Redis
    /// ReplyError values without losing the remainder of a pipeline.
    pub async fn pipeline(&mut self,commands:&[&[&[u8]]],at:Instant,mut receive:impl FnMut(usize,Result<Value,crate::Error>)->io::Result<()>)->io::Result<()> {
        let mut operation=Operation{client:self,done:false};
        if !operation.client.is_connected() {
            while !operation.client.is_connected() { operation.client.next_event(at).await?; }
        }
        let first=operation.client.token.checked_add(1).ok_or_else(||io::Error::other("Redis token exhausted"))?;
        for command in commands {
            operation.client.token=operation.client.token.checked_add(1).ok_or_else(||io::Error::other("Redis token exhausted"))?;
            let token=operation.client.token;
            operation.client.driver.core_mut().0.command(token,command,Some(time(at)?)).map_err(io::Error::other)?;
        }
        let mut replies=0;
        while replies<commands.len() {
            match operation.client.next_event(at).await? {
                Event::Reply{token,result}=>{
                    if token!=first+replies as u64 {return Err(io::Error::other("Redis pipeline reply order"));}
                    receive(replies,result)?;replies+=1;
                }
                e @ (Event::Message{..}|Event::Push(_))=>operation.client.messages.push_back(e),
                Event::Closed=>return Err(io::ErrorKind::NotConnected.into()),
                _=>{}
            }
        }
        operation.done=true;Ok(())
    }
    pub async fn command(&mut self,args:&[&[u8]],at:Instant)->io::Result<Value> {
        let mut result=None;
        self.pipeline(&[args],at,|_,reply|{result=Some(reply);Ok(())}).await?;
        result.ok_or_else(||io::Error::other("missing Redis reply"))?.map_err(io::Error::other)
    }
    pub async fn subscribe(&mut self,channels:&[&[u8]],at:Instant)->io::Result<Subscriber<'_,B>> {
        let mut args=Vec::with_capacity(channels.len()+1);args.push(b"SUBSCRIBE".as_slice());args.extend_from_slice(channels);
        self.command(&args,at).await?;
        Ok(Subscriber{client:self})
    }
}
struct Operation<'a,B:Backend>{client:&'a mut Client<B>,done:bool}
impl<B:Backend> Drop for Operation<'_,B>{fn drop(&mut self){if !self.done{self.client.close();}}}
/// Subscriber ownership is exclusive. Dropping it closes the subscribed session.
pub struct Subscriber<'a,B:Backend>{client:&'a mut Client<B>}
impl<B:Backend> Subscriber<'_,B>{
    pub async fn next(&mut self,at:Instant)->io::Result<Event>{
        if let Some(event)=self.client.messages.pop_front(){return Ok(event);}
        loop {match self.client.next_event(at).await? {e @ (Event::Message{..}|Event::Push(_))=>return Ok(e),Event::Closed=>return Err(io::ErrorKind::NotConnected.into()),_=>{}}}
    }
}
impl<B:Backend> Drop for Subscriber<'_,B>{fn drop(&mut self){self.client.close();}}
