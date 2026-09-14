//! connection-string/connection-string-spec.md; initial-dns-seedlist-discovery/
//! initial-dns-seedlist-discovery.md §§ Seedlist Discovery, DNS Record Validation.
use crate::{auth::Mechanism, Error, ErrorKind, Result};
use std::{collections::BTreeMap,time::Duration};
#[derive(Clone,Debug,PartialEq,Eq,PartialOrd,Ord)]
pub struct Address { pub host:String,pub port:u16 }
impl Address{
 pub fn parse(s:&str)->Result<Self>{
  let (host,port)=if let Some(rest)=s.strip_prefix('['){let (h,t)=rest.split_once(']').ok_or_else(||parse_error("Invalid IPv6 address"))?;h.parse::<std::net::Ipv6Addr>().map_err(|_|parse_error("Invalid IPv6 address"))?;(h,if t.is_empty(){27017}else{parse_port(t.strip_prefix(':').ok_or_else(||parse_error("Invalid host"))?)?})}else{
   if s.matches(':').count()>1{return Err(parse_error("IPv6 addresses must be bracketed"));}if let Some((h,p))=s.split_once(':'){(h,parse_port(p)?)}else{(s,27017)}
  };let host=decode(host)?.to_lowercase();if host.is_empty()||host.chars().any(|c|c.is_whitespace()||matches!(c,'/'|'?'|'#'|'@'|'['|']')) {return Err(parse_error("Invalid hostname"));}Ok(Self{host,port})
 }
 pub fn authority(&self)->String{if self.host.contains(':'){format!("[{}]:{}",self.host,self.port)}else{format!("{}:{}",self.host,self.port)}}
}
fn parse_port(s:&str)->Result<u16>{s.parse::<u16>().ok().filter(|&n|n!=0).ok_or_else(||parse_error("Invalid port"))}
#[derive(Clone)]
pub struct Credential{pub username:String,pub password:String,pub source:String,pub mechanism:Option<Mechanism>}
impl std::fmt::Debug for Credential{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.debug_struct("Credential").field("username",&self.username).field("source",&self.source).field("mechanism",&self.mechanism).finish_non_exhaustive()}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum ReadPreference{Primary,PrimaryPreferred,Secondary,SecondaryPreferred,Nearest}
impl ReadPreference{pub fn parse(s:&str)->Result<Self>{Ok(match s {"primary"=>Self::Primary,"primaryPreferred"=>Self::PrimaryPreferred,"secondary"=>Self::Secondary,"secondaryPreferred"=>Self::SecondaryPreferred,"nearest"=>Self::Nearest,_=>return Err(parse_error("Invalid read preference mode"))})}pub fn as_str(self)->&'static str{match self{Self::Primary=>"primary",Self::PrimaryPreferred=>"primaryPreferred",Self::Secondary=>"secondary",Self::SecondaryPreferred=>"secondaryPreferred",Self::Nearest=>"nearest"}}}
#[derive(Clone,Debug)]
pub struct Options{
 pub seeds:Vec<Address>,pub srv:Option<String>,pub database:Option<String>,pub credential:Option<Credential>,
 pub tls:bool,pub direct:bool,pub replica_set:Option<String>,pub app_name:Option<String>,
 pub connect_timeout:Duration,pub server_selection_timeout:Duration,pub socket_timeout:Duration,pub heartbeat:Duration,pub local_threshold:Duration,
 pub min_pool_size:usize,pub max_pool_size:usize,pub max_connecting:usize,pub wait_queue_timeout:Duration,pub max_idle_time:Duration,
 pub retry_reads:bool,pub retry_writes:bool,pub read_preference:ReadPreference,pub read_preference_tags:Vec<BTreeMap<String,String>>,pub max_staleness:Option<Duration>,
 pub compressors:Vec<String>,pub raw:BTreeMap<String,String>,
}
#[derive(Clone,Debug,PartialEq,Eq)]
pub enum ResolutionRequest{Srv{ name:String },Txt{name:String}}
impl Options{
 pub fn parse(uri:&str)->Result<Self>{
  let (srv,rest)=if let Some(s)=uri.strip_prefix("mongodb://"){(false,s)}else if let Some(s)=uri.strip_prefix("mongodb+srv://"){(true,s)}else{return Err(parse_error("Invalid scheme, expected connection string to start with mongodb:// or mongodb+srv://"));};
  if rest.contains('#'){return Err(parse_error("Unescaped # in connection string"));}
  let (authority,tail)=rest.split_once('/').unwrap_or((rest,""));if authority.contains('?'){return Err(parse_error("Connection string must have a slash before options"));}
  let (user,hosts)=if let Some((u,h))=authority.rsplit_once('@'){if u.contains('@'){return Err(parse_error("Username and password must be escaped"));}(Some(u),h)}else{(None,authority)};
  let mut seeds=Vec::new();for h in hosts.split(','){let a=Address::parse(h)?;if !seeds.contains(&a){seeds.push(a);}}
  if srv&&(seeds.len()!=1||hosts.contains(':')||!seeds[0].host.contains('.')){return Err(parse_error("mongodb+srv URI must include one hostname and no port"));}
  let (db,query)=tail.split_once('?').unwrap_or((tail,""));let db=decode(db)?;if db.chars().any(|c|matches!(c,'/'|'\\'|' '|'.'|'"'|'$'|'\0')){return Err(parse_error("Invalid database name"));}
  let mut raw=BTreeMap::new();let mut tags=Vec::new();
  for item in query.split('&').filter(|s|!s.is_empty()) {let (k,v)=item.split_once('=').ok_or_else(||parse_error("Connection string option requires a value"))?;let k=k.to_ascii_lowercase();let v=decode(v)?;if k=="readpreferencetags"{let mut tag=BTreeMap::new();for t in v.split(',').filter(|t|!t.is_empty()){let(k,v)=t.split_once(':').ok_or_else(||parse_error("Invalid read preference tags"))?;tag.insert(k.to_owned(),v.to_owned());}tags.push(tag);}else if raw.insert(k,v).is_some(){return Err(parse_error("Duplicate connection string option"));}}
  let mut out=Self{srv:if srv{Some(seeds[0].host.clone())}else{None},seeds,database:if db.is_empty(){None}else{Some(db)},credential:None,tls:srv,direct:false,replica_set:None,app_name:None,connect_timeout:Duration::from_secs(30),server_selection_timeout:Duration::from_secs(30),socket_timeout:Duration::ZERO,heartbeat:Duration::from_secs(10),local_threshold:Duration::from_millis(15),min_pool_size:0,max_pool_size:100,max_connecting:2,wait_queue_timeout:Duration::ZERO,max_idle_time:Duration::ZERO,retry_reads:true,retry_writes:true,read_preference:ReadPreference::Primary,read_preference_tags:tags,max_staleness:None,compressors:Vec::new(),raw};
  out.apply_options()?;
  if let Some(user)=user{let(u,p)=user.split_once(':').ok_or_else(||parse_error("SCRAM credentials require a password"))?;if u.is_empty()||u.contains([':', '/', '?', '[', ']'])||p.contains([':', '/', '?', '[', ']']){return Err(parse_error("Username and password must be escaped"));}let mechanism=out.raw.get("authmechanism").map(|s|Mechanism::parse(s)).transpose()?;out.credential=Some(Credential{username:decode(u)?,password:decode(p)?,source:out.raw.get("authsource").cloned().or_else(||out.database.clone()).unwrap_or_else(||"admin".into()),mechanism});}
  if out.raw.contains_key("authmechanism")&&out.credential.is_none(){return Err(parse_error("Authentication requires a username"));}Ok(out)
 }
 fn apply_options(&mut self)->Result<()> {
  for(k,v)in &self.raw{match k.as_str(){
   "tls"|"ssl"=>self.tls=boolean(v)?,"directconnection"=>self.direct=boolean(v)?,"replicaset"=>{if v.is_empty(){return Err(parse_error("replicaSet cannot be empty"));}self.replica_set=Some(v.clone());},"appname"=>{if v.len()>128{return Err(parse_error("appName must be at most 128 bytes"));}self.app_name=Some(v.clone());},
   "connecttimeoutms"=>self.connect_timeout=millis(v)?,"serverselectiontimeoutms"=>self.server_selection_timeout=millis(v)?,"sockettimeoutms"=>self.socket_timeout=millis(v)?,"heartbeatfrequencyms"=>self.heartbeat=millis(v)?,"localthresholdms"=>self.local_threshold=millis(v)?,"waitqueuetimeoutms"=>self.wait_queue_timeout=millis(v)?,"maxidletimems"=>self.max_idle_time=millis(v)?,
   "minpoolsize"=>self.min_pool_size=integer(v)?,"maxpoolsize"=>self.max_pool_size=integer(v)?,"maxconnecting"=>self.max_connecting=integer(v)?,"retryreads"=>self.retry_reads=boolean(v)?,"retrywrites"=>self.retry_writes=boolean(v)?,"readpreference"=>self.read_preference=ReadPreference::parse(v)?,"maxstalenessseconds"=>self.max_staleness=if v=="-1"{None}else{Some(Duration::from_secs(integer(v)? as u64))},
   "compressors"=>{self.compressors=v.split(',').filter(|v|!v.is_empty()).map(str::to_owned).collect();if self.compressors.iter().any(|c|c!="zlib"){return Err(parse_error("Only zlib compression is supported"));}},
   "authsource"=>{if v.is_empty(){return Err(parse_error("authSource cannot be empty"));}},"authmechanism"=>{Mechanism::parse(v)?;},
   "w"=>{},"journal"=>{boolean(v)?;},"wtimeoutms"=>{millis(v)?;},"readconcernlevel"=>{},
   _=>return Err(Error::new(ErrorKind::Parse,format!("option {k} is not supported"))),
  }}
  if self.raw.get("tls").zip(self.raw.get("ssl")).is_some_and(|(a,b)|a!=b){return Err(parse_error("tls and ssl options conflict"));}
  if self.direct&&(self.seeds.len()!=1||self.srv.is_some()){return Err(parse_error("directConnection requires exactly one non-SRV host"));}
  if self.heartbeat<Duration::from_millis(500)||self.max_connecting==0||(self.max_pool_size!=0&&self.min_pool_size>self.max_pool_size){return Err(parse_error("Invalid heartbeat or pool size"));}
  if self.read_preference==ReadPreference::Primary&&(!self.read_preference_tags.is_empty()||self.max_staleness.is_some()){return Err(parse_error("Primary read preference cannot be combined with tags or maxStalenessSeconds"));}
  if self.max_staleness.is_some_and(|d|d<Duration::from_secs(90)||d<self.heartbeat+Duration::from_secs(10)){return Err(parse_error("maxStalenessSeconds is too small"));}Ok(())
 }
 pub fn resolution_requests(&self)->Option<[ResolutionRequest;2]>{self.srv.as_ref().map(|s|[ResolutionRequest::Srv{name:format!("_mongodb._tcp.{s}")},ResolutionRequest::Txt{name:s.clone()}])}
 /// Host supplies both answers; TXT defaults never override explicit URI options.
 pub fn resolve(&mut self,records:&[Address],txt:&[String])->Result<()> {
  let srv=self.srv.as_ref().ok_or_else(||parse_error("Not an SRV connection"))?;if records.is_empty()||txt.len()>1{return Err(parse_error("Invalid SRV/TXT response"));}
  let labels:Vec<_>=srv.split('.').collect();let domain=if labels.len()>=3{labels[1..].join(".")}else{srv.clone()};let suffix=format!(".{domain}");
  for r in records{if !r.host.trim_end_matches('.').to_ascii_lowercase().ends_with(&suffix){return Err(parse_error("SRV host does not share parent domain"));}}
  if let Some(t)=txt.first(){for option in t.split('&'){let(k,v)=option.split_once('=').ok_or_else(||parse_error("Invalid TXT options"))?;let k=k.to_ascii_lowercase();if !matches!(k.as_str(),"authsource"|"replicaset"){return Err(parse_error("Unsupported TXT option"));}self.raw.entry(k).or_insert(decode(v)?);}}
  self.seeds=records.iter().map(|a|Address{host:a.host.trim_end_matches('.').to_ascii_lowercase(),port:a.port}).collect();self.apply_options()?;
  if let Some(c)=&mut self.credential{if let Some(s)=self.raw.get("authsource"){c.source=s.clone();}}Ok(())
 }
}
fn parse_error(m:&'static str)->Error{Error::new(ErrorKind::Parse,m)}
fn boolean(s:&str)->Result<bool>{match s{"true"=>Ok(true),"false"=>Ok(false),_=>Err(parse_error("Boolean option must be true or false"))}}
fn integer(s:&str)->Result<usize>{if s.is_empty()||!s.bytes().all(|c|c.is_ascii_digit()){return Err(parse_error("Expected a nonnegative integer"));}s.parse().map_err(|_|parse_error("Integer option out of range"))}
fn millis(s:&str)->Result<Duration>{Ok(Duration::from_millis(integer(s)? as u64))}
fn decode(s:&str)->Result<String>{let mut b=Vec::with_capacity(s.len());let mut i=0;while i<s.len(){let c=s.as_bytes()[i];if c==b'%'{let h=s.as_bytes().get(i+1..i+3).ok_or_else(||parse_error("Invalid percent escape"))?;let x=|v:u8|char::from(v).to_digit(16);let v=x(h[0]).zip(x(h[1])).map(|(a,b)|(a*16+b)as u8).ok_or_else(||parse_error("Invalid percent escape"))?;b.push(v);i+=3;}else{b.push(c);i+=1;}}if b.contains(&0){return Err(parse_error("NUL in connection string"));}String::from_utf8(b).map_err(|_|parse_error("Invalid UTF-8 escape"))}
