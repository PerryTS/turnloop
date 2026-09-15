#![cfg(all(windows, not(loom)))]
#![deny(unsafe_op_in_unsafe_fn)]
use std::{
    mem::size_of,
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
    System::{
        Console::*,
        Pipes::CreatePipe,
        Threading::{
            CREATE_NEW_CONSOLE, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
            InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            PROCESS_INFORMATION, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
            WaitForSingleObject,
        },
    },
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
            // windows-2025 starts CI steps with CTRL+C processing disabled; the flag is
            // inherited through CREATE_NEW_CONSOLE (process parameters ConsoleFlags bit 0
            // was 1 in run 34932207539), so no handler observes CTRL_C_EVENT. Node and
            // libuv also honor the inherited flag. Establish the precondition explicitly,
            // as real_console_signals_fan_out_to_four_loops does; children inherit it.
            // SAFETY: isolated console child only; restores default CTRL+C processing.
            assert_ne!(unsafe { SetConsoleCtrlHandler(None, 0) }, 0);
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

// Close-test markers travel over a named pipe served by the test host. Standard output
// cannot carry them: under a pseudoconsole it is the console being closed.
static MARKER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn marker(message: &[u8]) {
    let handle = MARKER.load(std::sync::atomic::Ordering::SeqCst) as HANDLE;
    let mut written = 0;
    // SAFETY: the fixture leaks its connected marker pipe for the process lifetime;
    // immutable bytes and the count outlive this synchronous write.
    let ok = unsafe {
        WriteFile(
            handle,
            message.as_ptr(),
            message.len() as u32,
            &mut written,
            ptr::null_mut(),
        )
    };
    assert!(ok != 0 && written as usize == message.len(), "close marker");
}

// Registered before turnloop to prove the real Windows chain continues when no
// Hup subscription claims close.
unsafe extern "system" fn older_close_handler(control: u32) -> i32 {
    if control != CTRL_CLOSE_EVENT {
        return 0;
    }
    marker(b"older-host-close-handler\n");
    0 // continue to the default handler, which terminates this isolated process
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseHost {
    // CreatePseudoConsole + ClosePseudoConsole: conhost delivers a real CTRL_CLOSE_EVENT
    // to every attached process. Works in any session, including hosted CI runners.
    PseudoConsole,
    // CREATE_NEW_CONSOLE + WM_CLOSE to the console window, as a user closing it.
    NewConsoleWindow,
}

// A fixture process attached to its own console, plus the host side of its close.
struct CloseFixture {
    process: OwnedHandle,
    pseudoconsole: Option<HPCON>,
    input: Option<OwnedHandle>,
    output: Option<std::thread::JoinHandle<()>>,
}
impl CloseFixture {
    fn spawn(host: CloseHost, name: &str, mode: &str, pipe: &str) -> Self {
        use std::os::windows::ffi::OsStrExt;
        let exe = std::env::current_exe().expect("test executable");
        if host == CloseHost::NewConsoleWindow {
            let child = std::process::Command::new(&exe)
                .args(["--exact", name, "--nocapture", "--test-threads=1"])
                .env("TURNLOOP_CLOSE_TEST", mode)
                .env("TURNLOOP_CLOSE_MARKER", pipe)
                .creation_flags(CREATE_NEW_CONSOLE)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit())
                .spawn()
                .expect("close fixture process");
            let process: OwnedHandle = child.into();
            return Self {
                process,
                pseudoconsole: None,
                input: None,
                output: None,
            };
        }
        let pipe_pair = || {
            let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
            // SAFETY: two writable outputs; noninheritable anonymous pipe.
            assert_ne!(
                // SAFETY: two writable outputs; noninheritable anonymous pipe.
                unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) },
                0
            );
            // SAFETY: successful CreatePipe transferred two unique owners.
            unsafe {
                (
                    OwnedHandle::from_raw_handle(read),
                    OwnedHandle::from_raw_handle(write),
                )
            }
        };
        let (console_input, input) = pipe_pair();
        let (output, console_output) = pipe_pair();
        let mut pseudoconsole = 0;
        // SAFETY: live pipe ends are duplicated by the pseudoconsole; valid output.
        let hr = unsafe {
            CreatePseudoConsole(
                COORD { X: 80, Y: 25 },
                console_input.as_raw_handle(),
                console_output.as_raw_handle(),
                0,
                &mut pseudoconsole,
            )
        };
        assert_eq!(hr, 0, "CreatePseudoConsole");
        drop((console_input, console_output));
        // conhost blocks if its output is not drained; the thread ends at pipe EOF.
        let output = std::thread::spawn(move || {
            let mut output = std::fs::File::from(output);
            let _ = std::io::copy(&mut output, &mut std::io::sink());
        });
        let mut size = 0;
        // SAFETY: size query with a null list is the documented first call.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        let attributes = storage.as_mut_ptr().cast();
        // SAFETY: aligned storage of at least `size` bytes, initialized exactly once.
        assert_ne!(
            // SAFETY: aligned storage of at least `size` bytes, initialized exactly once.
            unsafe { InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) },
            0
        );
        // SAFETY: HPCON value attribute copied by CreateProcessW; list initialized above.
        assert_ne!(
            // SAFETY: HPCON value attribute copied by CreateProcessW; list initialized above.
            unsafe {
                UpdateProcThreadAttribute(
                    attributes,
                    0,
                    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                    pseudoconsole as *const std::ffi::c_void,
                    size_of::<HPCON>(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            },
            0
        );
        let mut command: Vec<u16> = format!(
            "\"{}\" --exact {name} --nocapture --test-threads=1",
            exe.display()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let mut environment = Vec::new();
        for (key, value) in std::env::vars_os() {
            if key == "TURNLOOP_CLOSE_TEST" || key == "TURNLOOP_CLOSE_MARKER" {
                continue;
            }
            environment.extend(key.encode_wide());
            environment.push(u16::from(b'='));
            environment.extend(value.encode_wide());
            environment.push(0);
        }
        for pair in [
            format!("TURNLOOP_CLOSE_TEST={mode}"),
            format!("TURNLOOP_CLOSE_MARKER={pipe}"),
        ] {
            environment.extend(pair.encode_utf16());
            environment.push(0);
        }
        environment.push(0);
        // SAFETY: zeroed C startup structure; fields set below.
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes;
        // SAFETY: zeroed C output structure.
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: mutable terminated command line, double-terminated UTF-16 environment
        // block and attribute list all outlive the call; no handles are inherited.
        let created = unsafe {
            CreateProcessW(
                ptr::null(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast(),
                ptr::null(),
                &startup.StartupInfo,
                &mut info,
            )
        };
        // SAFETY: initialized list; deleted exactly once after process creation.
        unsafe { DeleteProcThreadAttributeList(attributes) };
        assert_ne!(created, 0, "{}", std::io::Error::last_os_error());
        // SAFETY: successful CreateProcessW transferred both handle owners.
        let (process, thread) = unsafe {
            (
                OwnedHandle::from_raw_handle(info.hProcess),
                OwnedHandle::from_raw_handle(info.hThread),
            )
        };
        drop(thread);
        Self {
            process,
            pseudoconsole: Some(pseudoconsole),
            input: Some(input),
            output: Some(output),
        }
    }
    fn close_console(&mut self) {
        if let Some(pseudoconsole) = self.pseudoconsole.take() {
            // SAFETY: owned pseudoconsole closed exactly once. Its output is drained by
            // a separate thread, so a synchronous close cannot block on unread frames.
            unsafe { ClosePseudoConsole(pseudoconsole) };
            self.input = None;
        }
    }
    fn wait(&self, milliseconds: u32) -> u32 {
        // SAFETY: owned process handle for this bounded wait.
        unsafe { WaitForSingleObject(self.process.as_raw_handle(), milliseconds) }
    }
    fn exit_code(&self) -> u32 {
        let mut code = 0;
        // SAFETY: owned process handle and writable exit code.
        assert_ne!(
            // SAFETY: owned process handle and writable exit code.
            unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) },
            0
        );
        code
    }
}
impl Drop for CloseFixture {
    fn drop(&mut self) {
        if self.wait(0) != WAIT_OBJECT_0 {
            // SAFETY: owned process handle; terminate a fixture that failed its checks.
            unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
            self.wait(10_000);
        }
        self.close_console();
        if let Some(output) = self.output.take() {
            let _ = output.join();
        }
    }
}

#[test]
fn real_console_close_chains_without_hup_and_allows_subscribed_cleanup() {
    use std::io::{BufRead, Read};
    use windows_sys::Win32::{
        System::Pipes::*,
        UI::WindowsAndMessaging::{GetClassNameW, PostMessageW, WM_CLOSE},
    };
    const NAME: &str = "real_console_close_chains_without_hup_and_allows_subscribed_cleanup";
    if let Ok(mode) = std::env::var("TURNLOOP_CLOSE_TEST") {
        let pipe = std::fs::OpenOptions::new()
            .write(true)
            .open(std::env::var("TURNLOOP_CLOSE_MARKER").expect("marker pipe name"))
            .expect("marker pipe");
        MARKER.store(
            std::os::windows::io::IntoRawHandle::into_raw_handle(pipe) as usize,
            std::sync::atomic::Ordering::SeqCst,
        );
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
        assert!(!window.is_null(), "the fixture must own a console");
        let mut class = [0u16; 64];
        // SAFETY: live window handle and writable fixed-size class buffer.
        let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
        let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
        marker(format!("ready:{}:{class}\n", window as usize).as_bytes());
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
                marker(b"hup-delivered-and-loop-dropped\n");
                std::process::exit(23);
            }
        }
    }
    let (mut cases, mut window_cases) = (0, 0);
    for host in [CloseHost::PseudoConsole, CloseHost::NewConsoleWindow] {
        for mode in ["chain", "hup"] {
            let pipe = format!(
                r"\\.\pipe\turnloop-close-{}-{host:?}-{mode}",
                std::process::id()
            );
            let wide: Vec<u16> = pipe.encode_utf16().chain(Some(0)).collect();
            // SAFETY: terminated private name; one synchronous inbound instance.
            let server = unsafe {
                CreateNamedPipeW(
                    wide.as_ptr(),
                    PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    1,
                    0,
                    4096,
                    0,
                    ptr::null(),
                )
            };
            assert_ne!(server, INVALID_HANDLE_VALUE);
            // SAFETY: successful create transferred unique ownership.
            let server = unsafe { OwnedHandle::from_raw_handle(server) };
            let mut fixture = CloseFixture::spawn(host, NAME, mode, &pipe);
            let (ready, receive) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                // SAFETY: owned server handle; synchronous connect waits for the fixture
                // or for the host's unblocking client below.
                if unsafe { ConnectNamedPipe(server.as_raw_handle(), ptr::null_mut()) } == 0 {
                    // SAFETY: this thread's last error, read immediately.
                    assert_eq!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED);
                }
                let mut reader = std::io::BufReader::new(std::fs::File::from(server));
                let mut transcript = String::new();
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if let Some(value) = line.trim().strip_prefix("ready:") {
                        let (window, class) = value.split_once(':').expect("ready fields");
                        let _ = ready.send((
                            window.parse::<usize>().expect("console HWND"),
                            class.to_string(),
                        ));
                    }
                    transcript.push_str(&line);
                    line.clear();
                }
                let _ = reader.read_to_string(&mut transcript);
                transcript
            });
            let unblock = || {
                // SAFETY: terminated name; a failed open means the fixture connected.
                let client = unsafe {
                    CreateFileW(
                        wide.as_ptr(),
                        GENERIC_WRITE,
                        0,
                        ptr::null(),
                        OPEN_EXISTING,
                        0,
                        ptr::null_mut(),
                    )
                };
                if client != INVALID_HANDLE_VALUE {
                    // SAFETY: successful open; closing it delivers EOF to the reader.
                    drop(unsafe { OwnedHandle::from_raw_handle(client) });
                }
            };
            let (window, class) = match receive.recv_timeout(Duration::from_secs(10)) {
                Ok(ready) => ready,
                Err(error) => {
                    drop(fixture);
                    unblock();
                    let transcript = reader.join().expect("marker reader");
                    panic!("{host:?} close fixture not ready: {error}: {transcript}");
                }
            };
            if host == CloseHost::NewConsoleWindow && class != "ConsoleWindowClass" {
                // Capability gate, logged: WM_CLOSE is a close request only for a classic
                // conhost window. windows-2025 hands CREATE_NEW_CONSOLE to Windows
                // Terminal's OpenConsole; its hidden PseudoConsoleWindow is destroyed
                // by WM_CLOSE without any console event (run 34932207539). The
                // pseudoconsole cases above deliver the same OS event there.
                println!(
                    "SKIP {host:?}/{mode}: console window class {class:?} does not turn WM_CLOSE into CTRL_CLOSE_EVENT"
                );
                drop(fixture);
                unblock();
                reader.join().expect("marker reader");
                continue;
            }
            if host == CloseHost::PseudoConsole {
                fixture.close_console();
            } else {
                assert_ne!(
                    // SAFETY: child sent its console HWND while waiting; WM_CLOSE has no pointers.
                    unsafe { PostMessageW(window as _, WM_CLOSE, 0, 0) },
                    0
                );
                window_cases += 1;
            }
            let result = fixture.wait(10_000);
            let code = fixture.exit_code();
            drop(fixture);
            unblock();
            let output = reader.join().expect("close output reader");
            assert_eq!(
                result, WAIT_OBJECT_0,
                "{host:?} close handler stalled: {output}"
            );
            if mode == "chain" {
                assert_ne!(code, 0);
                assert_eq!(
                    output.matches("older-host-close-handler").count(),
                    1,
                    "{host:?}: {output}"
                );
                assert!(!output.contains("hup-delivered"));
            } else {
                assert_eq!(code, 23, "{host:?} did not get cleanup time: {output}");
                assert_eq!(output.matches("hup-delivered-and-loop-dropped").count(), 1);
                assert!(
                    !output.contains("older-host-close-handler"),
                    "libuv holds the chain while Hup is subscribed"
                );
            }
            cases += 1;
        }
    }
    // Both pseudoconsole cases always run; window cases run where WM_CLOSE applies.
    assert_eq!(cases, 2 + window_cases);
    assert!(cases >= 2);
}
