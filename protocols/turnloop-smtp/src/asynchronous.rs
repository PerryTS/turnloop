//! Getting started on turnloop: adopt an AsyncIo/TlsStream with AsyncConnection,
//! submit commands through core_mut(), and await next() to consume borrowed events.
//! Authentication/TLS requests remain explicit protocol events for the host.
use std::io;
use turnloop_io::{Instant, Output, SansIo};
/// Shared transport driver; no protocol-specific I/O loop or scratch allocation.
pub type AsyncConnection<S> = turnloop_io::Driver<S, crate::Connection>;
impl Output for crate::Connection {
    fn output(&self) -> &[u8] {
        self.output()
    }
    fn consume_output(&mut self, n: usize) -> io::Result<()> {
        self.consume_output(n);
        Ok(())
    }
}
impl SansIo for crate::Connection {
    type Event<'a> = crate::Event;
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
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        {
            self.receive(bytes, _now).map_err(io::Error::other)?;
            Ok(bytes.len())
        }
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            let _ = bytes;
            Err(io::ErrorKind::Unsupported.into())
        }
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
        let mut core = crate::Connection::new(Default::default()).expect("core");
        SansIo::disconnected(&mut core);
        let mut closed = 0;
        for _ in 0..10 {
            if !SansIo::event(&mut core, |event| {
                if matches!(event, crate::Event::Closed) {
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
