#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    io,
    os::windows::process::CommandExt,
    process::Command,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;

#[test]
fn real_console_events_are_confined_to_child_console() -> io::Result<()> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_console_child"))
        .arg("--isolated-console")
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            assert_eq!(status.code(), Some(42));
            break;
        }
        if Instant::now() > deadline {
            child.kill()?;
            child.wait()?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "console child timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
