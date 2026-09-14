//! Cluster slot routing and Sentinel discovery using the existing state machines.
use std::io;
use turnloop_io::{Backend,ExecutorHandle,Instant};
use crate::{resp::Value,routing::{Endpoint,SlotMap,Redirect,SentinelDiscovery,DiscoveryAction}};
use super::{Client,ConnectOptions};
pub struct ClusterClient<B:Backend> {executor:ExecutorHandle<B>, options:ConnectOptions, slots:SlotMap, nodes:Vec<(Endpoint,Client<B>)>, pub max_redirects:usize}
impl<B:Backend> ClusterClient<B> {
    pub async fn connect(executor:&ExecutorHandle<B>,options:ConnectOptions,at:Instant)->io::Result<Self> {
        let seed=Endpoint{host:options.address.ip().to_string(),port:options.address.port()};
        let client=Client::connect(executor,&options,at).await?;
        let mut this=Self{executor:executor.clone(),options,slots:SlotMap::new(),nodes:vec![(seed,client)],max_redirects:16};
        this.refresh(at).await?;Ok(this)
    }
    pub fn slots(&self)->&SlotMap {&self.slots}
    pub async fn refresh(&mut self,at:Instant)->io::Result<()> {
        let value=self.nodes[0].1.command(&[b"CLUSTER",b"SLOTS"],at).await?;
        self.slots.update_slots(&value,&self.nodes[0].0.host).map_err(io::Error::other)
    }
    async fn node(&mut self,endpoint:Endpoint,at:Instant)->io::Result<usize> {
        if let Some(i)=self.nodes.iter().position(|(e,_)|*e==endpoint) {return Ok(i);}
        let mut options=self.options.clone();
        options.address=turnloop_io::resolve(&self.executor,&endpoint.host,endpoint.port,at).await?;
        let client=Client::connect(&self.executor,&options,at).await?;
        self.nodes.push((endpoint,client));Ok(self.nodes.len()-1)
    }
    /// Known key layouts are validated before any bytes are written. ASKING and
    /// its command share one exclusive node connection; ASK never changes slots.
    pub async fn command(&mut self,args:&[&[u8]],at:Instant)->io::Result<Value> {
        let endpoint=self.slots.route_command(args).map_err(io::Error::other)?;
        let mut index=match endpoint {
            Some(e)=>match self.nodes.iter().position(|(n,_)|n==e) {Some(i)=>i,None=>{let e=e.clone();self.node(e,at).await?}},
            None=>0,
        };
        for _ in 0..=self.max_redirects {
            match self.nodes[index].1.command(args,at).await {
                Ok(value)=>return Ok(value),
                Err(error)=>{
                    let Some(redirect)=error.get_ref().and_then(|e|e.downcast_ref::<crate::Error>()).and_then(Redirect::parse) else {return Err(error);};
                    self.slots.apply_redirect(&redirect).map_err(io::Error::other)?;
                    index=self.node(redirect.endpoint,at).await?;
                    if redirect.asking {
                        let value=self.nodes[index].1.command(&[b"ASKING"],at).await?;
                        if value.bytes()!=Some(b"OK") {return Err(io::Error::other("ASKING refused"));}
                    }
                }
            }
        }
        Err(io::Error::other("Redis Cluster redirect limit"))
    }
}
/// Sentinel and data-node options deliberately remain separate: their ACL and
/// TLS credentials can differ. Discovery validates ROLE before returning a client.
pub async fn sentinel<B:Backend>(executor:&ExecutorHandle<B>,seeds:Vec<Endpoint>,name:String,sentinel_options:&ConnectOptions,data_options:&ConnectOptions,at:Instant)->io::Result<Client<B>> {
    let mut discovery=SentinelDiscovery::new(seeds,name).map_err(io::Error::other)?;
    let mut candidate=None;
    loop {
        if executor.now()>=at {return Err(io::ErrorKind::TimedOut.into());}
        match discovery.poll_action() {
            Some(DiscoveryAction::QueryMaster{sentinel,name})=>{
                let mut options=sentinel_options.clone();
                let result=async {
                    options.address=turnloop_io::resolve(executor,&sentinel.host,sentinel.port,at).await?;
                    let mut client=Client::connect(executor,&options,at).await?;
                    client.command(&[b"SENTINEL",b"get-master-addr-by-name",name.as_bytes()],at).await
                }.await;
                match result {Ok(reply)=>discovery.reply(&reply).map_err(io::Error::other)?,Err(_)=>discovery.failed()}
            }
            Some(DiscoveryAction::VerifyRole{candidate:endpoint})=>{
                let mut options=data_options.clone();
                let result=async {
                    options.address=turnloop_io::resolve(executor,&endpoint.host,endpoint.port,at).await?;
                    let mut client=Client::connect(executor,&options,at).await?;
                    let reply=client.command(&[b"ROLE"],at).await?;
                    Ok::<_,io::Error>((client,reply))
                }.await;
                match result {Ok((client,reply))=>{discovery.reply(&reply).map_err(io::Error::other)?;candidate=Some(client);},Err(_)=>discovery.failed()}
            }
            Some(DiscoveryAction::Discovered(_))=>return candidate.ok_or_else(||io::Error::other("missing Sentinel candidate")),
            Some(DiscoveryAction::Retry{attempt})=>{
                let delay=if attempt<=sentinel_options.max_reconnects {Some(sentinel_options.retry_delay)}else{None};
                discovery.retry(super::client::time(executor.now())?,delay).map_err(io::Error::other)?;
            }
            Some(DiscoveryAction::Stopped)=>return Err(io::Error::other("Sentinel discovery exhausted")),
            None=>{
                let until=discovery.next_timeout().ok_or_else(||io::Error::other("Sentinel has no pending deadline"))?;
                #[cfg(not(all(target_arch="wasm32",target_os="unknown")))]
                let sleep_at=until.min(at);
                #[cfg(all(target_arch="wasm32",target_os="unknown"))]
                let sleep_at={let _=until;at};
                executor.sleep_until(sleep_at).await.map_err(turnloop_io::error)?;
                discovery.handle_timeout(super::client::time(executor.now())?);
            }
        }
    }
}
