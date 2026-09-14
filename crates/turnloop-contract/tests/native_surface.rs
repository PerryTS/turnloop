#![cfg(all(not(loom), any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd")))]
use turnloop::*;

#[test]
fn local_echo_and_descriptor_ownership() {
    let path = std::env::temp_dir().join(format!("tl-ipc-{}.sock", std::process::id()));
    let name = PipeName(path.clone());
    turnloop_contract::native_surface::ipc::<backend::Platform>(&name);
    std::fs::remove_file(path).expect("remove listener path");
}

#[test]
fn concurrent_256_children_exit_once() {
    turnloop_contract::native_surface::children::<backend::Platform>(std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")));
}
#[test]
fn spawned_child_stdio_uses_the_driver() {
    turnloop_contract::native_surface::child_stdio::<backend::Platform>(std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")));
}
#[test]
fn signals_reach_four_loops_on_four_threads() {
    turnloop_contract::native_surface::signal_fanout::<backend::Platform>(|| {
        // SAFETY: SIGUSR1 is subscribed by all four loops before the barrier opens.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
    });
}
#[test]
fn shared_external_wait_service_routes_and_cancels() {
    turnloop_contract::native_surface::external_waits::<backend::Platform>();
}
#[test]
fn kills_live_child_and_grandchild_as_a_group() {
    turnloop_contract::native_surface::process_group::<backend::Platform>(std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")));
}
#[test]
fn registered_processes_and_signals_do_not_spin() {
    turnloop_contract::native_surface::services_no_spin::<backend::Platform>(std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")));
}
#[test]
fn terminal_modes_resize_and_restore_on_close_and_drop() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: valid writable descriptor slots; null name/termios/winsize use defaults.
    assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) }, 0);
    // SAFETY: successful openpty transferred two new exclusively owned descriptors.
    let (_master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let get = || {
        // SAFETY: termios is valid zeroed C output storage.
        let mut mode: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: live PTY slave and initialized writable termios.
        assert_eq!(unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut mode) }, 0); mode
    };
    let original = get();
    for close in [true, false] {
        let mut l = Loop::new(Config::default()).expect("loop");
        let h = l.attach(Detached::from_fd(slave.try_clone().expect("dup slave")).expect("adopt tty"), Token(1)).expect("attach tty");
        l.tty_set_mode(h, TtyMode::Raw).expect("raw");
        assert_eq!(get().c_lflag & libc::ICANON, 0); assert_ne!(get().c_lflag & libc::ISIG, 0);
        l.tty_set_mode(h, TtyMode::Io).expect("io"); assert_eq!(get().c_lflag & libc::ISIG, 0);
        l.tty_set_mode(h, TtyMode::Normal).expect("normal"); assert_eq!(get().c_lflag, original.c_lflag);
        let resize = l.tty_resize_start(h, Token(2)).expect("resize subscription");
        let size = libc::winsize { ws_row: 31, ws_col: 117, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: live PTY and initialized window size. This fixture has no controlling session.
        assert_eq!(unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size) }, 0);
        // SAFETY: SIGWINCH is subscribed; simulate the controlling-terminal notification.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGWINCH) }, 0);
        let mut out = Completions::default(); let until = l.now() + std::time::Duration::from_secs(2);
        loop {
            assert!(l.now() < until); l.turn(Timeout::Until(until), &mut out).expect("resize turn");
            if !out.is_empty() { assert_eq!(out.len(), 1); assert!(matches!(out[0].result, OpResult::Signal(Signal::WinCh))); break; }
        }
        assert_eq!(l.tty_window_size(h).expect("size"), WindowSize { columns: 117, rows: 31 });
        l.signal_stop(resize, Token(3)).expect("stop resize");
        l.tty_set_mode(h, TtyMode::Io).expect("io again");
        if close {
            l.close(h, Token(4)).expect("close tty");
            let mut closed = false;
            while !closed { l.turn(Timeout::Now, &mut out).expect("close turn"); closed |= out.iter().any(|c| c.handle == Some(h) && matches!(c.result, OpResult::Closed)); }
        }
        drop(l);
        let restored = get(); assert_eq!(restored.c_lflag, original.c_lflag); assert_eq!(restored.c_iflag, original.c_iflag); assert_eq!(restored.c_oflag, original.c_oflag);
    }
}
#[test]
fn file_backed_stdio_runs_in_the_child() {
    use std::{io::{Read, Seek, SeekFrom, Write}, process::{Command, Stdio as ChildStdio}};
    let path = std::env::temp_dir().join(format!("tl-stdio-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).expect("fixture file");
    file.write_all(b"regular file stdin\n").expect("fixture write"); file.seek(SeekFrom::Start(0)).expect("seek");
    let output = Command::new(env!("CARGO_BIN_EXE_native_child")).arg("stdio").stdin(ChildStdio::from(file.try_clone().expect("dup file"))).output().expect("spawn file stdin");
    assert_eq!(output.status.code(), Some(23)); assert_eq!(output.stdout, b"regular file stdin\n"); assert_eq!(output.stderr, output.stdout);
    // File-backed stdout as well: child loop must send writes to the pool.
    file.set_len(0).expect("truncate fixture"); file.seek(SeekFrom::Start(0)).expect("seek");
    let mut child = Command::new(env!("CARGO_BIN_EXE_native_child")).arg("stdio").stdin(ChildStdio::piped()).stdout(ChildStdio::from(file.try_clone().expect("dup stdout"))).stderr(ChildStdio::null()).spawn().expect("spawn file stdout");
    child.stdin.take().expect("child stdin").write_all(b"regular file stdout\n").expect("send");
    assert_eq!(child.wait().expect("wait").code(), Some(23));
    file.seek(SeekFrom::Start(0)).expect("seek"); let mut bytes = Vec::new(); file.read_to_end(&mut bytes).expect("read file"); assert_eq!(bytes, b"regular file stdout\n");
    std::fs::remove_file(path).expect("remove fixture");
}
