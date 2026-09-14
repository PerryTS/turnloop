#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{io, time::Duration};
use turnloop_iocp_spike::port::{Entry, Port, WAKE, Wait, bool_result};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
fn count() -> io::Result<u32> {
    let mut count = 0;
    // SAFETY: current-process pseudo handle is borrowed, output is writable.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesshandlecount
    bool_result(unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) })?;
    Ok(count)
}
#[test]
fn port_churn_returns_owned_handles_to_baseline() -> io::Result<()> {
    let baseline = count()?;
    let mut completions = 0;
    for _ in 0..128 {
        let port = Port::new()?;
        assert_eq!(count()?, baseline + 1);
        port.post(WAKE, 7)?;
        let mut entries = [Entry::default(); 1];
        assert_eq!(
            port.wait(Some(Duration::ZERO), false, &mut entries)?,
            Wait::Entries(1)
        );
        assert_eq!(entries[0].bytes, 7);
        completions += 1;
        drop(port);
        assert_eq!(count()?, baseline);
    }
    assert_eq!(completions, 128);
    Ok(())
}
