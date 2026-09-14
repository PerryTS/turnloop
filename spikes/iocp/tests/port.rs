#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use turnloop_iocp_spike::port::{Entry, Port, WAKE, Wait};

#[test]
fn bounded_wait_and_cross_thread_wake_are_isolated_per_port() -> io::Result<()> {
    let port = Arc::new(Port::new()?);
    let other = Port::new()?;
    let mut entries = [Entry::default(); 8];
    let start = Instant::now();
    assert_eq!(
        port.wait(Some(Duration::from_millis(20)), false, &mut entries)?,
        Wait::Timeout
    );
    assert!(start.elapsed() >= Duration::from_millis(15));
    assert!(start.elapsed() < Duration::from_secs(1));
    let sender = Arc::clone(&port);
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        sender.post(WAKE, 42)
    });
    assert_eq!(
        port.wait(Some(Duration::from_secs(2)), false, &mut entries)?,
        Wait::Entries(1)
    );
    thread
        .join()
        .map_err(|_| io::Error::other("producer panicked"))??;
    assert_eq!(
        (entries[0].key, entries[0].bytes, entries[0].overlapped),
        (WAKE, 42, 0)
    );
    assert_eq!(
        other.wait(Some(Duration::ZERO), false, &mut entries)?,
        Wait::Timeout
    );
    Ok(())
}
