#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io, ptr,
    sync::Arc,
    time::{Duration, Instant},
};
use turnloop_iocp_spike::{
    integration::EventIntegration,
    port::{Entry, Port, TIMER},
    timer::PacketTimer,
};
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    UI::WindowsAndMessaging::*,
};

#[test]
fn gui_waiter_drains_partial_batches_and_timer() -> io::Result<()> {
    let port = Arc::new(Port::new()?);
    let mut integration = EventIntegration::new(Arc::clone(&port))?;
    let mut timer = PacketTimer::new(Arc::clone(&port))?;
    timer.arm(Duration::from_micros(250))?;
    for i in 0..1024 {
        port.post(30, i)?;
    }
    let mut seen = [false; 1024];
    let mut count = 0;
    let mut timers = 0;
    let mut event_wakes = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while count < 1024 || timers == 0 {
        assert!(Instant::now() < deadline, "external waiter lost work");
        let event = integration.event();
        // SAFETY: event live through wait; IOCP is intentionally not in wait set.
        // https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex
        let result = unsafe {
            MsgWaitForMultipleObjectsEx(1, &event, 1000, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        if result == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        assert_ne!(result, WAIT_TIMEOUT);
        if result == WAIT_OBJECT_0 {
            event_wakes += 1;
            let mut batch = [Entry::default(); 7]; // deliberately partial drain
            let n = integration.drain(&mut batch)?;
            for entry in &batch[..n] {
                if entry.key == TIMER {
                    assert_eq!(timers, 0);
                    timers += 1;
                    // SAFETY: helper dequeued this timer's packet, then forwarded it.
                    unsafe { timer.dequeued() };
                } else {
                    assert_eq!(entry.key, 30);
                    assert!(!seen[entry.bytes as usize], "duplicate forwarded packet");
                    seen[entry.bytes as usize] = true;
                    count += 1;
                }
            }
        } else {
            assert_eq!(result, WAIT_OBJECT_0 + 1);
            // SAFETY: initialized MSG and valid output; host owns message dispatch.
            unsafe {
                let mut message = std::mem::zeroed();
                while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
    assert!(seen.iter().all(|value| *value));
    assert!(event_wakes > 0);
    assert_eq!(timers, 1);
    integration.shutdown()?;
    Ok(())
}

#[test]
fn shutdown_unblocks_a_full_helper_queue() -> io::Result<()> {
    let port = Arc::new(Port::new()?);
    let mut integration = EventIntegration::new(Arc::clone(&port))?;
    for _ in 0..1024 {
        port.post(31, 1)?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while integration.high_water()? != 128 {
        assert!(
            Instant::now() < deadline,
            "helper did not fill its bounded queue"
        );
        std::thread::yield_now();
    }
    assert_eq!(integration.high_water()?, 128);
    let event = integration.event();
    // SAFETY: live event in a one-handle wait set.
    assert_eq!(
        // SAFETY: helper owns this event until after the wait returns.
        unsafe { MsgWaitForMultipleObjectsEx(1, &event, 1000, 0, 0) },
        WAIT_OBJECT_0
    );
    let mut batch = [Entry::default(); 1];
    assert_eq!(integration.drain(&mut batch)?, 1);
    assert_eq!(batch[0].bytes, 1);
    integration.shutdown()?;
    Ok(())
}
