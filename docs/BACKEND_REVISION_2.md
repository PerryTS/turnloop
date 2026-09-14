# Backend revision 2: native services and executor handoff

The core2 lane extends revision 2. The starting `main` already named the trait
revision 2 for `PollInfo::zero_event_waits`; revision 1 remains the `trait-v1`
compatibility reference. This document records the additional source changes.
No tag or commit is created from this read-only Git checkout.

## Boundary changes

The full contract is rustdoc on `turnloop::backend::Backend`. Generational
`Handle`/`OpId`, buffer ownership, cancellation acknowledgements, bounded output,
notifier parking and the single-wait/no-spin rules are unchanged.

| Addition | Purpose and required behavior |
|---|---|
| `Open::Pipe(PipeName)` | Create an unconnected local stream; `Operation::Connect` establishes it. |
| `Open::PipeListener { name, opts }` | Bind a local stream listener; `Operation::Accept` also handles this resource. |
| `Open::Stdio(Stdio)` | Duplicate stdin/stdout/stderr, retaining the host's original descriptor/handle. Classify pipe, file and terminal. |
| `Operation::SendHandle(Handle)` | Capture independent socket ownership at acceptance, even if the source handle closes before completion. |
| `Operation::RecvHandle` | Receive one owning transport on a dedicated local control stream. |
| `Operation::ProcessExit` | Produce one terminal `Outcome::Exited(ExitStatus)` for the bound process. Native reaping precedes Unix exit completion. |
| `Operation::WatchSignal` | Produce nonterminal `Outcome::Signal(Signal)` deliveries and a terminal cancellation acknowledgement when stopped. |
| `Outcome::PipeAccepted(Detached)` | Transfer an accepted local stream without inventing an IP peer address. The core attaches it to a new handle. |
| `Outcome::HandleReceived(Detached)` | Transfer a received socket; core attachment and cancellation dispose of it exactly as for accepted sockets. |
| `Outcome::HandleSent` | Confirm successful handle transfer. |
| `Backend::set_notifier(Notifier)` | Called once after construction, before registration. Gives services the loop's parking-aware notification endpoint. Default is a no-op. |
| `Backend::spawn(handle, pipes, &ProcessSpec) -> Result<u32>` | Bind the owned process and requested parent stdio handles atomically. The core reserves handles and the exit operation before calling this method. On error release partial state and reap any created child. |
| `Backend::prepare_close(handle)` | Begin nonblocking teardown before core cancellation. A process backend terminates the child/tree and retains its exit registration until reaped; terminal acknowledgement and Closed then follow. Default is a no-op returning success for other resources. |
| `Backend::kill(handle, Signal, group)` | Signal the owned child or its explicitly created process group/tree. Never signal a reused process identity. |
| `Backend::signal(handle, Signal)` | Install a per-loop subscription to the process-wide dispatcher, then accept `WatchSignal`. |
| `Backend::tty_set_mode` / `tty_window_size` | Set/restore terminal mode and query character dimensions. Save original state for final release/drop. |

The five optional native capability methods default to `Unsupported`; no native
success is synthesized. `set_notifier` and `prepare_close` have separate no-op defaults. Backend
implementations with exhaustive `Open`, `Operation` or `Outcome` matches must add
these variants, using explicit capability errors where appropriate. The generic
host test checks failed setup leaves no handles/operations alive.

`OpResult` gains `PipeAccepted`, `HandleSent`, `HandleReceived`, `Exited`, `Signal`
and `ExternalWait`. A process is still an ordinary referenced handle after its
exit completion; close it, or unref it when the host does not want it keeping the
loop alive. `signal_stop` emits `Stopped` then `Closed`. A resize subscription is
an independent signal handle; stop it explicitly when finished with the TTY.

## Native implementation and ownership

- Unix local sockets use filesystem `PipeName(PathBuf)` addresses. Paths reject
  embedded NULs, empty names and overlong `sun_path` values. Hosts remove paths;
  listen does not silently unlink another listener. `reuse_port` is unsupported
  for local streams.
- SCM_RIGHTS uses a one-byte control frame plus one descriptor. Keep normal reads
  off that control stream. Transfer supports TCP streams/listeners, UDP sockets
  and Unix streams/listeners. Received extra or truncated descriptors are closed.
  Sending duplicates ownership; it does not implicitly detach the sender.
  macOS lacks `SO_ACCEPTCONN`, so the frame retains listener metadata; the receiver
  still validates socket family/type. This is an internal trusted-peer protocol,
  not a general-purpose wire format or the Windows wire format.
- `Detached::from_fd(OwnedFd)` adopts native sockets, pipes, files and TTYs. Passing
  a socket through SCM_RIGHTS and subsequently detaching/attaching it preserves
  usable independent ownership. Explicit deregistration matters when duplicated
  descriptors keep the same underlying socket open.
- `open_stdio` preserves the original host descriptor. Duplicates share open-file
  flags and terminal state, so the host must coordinate simultaneous direct I/O
  and mode changes. Final transport close/drop restores captured flags and termios;
  detach moves that restoration responsibility to the receiving owner.
- Regular-file stdio uses the existing bounded blocking pool. Slots, completion
  queues and staging leases are reserved; operations on the same file are FIFO.
  Cancellation of a running job acknowledges only after its buffer access ends.
  Loop drop waits for worker buffer quiescence. Pipe/TTY traffic uses readiness;
  writes protect the calling thread against SIGPIPE without a process-wide ignore.
- `ProcessSpec` uses owned program/argv/environment/directory values, stdio
  inherit/null/pipe/existing-handle options, Unix uid/gid, and isolated groups.
  Linux registers pidfds, with SIGCHLD fallback on unsupported/denied pidfd setup.
  `process-sigchld` forces that fallback for CI; `--all-features` selects it.
  kqueue uses `EVFILT_PROC/NOTE_EXIT`. A shared SIGCHLD subscription also closes
  the interval where NOTE_EXIT is visible before wait status becomes reapable.
  Already-exited children are checked before treating registration failure as an
  error. Only owned child PIDs are reaped; never `waitpid(-1)`.
- Closing a live child starts termination (including its group when requested),
  then retains the native exit registration until reaping. Cancelled precedes
  Closed, and release performs no blocking reap. Cancelling an exit registration
  alone retains it until the child is reapable; close also initiates termination.
  Dropping the loop may synchronously wait for OS teardown. Ordinary process/signal
  polling uses readiness and never polls on a fixed interval. Kill after the
  leader's exit was reaped returns `NotFound`, avoiding a reused PID/PGID. Hosts
  must request group termination while the leader is still owned and unreaped.
  Hosts must not reap turnloop children independently or replace subscribed
  signal dispositions. Spawn rejects inherited SIGCHLD ignore/`SA_NOCLDWAIT`.
- There is one lazy process-wide signal thread. kqueue uses EVFILT_SIGNAL; Linux
  uses an async-signal-safe self-pipe. Only subscribed signals have handlers;
  dropping the last subscription restores the prior sigaction. Child ownership
  counts as a SIGCHLD subscription. Standard signals may coalesce, but every
  subscribed loop has its own pending flag and notifier.
- `TtyMode::Normal` restores the captured mode; `Raw` retains ISIG; `Io` disables
  terminal-generated signals too. `tty_resize_start` produces `Signal::WinCh`;
  query `tty_window_size` on notification. The openpty test compares full restored
  termios after XNU applies its pending canonical-input transition.

## External waits

`WaitCondition` wraps host-provided `Arc<AtomicU64>` storage and a notification
generation. `external_wait(condition, expected, deadline, token)` reserves a core
operation and submits to one process-wide helper with 16,384 fixed slots. Initial
inequality completes `NotEqual`; a notification/value change completes `Notified`;
an exact deadline completes `TimedOut`. `notify` wakes current registrations even
if the value did not change. Hosts mutating the supplied atomic must call `notify`.
Registration/notification/parking share one lock to prevent lost wakes. Completion
publication uses the waiting loop's existing work queue and notifier. Cancellation
and loop drop remove registrations; no thread or allocation is created per wait.
WASM returns `Unsupported` for this native thread service. Host callback/worker
providers can add an equivalent API implementation without JavaScript in the core.

## Executor

Enable `executor` for `LocalExecutor<B>`, `ExecutorHandle`, `AsyncIo`, `Sleep`,
`executor::Timeout`, `Accept` and cancelling `JoinHandle`. The sole new dependency
is the optional pinned `futures-io = 0.3.31`; `std::future`/`std::task` supply the
remaining machinery. No executor, scheduler or futures utility crate is added.

The host still calls `turn`. Futures run in bounded passes before and after the
underlying driver turn, never from inside it. Task wakes only mark tasks runnable
and notify the owning loop. Tokens reserve bit 63 and encode slot generation;
the executor consumes completions, so drive unrelated raw operations separately.
`driver()` supports synchronous resource creation/configuration.

Tasks and join state allocate at spawn; task polls do not allocate. Operation
slots and staging buffers are fixed by `ExecutorConfig`. Borrowed caller buffers
are never retained across Pending. Reads preserve unread staging bytes when a
later caller supplies a smaller buffer. Writes are buffered: acceptance returns
Ready with the copied byte count, and flush/close/the next write observes native
completion and delayed errors. Flush before drop to preserve buffered output.
A Pending write consumes no current caller bytes. UDP writes represent one whole
datagram and reject oversized staging; the AsyncRead view omits sender addresses.

Dropping a task/adapter/future requests cancellation and retains operation storage
until terminal acknowledgement. Dropping an unconsumed completed accept closes
its received socket. Join cancellation resolves `Err(JoinError::Cancelled)`,
including when cancelled before the first poll. `LocalExecutor` and adapters are
`!Send`; cloneable wake endpoints remain thread safe.

## Windows and WASM integration

Windows has the same resource/operation shapes needed by `spikes/iocp`: named pipe
accept/connect, duplicate stdio, duplicated sockets/handles, process waits and Job
Objects, console modes/resize and console signal dispatch. Keep OVERLAPPED storage
pinned through cancellation. Parent stdio pipe ends need overlapped operation;
child ends must work with ordinary synchronous child I/O. The wire format may use
peer PID negotiation for WSADuplicateSocketW/ DuplicateHandle; no Unix fd leaks
through this trait. uid/gid and unavailable console signals must reject explicitly.

The new shared scenarios are public generic functions in
`turnloop-contract::{native_surface,executor_contract}`. Instantiate them with
IOCP and supply a named-pipe name, the child fixture executable, and a supported
signal plus its real OS delivery closure for fan-out. The no-spin scenario also
accepts the platform's idle signal. Adapt the fixture's platform alias/cfg once
production IOCP exists; do not substitute ordinary stdio for its driver exercise.
Unix-only openpty/sigaction/waitpid assertions stay in native tests. IOCP needs its
console and process registration race equivalents in addition to the shared tests.

WASI 0.2/0.3, web and Windows common core/executor compile without a production
backend. Their existing spikes remain separate, unchanged projects. Passing a
cross-check does not claim runtime support: platform contract jobs must execute
nonzero tests after those adapters land.

## Specification clarifications proposed for review

No changes were made to authoritative `DESIGN.md`.

1. Specify local control-stream framing and independent send ownership explicitly.
2. Specify nonblocking kill/reap-on-close via `prepare_close`, delayed cancellation
   of exit registrations until reapable, and potentially blocking loop destruction.
   Both close and destruction guarantee no owned child is left as a zombie.
3. Define the subscribed-signal disposition ownership contract and coalescing;
   include SIGCHLD as an internal subscription while children exist.
4. Record `Raw` versus `Io`, resize subscription lifetime, and restoration ownership
   across detach/attach, including shared Unix open-file state.
5. Specify buffered futures-io writes, flush/close, caller-buffer cancellation and
   the external-wait condition/deadline API. Resource setup (spawn, subscription,
   executor/task construction) allocates; the §10 read/write/accept/timer gates and
   new steady completion/wait gates remain zero allocation.

Primary platform references: [pidfd_open](https://www.man7.org/linux/man-pages/man2/pidfd_open.2.html),
[kevent](https://man.freebsd.org/cgi/man.cgi?n=1&query=kevent&sektion=2),
[Apple socket options](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/getsockopt.2.html),
[XNU terminal implementation](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/tty.c),
[futures-io AsyncWrite](https://docs.rs/futures-io/latest/futures_io/trait.AsyncWrite.html).
