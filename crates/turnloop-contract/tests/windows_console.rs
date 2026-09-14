#![cfg(all(windows, not(loom)))]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    ptr,
    time::{Duration, Instant},
};
use turnloop::*;
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Console::*, Threading::CREATE_NEW_CONSOLE},
};

fn isolated(name: &str, body: impl FnOnce()) {
    if std::env::var("TURNLOOP_CONSOLE_TEST").as_deref() == Ok(name) {
        body();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env("TURNLOOP_CONSOLE_TEST", name)
        .creation_flags(CREATE_NEW_CONSOLE)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("isolated console child");
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().expect("child status").is_none() {
        if Instant::now() >= deadline {
            child.kill().expect("watchdog kill");
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("child output");
    assert!(
        output.status.success(),
        "console child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
fn console(input: bool) -> OwnedHandle {
    let name: Vec<u16> = if input { "CONIN$\0" } else { "CONOUT$\0" }
        .encode_utf16()
        .collect();
    // SAFETY: console device path; newly created, independently owned handle.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    // SAFETY: CreateFileW successfully transferred exclusive ownership.
    unsafe { OwnedHandle::from_raw_handle(handle) }
}
fn mode(handle: &OwnedHandle) -> u32 {
    let mut mode = 0;
    // SAFETY: owned console handle and writable mode output.
    assert_ne!(
        // SAFETY: owned console handle and writable mode output.
        unsafe { GetConsoleMode(handle.as_raw_handle(), &mut mode) },
        0
    );
    mode
}
#[test]
fn real_console_signals_fan_out_to_four_loops() {
    isolated("real_console_signals_fan_out_to_four_loops", || {
        turnloop_contract::native_surface::signal_fanout::<backend::Platform>(
            Signal::Break,
            || {
                // SAFETY: test owns its isolated console; all four subscriptions are installed.
                assert_ne!(unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0) }, 0);
            },
        );
        // SAFETY: isolated child only; enable Ctrl-C delivery for the next real event.
        assert_ne!(unsafe { SetConsoleCtrlHandler(None, 0) }, 0);
        turnloop_contract::native_surface::signal_fanout::<backend::Platform>(Signal::Int, || {
            // SAFETY: isolated console and live subscriptions; no host console is affected.
            assert_ne!(unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) }, 0);
        });
    });
}
#[test]
fn console_input_resize_and_modes_restore() {
    isolated("console_input_resize_and_modes_restore", || {
        let input = console(true);
        let output = console(false);
        let original = [mode(&input), mode(&output)];
        for close in [true, false] {
            let mut driver = Loop::new(Config::default()).expect("loop");
            let h = driver
                .attach(
                    Detached::from_handle(input.try_clone().expect("input duplicate"))
                        .expect("input"),
                    Token(1),
                )
                .expect("attach input");
            let screen = driver
                .attach(
                    Detached::from_handle(output.try_clone().expect("output duplicate"))
                        .expect("output"),
                    Token(2),
                )
                .expect("attach output");
            driver.tty_set_mode(h, TtyMode::Raw).expect("raw");
            assert_eq!(mode(&input) & (ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT), 0);
            assert_ne!(mode(&input) & ENABLE_PROCESSED_INPUT, 0);
            driver.tty_set_mode(h, TtyMode::Io).expect("io");
            assert_eq!(mode(&input) & ENABLE_PROCESSED_INPUT, 0);
            driver
                .tty_set_mode(screen, TtyMode::Raw)
                .expect("VT output");
            assert_ne!(mode(&output) & ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0);
            let size = driver.tty_window_size(h).expect("input window query");
            assert!(size.columns > 0 && size.rows > 0);
            let resize = driver.tty_resize_start(h, Token(3)).expect("resize");
            driver
                .read(h, ReadBuf::Pooled, Token(4))
                .expect("console read");
            let mut out = Completions::default();
            driver.turn(Timeout::Now, &mut out).expect("start reader");
            assert!(out.is_empty());
            // SAFETY: initialized native input records; fill the selected union arms.
            let mut records: [INPUT_RECORD; 2] = unsafe { std::mem::zeroed() };
            records[0].EventType = WINDOW_BUFFER_SIZE_EVENT as u16;
            records[0].Event.WindowBufferSizeEvent.dwSize = COORD { X: 80, Y: 25 };
            records[1].EventType = KEY_EVENT as u16;
            records[1].Event.KeyEvent = KEY_EVENT_RECORD {
                bKeyDown: 1,
                wRepeatCount: 1,
                wVirtualKeyCode: 0,
                wVirtualScanCode: 0,
                uChar: KEY_EVENT_RECORD_0 {
                    UnicodeChar: b'Z' as u16,
                },
                dwControlKeyState: 0,
            };
            let mut written = 0;
            // SAFETY: test's isolated console input and two complete initialized records.
            assert_ne!(
                // SAFETY: isolated input handle and two fully initialized records.
                unsafe {
                    WriteConsoleInputW(input.as_raw_handle(), records.as_ptr(), 2, &mut written)
                },
                0
            );
            assert_eq!(written, 2);
            let (mut read, mut resized) = (false, false);
            let deadline = driver.now() + Duration::from_secs(3);
            while !read || !resized {
                assert!(driver.now() < deadline);
                driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("input turn");
                for c in out.drain() {
                    match c.result {
                        OpResult::Read {
                            n,
                            lease: Some(bytes),
                        } => {
                            assert_eq!(n, 1);
                            assert_eq!(bytes.as_slice(), b"Z");
                            assert!(!read);
                            read = true;
                        }
                        OpResult::Signal(Signal::WinCh) => {
                            resized = true;
                        }
                        other => panic!("unexpected console result: {other:?}"),
                    }
                }
            }
            driver.signal_stop(resize, Token(5)).expect("stop resize");
            if close {
                driver.close(h, Token(6)).expect("close input");
                driver.close(screen, Token(7)).expect("close output");
                while driver.alive() {
                    driver.turn(Timeout::Now, &mut out).expect("close turn");
                }
            }
            drop(driver);
            assert_eq!([mode(&input), mode(&output)], original);
        }
    });
}
