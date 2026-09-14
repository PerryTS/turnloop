#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use windlass_iocp_spike::{
    port::{Entry, Port, TIMER, Wait},
    timer::{ApcTimer, PacketTimer, relative_due},
};

fn report(label: &str, values: &mut [Duration]) {
    values.sort_unstable();
    println!(
        "{label}: n={} p50={:?} p95={:?} max={:?}",
        values.len(),
        values[values.len() / 2],
        values[values.len() * 95 / 100],
        values[values.len() - 1]
    );
    assert_eq!(values.len(), 100);
    assert!(
        values[95] < Duration::from_millis(5),
        "functional CI lateness bound"
    );
    assert!(values[99] < Duration::from_millis(100));
}

#[test]
fn apc_precision_and_cancel() -> io::Result<()> {
    assert_eq!(relative_due(Duration::from_nanos(101)), -2);
    let port = Port::new()?;
    let mut timer = ApcTimer::new()?;
    let mut entries = [Entry::default(); 4];
    let mut lateness = [Duration::ZERO; 100];
    let delay = Duration::from_micros(250);
    for (i, late) in lateness.iter_mut().enumerate() {
        let start = Instant::now();
        timer.arm(delay)?;
        assert_eq!(
            port.wait(Some(Duration::from_secs(1)), true, &mut entries)?,
            Wait::Apc
        );
        assert_eq!(timer.count(), i as u64 + 1);
        assert!(start.elapsed() >= delay);
        *late = start.elapsed() - delay;
    }
    report("APC 250us lateness", &mut lateness);
    timer.arm(Duration::from_millis(1))?;
    timer.cancel()?;
    assert_eq!(
        port.wait(Some(Duration::from_millis(10)), true, &mut entries)?,
        Wait::Timeout
    );
    assert_eq!(timer.count(), 100);
    Ok(())
}

#[test]
fn packet_precision_and_rearm() -> io::Result<()> {
    let port = Arc::new(Port::new()?);
    let mut timer = PacketTimer::new(Arc::clone(&port))?;
    let mut entries = [Entry::default(); 4];
    let mut lateness = [Duration::ZERO; 100];
    let delay = Duration::from_micros(250);
    for late in &mut lateness {
        let start = Instant::now();
        timer.arm(delay)?;
        assert_eq!(
            port.wait(Some(Duration::from_secs(1)), false, &mut entries)?,
            Wait::Entries(1)
        );
        assert_eq!((entries[0].key, entries[0].status), (TIMER, 0));
        // SAFETY: this timer's only packet was just dequeued.
        unsafe { timer.dequeued() };
        assert!(start.elapsed() >= delay);
        *late = start.elapsed() - delay;
    }
    report("NT packet 250us lateness", &mut lateness);
    Ok(())
}

#[test]
fn packet_cancellation_removes_association_before_rearm() -> io::Result<()> {
    let port = Arc::new(Port::new()?);
    let mut timer = PacketTimer::new(Arc::clone(&port))?;
    let mut entries = [Entry::default(); 4];
    for _ in 0..100 {
        timer.arm(Duration::from_secs(30))?;
        assert_eq!(
            timer.cancel()?,
            0,
            "unsignaled timer must cancel synchronously"
        );
        assert_eq!(
            port.wait(Some(Duration::ZERO), false, &mut entries)?,
            Wait::Timeout
        );
    }
    timer.arm(Duration::from_micros(250))?;
    assert_eq!(
        port.wait(Some(Duration::from_secs(2)), false, &mut entries)?,
        Wait::Entries(1)
    );
    assert_eq!(entries[0].key, TIMER);
    // SAFETY: this timer's newly armed packet was dequeued.
    unsafe { timer.dequeued() };
    Ok(())
}
