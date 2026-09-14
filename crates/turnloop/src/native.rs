//! Portable names and options for native IPC, processes, signals and terminals.
use crate::Handle;
use std::{ffi::OsString, path::PathBuf};

/// Local IPC address: a filesystem socket path on Unix or named-pipe name on Windows.
/// Unix callers own removal of the socket path after closing the listener.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipeName(pub PathBuf);

/// Standard stream to duplicate into a loop. Closing it does not close the host fd.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stdio {
    /// Standard input.
    Stdin,
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// Child-side standard-stream configuration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessStdio {
    /// Inherit the corresponding host stream.
    #[default]
    Inherit,
    /// Connect to the platform null device.
    Null,
    /// Create a pipe; its parent end is returned in [`Process`].
    Pipe,
    /// Duplicate this loop-owned stream into the child.
    Handle(Handle),
}

/// Child launch options. Arguments exclude argv[0], which is the program name.
/// Environment entries override inherited values unless `env_clear` is true.
#[derive(Clone, Debug)]
pub struct ProcessSpec {
    /// Executable path or name resolved using PATH.
    pub program: OsString,
    /// Arguments after argv[0].
    pub args: Vec<OsString>,
    /// Environment additions or overrides.
    pub env: Vec<(OsString, OsString)>,
    /// Start from an empty environment.
    pub env_clear: bool,
    /// Child working directory; None inherits the host directory.
    pub cwd: Option<PathBuf>,
    /// Child stdin, stdout and stderr, in that order.
    pub stdio: [ProcessStdio; 3],
    /// Unix user ID; unsupported platforms reject this option.
    pub uid: Option<u32>,
    /// Unix group ID; unsupported platforms reject this option.
    pub gid: Option<u32>,
    /// Create an isolated process group (Windows Job Object) for tree termination.
    pub new_process_group: bool,
}
impl ProcessSpec {
    /// Launch a program with inherited environment, directory and standard streams.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(), args: Vec::new(), env: Vec::new(), env_clear: false,
            cwd: None, stdio: [ProcessStdio::Inherit; 3], uid: None, gid: None,
            new_process_group: false,
        }
    }
}

/// Owned child identity and optional parent pipe handles, all belonging to one loop.
#[derive(Clone, Copy, Debug)]
pub struct Process {
    /// Process handle; its exit operation carries the spawn token.
    pub handle: Handle,
    /// OS process identifier, for diagnostics rather than lifetime-safe signaling.
    pub pid: u32,
    /// Writable parent end of child stdin, if requested.
    pub stdin: Option<Handle>,
    /// Readable parent end of child stdout, if requested.
    pub stdout: Option<Handle>,
    /// Readable parent end of child stderr, if requested.
    pub stderr: Option<Handle>,
}

/// Portable signal names. Unsupported mappings return an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    /// Interrupt (console Ctrl-C on Windows).
    Int,
    /// Terminate request; no Windows console event equivalent.
    Term,
    /// Unconditional process termination; cannot be subscribed.
    Kill,
    /// Hangup (best-effort console close on Windows).
    Hup,
    /// Child status change. Cooperates with the process dispatcher.
    Chld,
    /// Terminal window size change.
    WinCh,
    /// First user-defined Unix signal.
    Usr1,
    /// Second user-defined Unix signal.
    Usr2,
    /// Console break, where supported.
    Break,
}

/// Child termination status, preserving exit codes and signal termination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    /// Normal exit code, absent for signal termination.
    pub code: Option<i32>,
    /// Native terminating signal number, absent for normal termination.
    pub signal: Option<i32>,
}

/// Terminal input/output mode, restored when the owning handle closes or drops.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TtyMode {
    /// Restore the mode captured when the terminal was opened.
    Normal,
    /// Raw input, retaining terminal signal processing.
    Raw,
    /// Fully raw input/output, including disabling terminal-generated signals.
    Io,
}

/// Terminal dimensions in character cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowSize {
    /// Character columns.
    pub columns: u16,
    /// Character rows.
    pub rows: u16,
}
