//! Extra child descriptors: Node's `stdio` tail, its IPC channel, and the
//! session control a pty child needs. Every assertion runs on Unix and Windows.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg(all(
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "windows"
    )
))]
use std::time::Duration;
use turnloop::*;

const CHILD: &str = env!("CARGO_BIN_EXE_native_child");
const LIMIT: Duration = Duration::from_secs(20);

/// Everything a turn can deliver that a descriptor exchange does not consume.
#[derive(Default)]
struct Seen {
    exit: Option<ExitStatus>,
    eof: usize,
    wrote: usize,
    cancelled: usize,
    closed: usize,
    /// Terminal lifecycle results in arrival order, as `(handle, Cancelled?)`.
    terminals: Vec<(u64, bool)>,
}

struct Rig {
    driver: Loop,
    out: Completions,
    seen: Seen,
    /// Bytes already delivered for a handle but not yet consumed by a caller.
    /// A stream read returns whatever has arrived, so framing is the test's job.
    buffered: std::collections::HashMap<u64, Vec<u8>>,
}
impl Rig {
    fn new() -> Self {
        Self {
            driver: Loop::new(Config::default()).expect("loop"),
            out: Completions::default(),
            seen: Seen::default(),
            buffered: std::collections::HashMap::new(),
        }
    }
    /// One bounded turn, routing reads into their handle's buffer.
    fn turn(&mut self, deadline: Instant) {
        assert!(self.driver.now() < deadline, "deadline");
        self.turn_for(Timeout::Until(deadline));
    }
    /// A short turn for waiting on something the loop itself cannot observe,
    /// such as a grandchild that no handle of this loop refers to.
    fn settle(&mut self) {
        self.turn_for(Timeout::After(Duration::from_millis(10)));
    }
    fn turn_for(&mut self, timeout: Timeout) {
        self.driver.turn(timeout, &mut self.out).expect("turn");
        for c in self.out.drain() {
            match c.result {
                OpResult::Read { n, lease } => {
                    assert!(n > 0, "a read completion carries bytes");
                    let bytes = lease.expect("pooled lease");
                    let handle = c.handle.expect("a read names its handle");
                    self.buffered
                        .entry(handle.key())
                        .or_default()
                        .extend_from_slice(bytes.as_slice());
                }
                OpResult::Wrote(n) => {
                    assert!(n > 0);
                    self.seen.wrote += 1;
                }
                OpResult::Eof => self.seen.eof += 1,
                OpResult::Exited(status) => {
                    assert!(self.seen.exit.replace(status).is_none(), "exit is once");
                }
                OpResult::Cancelled => {
                    assert!(c.op.is_some(), "a cancellation names its operation");
                    assert!(c.terminal);
                    self.seen.cancelled += 1;
                    self.seen
                        .terminals
                        .push((c.handle.expect("handle").key(), true));
                }
                OpResult::Closed => {
                    assert!(c.op.is_none(), "Closed is the handle's own result");
                    assert!(c.terminal);
                    self.seen.closed += 1;
                    self.seen
                        .terminals
                        .push((c.handle.expect("handle").key(), false));
                }
                other => panic!("unexpected completion {other:?}"),
            }
        }
    }
    fn have(&self, h: Handle) -> usize {
        self.buffered.get(&h.key()).map_or(0, Vec::len)
    }
    /// Consume exactly `want` bytes from `h`, reading more when short.
    fn read_exact(&mut self, h: Handle, want: usize) -> Vec<u8> {
        let deadline = self.driver.now() + LIMIT;
        while self.have(h) < want {
            let before = self.have(h);
            self.driver
                .read(h, ReadBuf::Pooled, Token(90))
                .expect("submit read");
            while self.have(h) == before {
                self.turn(deadline);
            }
        }
        let rest = self.buffered.get_mut(&h.key()).expect("buffered bytes");
        rest.drain(..want).collect()
    }
    /// Consume one newline-terminated line, newline included.
    fn read_line(&mut self, h: Handle) -> String {
        let mut line = Vec::new();
        loop {
            line.extend_from_slice(&self.read_exact(h, 1));
            if line.ends_with(b"\n") {
                return String::from_utf8(line).expect("text line");
            }
        }
    }
    fn write(&mut self, h: Handle, bytes: &[u8]) {
        self.driver
            .write(h, WriteBuf::Owned(bytes.to_vec()), Token(91))
            .expect("submit write");
    }
    fn exit_status(&mut self) -> ExitStatus {
        let deadline = self.driver.now() + LIMIT;
        while self.seen.exit.is_none() {
            self.turn(deadline);
        }
        self.seen.exit.expect("child exit")
    }
    /// Close a handle and require `ops` cancellations, then exactly one Closed,
    /// in that order and only once.
    fn close_completely(&mut self, h: Handle, ops: usize) {
        let (cancelled, closed) = (self.seen.cancelled, self.seen.closed);
        let before = self.seen.terminals.len();
        self.driver.close(h, Token(92)).expect("close");
        let deadline = self.driver.now() + LIMIT;
        while self.seen.closed == closed {
            self.turn(deadline);
        }
        assert_eq!(self.seen.closed, closed + 1, "one Closed per handle");
        assert_eq!(
            self.seen.cancelled,
            cancelled + ops,
            "one Cancelled per outstanding operation"
        );
        let mine: Vec<bool> = self.seen.terminals[before..]
            .iter()
            .filter(|(key, _)| *key == h.key())
            .map(|(_, cancelled)| *cancelled)
            .collect();
        let mut expected = vec![true; ops];
        expected.push(false);
        assert_eq!(mine, expected, "cancellations precede the single Closed");
    }
}

fn channel_spec() -> ProcessSpec {
    let mut spec = ProcessSpec::new(CHILD);
    spec.windows_hide = true;
    spec.args = vec!["channel".into()];
    spec.stdio = [ProcessStdio::Null, ProcessStdio::Pipe, ProcessStdio::Null];
    spec.extra = vec![
        ChildFd {
            number: 3,
            source: ChildFdSource::Duplex,
        },
        ChildFd {
            number: 4,
            source: ChildFdSource::Duplex,
        },
    ];
    // Exactly Node's handoff: the number lives in the environment, and the
    // child finds its channel by reading it back.
    spec.env = vec![
        ("NODE_CHANNEL_FD".into(), "3".into()),
        ("TURNLOOP_EXTRA_FD".into(), "4".into()),
    ];
    spec
}

#[test]
fn five_descriptors_carry_bytes_both_ways_through_a_channel_handoff() {
    let mut rig = Rig::new();
    let spec = channel_spec();
    let mut parents = [None; 2];
    let child = rig
        .driver
        .spawn_extra(&spec, Token(1), &mut parents)
        .expect("spawn with extra descriptors");
    let channel = parents[0].expect("descriptor 3 parent end");
    let extra = parents[1].expect("descriptor 4 parent end");
    let stdout = child.stdout.expect("child stdout");
    assert_ne!(channel, extra);

    rig.write(channel, b"ping-3");
    assert_eq!(
        rig.read_exact(channel, 6),
        b"pong-3",
        "descriptor 3 replies"
    );
    rig.write(extra, b"ping-4");
    assert_eq!(rig.read_exact(extra, 6), b"pong-4", "descriptor 4 replies");
    assert_eq!(rig.read_exact(stdout, 2), b"ok", "child ran to completion");
    assert_eq!(
        rig.exit_status(),
        ExitStatus {
            code: Some(0),
            signal: None
        }
    );
    assert_eq!(rig.seen.wrote, 2, "both parent writes completed");
    for h in [channel, extra, stdout] {
        rig.close_completely(h, 0);
    }
    rig.close_completely(child.handle, 0);
    assert!(!rig.driver.alive());
}

/// An OS pipe created outside the loop, so both ends are ordinary synchronous
/// descriptors a child can use with plain blocking reads and writes.
fn host_pipe() -> (Detached, Detached) {
    #[cfg(unix)]
    {
        use std::os::fd::{FromRawFd, OwnedFd};
        let mut fds = [0; 2];
        // SAFETY: writable pair of output descriptors, no other arguments.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: pipe transferred ownership of two fresh descriptors.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        (
            Detached::from_fd(read).expect("adopt read end"),
            Detached::from_fd(write).expect("adopt write end"),
        )
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: two writable handle outputs; default attributes and buffer size.
        let ok = unsafe {
            windows_sys::Win32::System::Pipes::CreatePipe(
                &mut read,
                &mut write,
                std::ptr::null(),
                0,
            )
        };
        assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
        // SAFETY: CreatePipe transferred ownership of two fresh handles.
        let (read, write) = unsafe {
            (
                OwnedHandle::from_raw_handle(read),
                OwnedHandle::from_raw_handle(write),
            )
        };
        (
            Detached::from_handle(read).expect("adopt read end"),
            Detached::from_handle(write).expect("adopt write end"),
        )
    }
}

#[test]
fn extra_sources_cover_one_way_pipes_the_null_device_and_adopted_transports() {
    let mut rig = Rig::new();
    let (read, write) = host_pipe();
    let read = rig.driver.attach(read, Token(2)).expect("attach read end");
    let write = rig
        .driver
        .attach(write, Token(3))
        .expect("attach write end");
    let mut spec = ProcessSpec::new(CHILD);
    spec.windows_hide = true;
    spec.args = vec!["extra-sources".into()];
    spec.stdio = [ProcessStdio::Null, ProcessStdio::Pipe, ProcessStdio::Null];
    spec.extra = vec![
        ChildFd {
            number: 3,
            source: ChildFdSource::Pipe,
        },
        ChildFd {
            number: 4,
            source: ChildFdSource::Null,
        },
        ChildFd {
            number: 5,
            source: ChildFdSource::Handle(write),
        },
    ];
    let mut parents = [None; 3];
    let child = rig
        .driver
        .spawn_extra(&spec, Token(4), &mut parents)
        .expect("spawn");
    let one_way = parents[0].expect("one-way parent end");
    assert!(parents[1].is_none(), "the null device has no parent end");
    assert!(parents[2].is_none(), "an adopted transport has no new end");
    let stdout = child.stdout.expect("child stdout");

    assert_eq!(rig.read_exact(one_way, 5), b"three", "child wrote fd 3");
    assert_eq!(rig.read_exact(read, 4), b"five", "child wrote fd 5");
    assert_eq!(rig.read_exact(stdout, 2), b"ok", "null device read as EOF");
    assert_eq!(
        rig.exit_status(),
        ExitStatus {
            code: Some(0),
            signal: None
        }
    );
    // The loop kept its own end of the adopted transport throughout.
    assert!(rig.driver.raw_transport(write).is_ok());
    for h in [one_way, stdout, read, write] {
        rig.close_completely(h, 0);
    }
    rig.close_completely(child.handle, 0);
}

#[test]
fn close_orders_cancel_before_closed_for_a_child_holding_extra_descriptors() {
    let mut rig = Rig::new();
    let mut spec = ProcessSpec::new(CHILD);
    spec.windows_hide = true;
    spec.args = vec!["sleep".into()];
    spec.stdio = [ProcessStdio::Null; 3];
    spec.extra = vec![
        ChildFd {
            number: 3,
            source: ChildFdSource::Duplex,
        },
        ChildFd {
            number: 4,
            source: ChildFdSource::Pipe,
        },
    ];
    let mut parents = [None; 2];
    let child = rig
        .driver
        .spawn_extra(&spec, Token(5), &mut parents)
        .expect("spawn");
    let extras = [parents[0].expect("fd 3"), parents[1].expect("fd 4")];
    // Closing a live child terminates it and waits for reaping: its exit
    // operation is the one outstanding operation, so it yields one Cancelled.
    rig.close_completely(child.handle, 1);
    assert!(rig.seen.exit.is_none(), "a cancelled watch reports no exit");
    #[cfg(unix)]
    {
        let mut status = 0;
        assert_eq!(
            // SAFETY: WNOHANG query of this fixture child only; ECHILD proves
            // the library reaped it rather than leaving a zombie.
            unsafe { libc::waitpid(child.pid as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
    for h in extras {
        rig.close_completely(h, 0);
    }
    assert!(!rig.driver.alive());
    rig.driver.turn(Timeout::Now, &mut rig.out).expect("quiet");
    assert!(rig.out.is_empty(), "no duplicate completions");
}

#[test]
fn rejected_descriptor_plans_create_nothing() {
    let mut driver = Loop::new(Config::default()).expect("loop");
    let base = channel_spec();
    let refuse = |driver: &mut Loop, spec: &ProcessSpec, parents: &mut [Option<Handle>]| {
        let error = driver
            .spawn_extra(spec, Token(6), parents)
            .expect_err("rejected plan");
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert!(parents.iter().all(Option::is_none), "nothing was created");
        assert!(!driver.alive(), "no handle survived a rejected spawn");
    };
    // The parent-end slice must match the plan exactly.
    refuse(&mut driver, &base, &mut [None; 1]);
    refuse(&mut driver, &base, &mut [None; 3]);
    // spawn() has nowhere to report parent ends.
    assert_eq!(
        driver.spawn(&base, Token(6)).expect_err("no sink").kind,
        ErrorKind::InvalidInput
    );
    assert!(!driver.alive());
    for number in [0, 1, 2, MAX_CHILD_FD + 1, u32::MAX] {
        let mut spec = base.clone();
        spec.extra[1].number = number;
        refuse(&mut driver, &spec, &mut [None; 2]);
    }
    // A repeated number would silently drop one of the two descriptors.
    let mut spec = base.clone();
    spec.extra[1].number = 3;
    refuse(&mut driver, &spec, &mut [None; 2]);
    // Claiming a controlling terminal requires the new session that grants it.
    let mut spec = base.clone();
    spec.controlling_terminal = true;
    refuse(&mut driver, &spec, &mut [None; 2]);
    // A handle another loop owns cannot be duplicated into a child.
    let mut elsewhere = Loop::new(Config::default()).expect("second loop");
    let (read, _write) = host_pipe();
    let foreign = elsewhere.attach(read, Token(0)).expect("attach elsewhere");
    let mut spec = base.clone();
    spec.extra[1].source = ChildFdSource::Handle(foreign);
    let error = driver
        .spawn_extra(&spec, Token(6), &mut [None; 2])
        .expect_err("foreign handle");
    assert!(matches!(
        error.kind,
        ErrorKind::NotFound | ErrorKind::InvalidInput
    ));
    assert!(!driver.alive(), "no handle survived a rejected spawn");
}

/// A failed exec must be reported as a failed spawn, even when the descriptor
/// numbers under test are exactly the ones the standard library would otherwise
/// hand to its own exec-error pipe.
///
/// `Command::spawn` creates that pipe after the standard streams and before the
/// fork, and it takes the lowest free numbers — the very ones the extra
/// descriptors' sources vacate when they are lifted above their targets. The
/// two lowest free numbers are probed and released here so that the collision
/// is the expected outcome rather than a coincidence: inherited standard streams
/// and null-device sources allocate nothing else in between.
#[cfg(unix)]
#[test]
fn a_failed_exec_is_reported_even_at_the_lowest_free_descriptor_numbers() {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let mut driver = Loop::new(Config::default()).expect("loop");
    let lowest = |from: i32| {
        // SAFETY: duplicates descriptor 0 at or above `from`, nothing else.
        let raw = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, from) };
        assert!(raw >= from, "{}", std::io::Error::last_os_error());
        // SAFETY: fcntl transferred ownership of a fresh descriptor.
        unsafe { OwnedFd::from_raw_fd(raw) }
    };
    let (first, second) = (lowest(3), lowest(4));
    let numbers = [first.as_raw_fd() as u32, second.as_raw_fd() as u32];
    drop((first, second));
    let mut spec = ProcessSpec::new("/nonexistent/turnloop-procspec-fixture");
    spec.stdio = [ProcessStdio::Inherit; 3];
    spec.extra = numbers
        .iter()
        .map(|number| ChildFd {
            number: *number,
            source: ChildFdSource::Null,
        })
        .collect();
    let mut parents = [None; 2];
    let error = driver
        .spawn_extra(&spec, Token(13), &mut parents)
        .expect_err("a missing program cannot be executed");
    assert_eq!(error.kind, ErrorKind::NotFound, "exec failure reached us");
    assert!(parents.iter().all(Option::is_none));
    assert!(!driver.alive(), "no handle survived the failed spawn");
}

/// turnloop reaps only the children it owns, by their own identity. A host that
/// waits for its own child in the same process keeps that child's status, and
/// loses nothing to the loop.
#[test]
fn a_sibling_waiter_and_the_loop_keep_their_own_children() {
    let mut rig = Rig::new();
    let sibling = |code: &str| {
        std::process::Command::new(CHILD)
            .args(["exit-with", code])
            .spawn()
            .expect("sibling child")
    };
    let mut spec = ProcessSpec::new(CHILD);
    spec.windows_hide = true;
    spec.args = vec!["exit-with".into(), "23".into()];
    spec.stdio = [ProcessStdio::Null; 3];
    spec.extra = vec![ChildFd {
        number: 3,
        source: ChildFdSource::Duplex,
    }];

    // The sibling exits first and stays unreaped while the loop reaps its own.
    let mut first = sibling("7");
    let mut parents = [None; 1];
    let owned = rig
        .driver
        .spawn_extra(&spec, Token(8), &mut parents)
        .expect("owned child");
    assert_eq!(
        rig.exit_status(),
        ExitStatus {
            code: Some(23),
            signal: None
        },
        "the loop reported its own child's status"
    );
    assert_eq!(
        first.wait().expect("reap sibling").code(),
        Some(7),
        "the sibling's status survived the loop's reaping"
    );
    rig.close_completely(parents[0].expect("fd 3"), 0);
    rig.close_completely(owned.handle, 0);

    // And the other order: the host reaps first, then the loop delivers.
    let mut second = sibling("11");
    assert_eq!(second.wait().expect("reap sibling").code(), Some(11));
    rig.seen.exit = None;
    let mut parents = [None; 1];
    let owned = rig
        .driver
        .spawn_extra(&spec, Token(9), &mut parents)
        .expect("owned child");
    assert_eq!(
        rig.exit_status(),
        ExitStatus {
            code: Some(23),
            signal: None
        },
        "a host reap did not consume the loop's child"
    );
    rig.close_completely(parents[0].expect("fd 3"), 0);
    rig.close_completely(owned.handle, 0);
}

/// A process identity pinned so that it cannot be confused with a later reuse.
struct Tracked {
    #[cfg(unix)]
    pid: i32,
    #[cfg(windows)]
    process: std::os::windows::io::OwnedHandle,
}
impl Tracked {
    fn pin(pid: u32) -> Self {
        #[cfg(unix)]
        {
            Self { pid: pid as i32 }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::FromRawHandle;
            // SAFETY: the loop's owned leader keeps this descendant's identity
            // alive; only synchronization access is requested.
            let raw = unsafe {
                windows_sys::Win32::System::Threading::OpenProcess(
                    windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            assert!(!raw.is_null(), "{}", std::io::Error::last_os_error());
            Self {
                // SAFETY: a successful OpenProcess transferred this handle.
                process: unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) },
            }
        }
    }
    fn alive(&self) -> bool {
        #[cfg(unix)]
        {
            // SAFETY: signal 0 only probes for the process's existence.
            unsafe { libc::kill(self.pid, 0) == 0 }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: owned synchronization handle and a nonblocking query.
            unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(
                    self.process.as_raw_handle(),
                    0,
                ) == windows_sys::Win32::Foundation::WAIT_TIMEOUT
            }
        }
    }
}

#[test]
fn a_process_group_with_extra_descriptors_still_kills_its_grandchild() {
    let mut rig = Rig::new();
    let mut spec = ProcessSpec::new(CHILD);
    spec.windows_hide = true;
    spec.args = vec!["grandchild".into()];
    spec.stdio = [ProcessStdio::Null, ProcessStdio::Pipe, ProcessStdio::Null];
    spec.new_process_group = true;
    spec.extra = vec![ChildFd {
        number: 3,
        source: ChildFdSource::Duplex,
    }];
    let mut parents = [None; 1];
    let child = rig
        .driver
        .spawn_extra(&spec, Token(10), &mut parents)
        .expect("leader");
    let stdout = child.stdout.expect("leader stdout");
    let text = rig.read_line(stdout);
    let grandchild = Tracked::pin(
        text.trim()
            .strip_prefix("grandchild:")
            .expect("identity")
            .parse()
            .expect("pid"),
    );
    assert!(grandchild.alive(), "grandchild is live before the kill");
    rig.driver
        .kill_group(child.handle, Signal::Kill)
        .expect("kill the whole group");
    let status = rig.exit_status();
    #[cfg(unix)]
    assert_eq!(status.signal, Some(libc::SIGKILL));
    #[cfg(windows)]
    assert_eq!(status.code, Some(1), "TerminateJobObject exit code");
    let deadline = rig.driver.now() + LIMIT;
    while grandchild.alive() {
        assert!(rig.driver.now() < deadline, "grandchild outlived its group");
        rig.settle();
    }
    rig.close_completely(parents[0].expect("fd 3"), 0);
    rig.close_completely(stdout, 0);
    rig.close_completely(child.handle, 0);
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "freebsd"))]
#[test]
fn a_detached_child_can_claim_its_stdin_as_a_controlling_terminal() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let (mut master, mut slave) = (0, 0);
    assert_eq!(
        // SAFETY: two writable descriptor outputs; null term/window settings
        // select the platform defaults.
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    // SAFETY: openpty transferred ownership of two fresh descriptors.
    let (_master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut rig = Rig::new();
    let terminal = rig
        .driver
        .attach(
            Detached::from_fd(slave.try_clone().expect("dup slave")).expect("adopt slave"),
            Token(11),
        )
        .expect("attach slave");
    let mut spec = ProcessSpec::new(CHILD);
    spec.args = vec!["tty-session".into()];
    spec.stdio = [
        ProcessStdio::Handle(terminal),
        ProcessStdio::Pipe,
        ProcessStdio::Null,
    ];
    spec.detached = true;
    spec.controlling_terminal = true;
    let child = rig.driver.spawn(&spec, Token(12)).expect("pty child");
    let stdout = child.stdout.expect("child stdout");
    let text = rig.read_line(stdout);
    let fields: Vec<&str> = text.trim().split(':').collect();
    assert_eq!(fields.len(), 4, "pid:group:session:terminal");
    let pid: i32 = fields[0].parse().expect("pid");
    assert_eq!(pid, child.pid as i32);
    assert_eq!(fields[1], fields[0], "the child leads its own group");
    assert_eq!(fields[2], fields[0], "and its own session");
    assert_eq!(fields[3], "1", "the child has a controlling terminal");
    assert_eq!(
        rig.exit_status(),
        ExitStatus {
            code: Some(0),
            signal: None
        }
    );
    rig.close_completely(stdout, 0);
    rig.close_completely(child.handle, 0);
    rig.close_completely(terminal, 0);
    drop(slave);
}
