//! Getting started on turnloop: adopt an AsyncIo/TlsStream with AsyncConnection,
//! submit commands through core_mut(), and await next() to consume borrowed events.
//! Authentication/TLS requests remain explicit protocol events for the host.
use std::io;
use turnloop_io::{Output,SansIo,Instant};
/// Shared transport driver; no protocol-specific I/O loop or scratch allocation.
pub type AsyncConnection<S> = turnloop_io::Driver<S,crate::Connection>;
impl Output for crate::Connection {
    fn output(&self)->&[u8]{self.output()}
    fn consume_output(&mut self,n:usize)->io::Result<()>{self.consume_output(n); Ok(())}
}
impl SansIo for crate::Connection {
    type Event<'a> = crate::Event;
    fn event(&mut self,mut receive:impl FnMut(Self::Event<'_>)->io::Result<()>)->io::Result<bool>{
        if let Some(event)=self.poll_event(){receive(event)?;Ok(true)}else{Ok(false)}
    }
    fn ingest(&mut self,bytes:&[u8],_now:Instant)->io::Result<usize>{self.receive(bytes).map_err(io::Error::other)?; Ok(bytes.len())}
    fn disconnected(&mut self){self.close();}
}
