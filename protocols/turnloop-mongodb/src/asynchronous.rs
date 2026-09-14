//! Getting started on turnloop: adopt an AsyncIo/TlsStream with AsyncConnection,
//! submit commands through core_mut(), and await next() to consume borrowed events.
//! Authentication/TLS requests remain explicit protocol events for the host.
use std::io;
use turnloop_io::{Instant, Output, SansIo};
/// Shared transport driver; no protocol-specific I/O loop or scratch allocation.
pub type AsyncConnection<S> = turnloop_io::Driver<S, crate::connection::Connection>;
impl Output for crate::connection::Connection {
    fn output(&self) -> &[u8] {
        self.transmit()
    }
    fn consume_output(&mut self, n: usize) -> io::Result<()> {
        self.consume_transmit(n).map_err(io::Error::other)
    }
}
impl SansIo for crate::connection::Connection {
    type Event<'a> = crate::connection::ConnectionEvent;
    fn event(
        &mut self,
        mut receive: impl FnMut(Self::Event<'_>) -> io::Result<()>,
    ) -> io::Result<bool> {
        if let Some(event) = self.poll_event() {
            receive(event)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn ingest(&mut self, bytes: &[u8], _now: Instant) -> io::Result<usize> {
        self.receive(bytes).map_err(io::Error::other)
    }
    fn disconnected(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_preserves_one_terminal_close() {
        let mut core = crate::connection::Connection::new(
            crate::uri::Options::parse("mongodb://127.0.0.1/").expect("options"),
        );
        SansIo::disconnected(&mut core);
        let mut closed = 0;
        for _ in 0..10 {
            if !SansIo::event(&mut core, |event| {
                if matches!(event, crate::connection::ConnectionEvent::Closed) {
                    closed += 1;
                }
                Ok(())
            })
            .expect("event")
            {
                break;
            }
        }
        assert_eq!(closed, 1);
        SansIo::disconnected(&mut core);
        assert!(
            !SansIo::event(&mut core, |_| panic!("duplicate terminal event")).expect("closed core")
        );
    }
}
