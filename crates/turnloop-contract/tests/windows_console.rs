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
            // conhost may queue its own buffer-size/focus records for a new console;
            // the assertions below must observe only the records this test writes.
            assert_ne!(
                // SAFETY: the test's isolated, owned console input handle.
                unsafe { FlushConsoleInputBuffer(input.as_raw_handle()) },
                0
            );
            let resize = driver.tty_resize_start(h, Token(3)).expect("resize");
            driver
                .read(h, ReadBuf::Pooled, Token(4))
                .expect("console read");
            let mut out = Completions::default();
            driver.turn(Timeout::Now, &mut out).expect("start reader");
            assert!(
                out.is_empty(),
                "completion before any input was written: {:?}",
                out.drain().map(|c| c.result).collect::<Vec<_>>()
            );
            // Console input and output are distinct native objects. An idle
            // input worker must leave actual output usable after cached adoption
            // classification, including the mode changes above.
            // SAFETY: initialized native output storage, queried on the output handle.
            let mut before: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
            // SAFETY: owned output handle and valid screen information output.
            assert_ne!(
                // SAFETY: owned output handle and valid screen information output.
                unsafe { GetConsoleScreenBufferInfo(output.as_raw_handle(), &mut before) },
                0
            );
            // SAFETY: static immutable byte remains live until its write completion.
            let bytes = unsafe { IoBuf::from_raw_parts(b"Q".as_ptr(), 1) };
            let write = driver
                .write(screen, WriteBuf::Provided(bytes), Token(8))
                .expect("output beside idle input");
            let deadline = driver.now() + Duration::from_secs(3);
            loop {
                assert!(driver.now() < deadline, "idle console read blocked output");
                driver
                    .turn(Timeout::Until(deadline), &mut out)
                    .expect("console output turn");
                if !out.is_empty() {
                    break;
                }
            }
            assert_eq!(out.len(), 1);
            assert_eq!(
                (out[0].handle, out[0].op, out[0].token, out[0].terminal),
                (Some(screen), Some(write), Token(8), true)
            );
            assert!(matches!(out[0].result, OpResult::Wrote(1)));
            let mut character = 0u16;
            let mut count = 0;
            // SAFETY: owned screen buffer, valid one-character output and count.
            assert_ne!(
                // SAFETY: owned screen buffer, valid one-character output and count.
                unsafe {
                    ReadConsoleOutputCharacterW(
                        output.as_raw_handle(),
                        &mut character,
                        1,
                        before.dwCursorPosition,
                        &mut count,
                    )
                },
                0
            );
            assert_eq!((character, count), (b'Q' as u16, 1));
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

#[test]
fn windows_hide_and_detached_match_console_inheritance() {
    isolated(
        "windows_hide_and_detached_match_console_inheritance",
        || {
            let mut host = Loop::new(Config::default()).expect("host loop");
            host.signal_start(Signal::Int, Token(90))
                .expect("host Ctrl-C handler");
            let mut host_out = Completions::default();
            let mut checked = 0;
            // An inherited standard handle preserves console attachment even with
            // windowsHide. The all-Null/Pipe case must retain CREATE_NO_WINDOW opt-in.
            for (hide, inherited, detached) in [
                (false, 0, false),
                (true, 0, false),
                (false, 1, false),
                (true, 1, false),
                (false, 2, false),
                (true, 2, false),
                (false, 0, true),
                (true, 0, true),
            ] {
                let mut driver = Loop::new(Config::default()).expect("child owner");
                let mut spec = ProcessSpec::new(env!("CARGO_BIN_EXE_native_child"));
                assert!(
                    !spec.windows_hide && !spec.detached,
                    "Node-compatible defaults"
                );
                spec.args.push("console-probe".into());
                spec.windows_hide = hide;
                spec.detached = detached;
                spec.stdio = [
                    match inherited {
                        0 => ProcessStdio::Null,
                        1 => ProcessStdio::Inherit,
                        2 => ProcessStdio::Handle(
                            driver
                                .attach(
                                    Detached::from_handle(console(true)).expect("console handle"),
                                    Token(8),
                                )
                                .expect("inherited console adoption"),
                        ),
                        _ => unreachable!(),
                    },
                    ProcessStdio::Pipe,
                    ProcessStdio::Null,
                ];
                let child = driver.spawn(&spec, Token(1)).expect("console probe");
                driver
                    .read_start(child.stdout.expect("probe stdout"), Token(2))
                    .expect("probe output");
                let mut out = Completions::with_capacity(1);
                let mut bytes = Vec::new();
                let (mut exit, mut eof, mut triggered) = (0, 0, false);
                let attached = !detached && (!hide || inherited != 0);
                let deadline = driver.now() + Duration::from_secs(10);
                while exit == 0 || eof == 0 {
                    assert!(driver.now() < deadline, "console probe watchdog");
                    driver
                        .turn(Timeout::Until(deadline), &mut out)
                        .expect("probe turn");
                    for c in out.drain() {
                        match c.result {
                            OpResult::Read {
                                n,
                                lease: Some(data),
                            } => {
                                assert!(n > 0);
                                bytes.extend_from_slice(data.as_slice());
                            }
                            OpResult::Exited(status) => {
                                assert_eq!(status.code, Some(0));
                                exit += 1;
                            }
                            OpResult::Eof => eof += 1,
                            other => panic!("unexpected {other:?}"),
                        }
                    }
                    if bytes.contains(&b'\n') && attached && !triggered {
                        assert_ne!(
                            // SAFETY: isolated console; host and ready child both subscribed.
                            unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) },
                            0
                        );
                        triggered = true;
                        let until = host.now() + Duration::from_secs(3);
                        loop {
                            assert!(host.now() < until);
                            host.turn(Timeout::Until(until), &mut host_out)
                                .expect("host broadcast");
                            if !host_out.is_empty() {
                                break;
                            }
                        }
                        assert_eq!(host_out.len(), 1);
                        assert!(matches!(host_out[0].result, OpResult::Signal(Signal::Int)));
                    }
                }
                assert_eq!((exit, eof), (1, 1));
                let show = if hide { 0 } else { 10 }; // SW_HIDE / SW_SHOWDEFAULT
                let expected = format!(
                    "console:{attached},show:{show}\n{}",
                    if attached { "ctrl-c-received\n" } else { "" }
                );
                assert_eq!(
                    String::from_utf8(bytes)
                        .expect("probe UTF8")
                        .replace("\r\n", "\n"),
                    expected
                );
                assert_eq!(triggered, attached);
                checked += 1;
            }
            assert_eq!(checked, 8);
            println!("eight process console modes verified");
        },
    );
}

// Registered before turnloop to prove the real Windows chain continues when no
// Hup subscription claims close. WriteFile uses the redirected pipe, not console I/O.
unsafe extern "system" fn older_close_handler(control: u32) -> i32 {
    if control != CTRL_CLOSE_EVENT {
        return 0;
    }
    let message = b"older-host-close-handler\n";
    let mut written = 0;
    // SAFETY: test process owns its redirected stdout; immutable static bytes and
    // writable count remain live through the synchronous write on the control thread.
    unsafe {
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        WriteFile(
            stdout,
            message.as_ptr(),
            message.len() as u32,
            &mut written,
            ptr::null_mut(),
        );
    }
    0 // continue to the default handler, which terminates this isolated process
}

#[test]
fn real_console_close_chains_without_hup_and_allows_subscribed_cleanup() {
    use std::io::{BufRead, Read, Write};
    use windows_sys::Win32::{
        System::Threading::WaitForSingleObject,
        UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE},
    };
    const NAME: &str = "real_console_close_chains_without_hup_and_allows_subscribed_cleanup";
    if let Ok(mode) = std::env::var("TURNLOOP_CLOSE_TEST") {
        assert_ne!(
            // SAFETY: process-lifetime callback installed only in this isolated child.
            unsafe { SetConsoleCtrlHandler(Some(older_close_handler), 1) },
            0
        );
        let mut driver = Loop::new(Config::default()).expect("close child loop");
        let subscribed = mode == "hup";
        driver
            .signal_start(
                if subscribed {
                    Signal::Hup
                } else {
                    Signal::Break
                },
                Token(7),
            )
            .expect("turnloop handler after host");
        // SAFETY: queries the isolated child's console window; no pointer arguments.
        let window = unsafe { GetConsoleWindow() };
        assert!(
            !window.is_null(),
            "CREATE_NEW_CONSOLE must create a console"
        );
        println!("ready:{}", window as usize);
        std::io::stdout().flush().expect("close fixture ready");
        let mut out = Completions::default();
        let deadline = driver.now() + Duration::from_secs(10);
        loop {
            assert!(driver.now() < deadline, "OS close never arrived");
            driver
                .turn(Timeout::Until(deadline), &mut out)
                .expect("close delivery");
            if !out.is_empty() {
                assert!(subscribed);
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].token, Token(7));
                assert!(matches!(out[0].result, OpResult::Signal(Signal::Hup)));
                drop(driver); // sleeping handler must not retain subscription access
                println!("hup-delivered-and-loop-dropped");
                std::io::stdout().flush().expect("cleanup proof");
                std::process::exit(23);
            }
        }
    }
    let mut cases = 0;
    for mode in ["chain", "hup"] {
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
                .env("TURNLOOP_CLOSE_TEST", mode)
                .creation_flags(CREATE_NEW_CONSOLE)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .expect("close fixture process");
        let stdout = child.stdout.take().expect("stdout pipe");
        let (ready, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut prefix = String::new();
            loop {
                let mut line = String::new();
                assert!(
                    reader.read_line(&mut line).expect("ready output") > 0,
                    "fixture exited before ready"
                );
                prefix.push_str(&line);
                if let Some((_, value)) = line.split_once("ready:") {
                    ready
                        .send(value.trim().parse::<usize>().expect("console HWND"))
                        .expect("ready HWND");
                    break;
                }
            }
            reader
                .read_to_string(&mut prefix)
                .expect("remaining close output");
            prefix
        });
        let window = match receive.recv_timeout(Duration::from_secs(10)) {
            Ok(window) => window,
            Err(error) => {
                child.kill().expect("readiness watchdog");
                child.wait().expect("watchdog reap");
                panic!("close fixture not ready: {error}");
            }
        };
        assert_ne!(
            // SAFETY: child sent its console HWND while waiting; WM_CLOSE has no pointers.
            unsafe { PostMessageW(window as _, WM_CLOSE, 0, 0) },
            0
        );
        let result =
            // SAFETY: std Child retains its owned process handle during this bounded wait.
            unsafe { WaitForSingleObject(child.as_raw_handle(), 10_000) };
        if result != WAIT_OBJECT_0 {
            child.kill().expect("close watchdog");
        }
        let status = child.wait().expect("close fixture status");
        let output = reader.join().expect("close output reader");
        assert_eq!(result, WAIT_OBJECT_0, "close handler stalled: {output}");
        if mode == "chain" {
            assert!(!status.success());
            assert_eq!(
                output.matches("older-host-close-handler").count(),
                1,
                "{output}"
            );
            assert!(!output.contains("hup-delivered"));
        } else {
            assert_eq!(
                status.code(),
                Some(23),
                "host did not get cleanup time: {output}"
            );
            assert_eq!(output.matches("hup-delivered-and-loop-dropped").count(), 1);
            assert!(
                !output.contains("older-host-close-handler"),
                "libuv holds the chain while Hup is subscribed"
            );
        }
        cases += 1;
    }
    assert_eq!(cases, 2);
}
