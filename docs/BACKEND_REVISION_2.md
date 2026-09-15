# Backend revision 2: native services and executor handoff

The core2 lane extends revision 2. The starting `main` already named the trait
revision 2 for `PollInfo::zero_event_waits`; revision 1 remains the `trait-v1`
compatibility reference. This document records the additional source changes.
No tag or commit is created from this read-only Git checkout.

## Boundary changes

The full contract is rustdoc on `turnloop::backend::Backend`. Generational
`Handle`/`OpId`, buffer ownership, cancellation acknowledgements, bounded output,
notifier parking and the no-spin rule are unchanged. The spec-owner-approved
tl-i01b amendment below refines the single-wait rule.

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
| `Backend::raw_transport(handle) -> Result<RawTransport>` | Report a live transport's native identity (Unix fd, Windows SOCKET/HANDLE) for the host's own bookkeeping, Node's `socket._handle.fd`. Read it out of the backend's existing table; touch no operation storage and allocate nothing. Resources with no descriptor identity of their own — timers, processes, signals, watches, and every WASI/web resource — return `Unsupported`. Default is `Unsupported`. |

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

## Blocking waits and nonblocking discovery (tl-i01b)

DESIGN D7 and §10 rule 3 permit at most one OS wait per turn. Queued work
(posts, blocking-pool and external-wait results, synchronous or terminal
completions) prohibits positive-timeout or infinite waits.
One zero-time native discovery poll is permitted only with native operations
pending and native output reserve available. Queued work with no native operation
pending makes no OS call. This retains fresh-I/O fairness through sustained
queued posts/timers, as libuv does with its zero-timeout `uv__io_poll`.

The driver enforces the skip; backends need no new method. On native and WASI
backends a queued turn with no native operation never calls `poll`,
even when `has_work()` reports stale cached readiness, because draining it could
fall through to the OS. The web backend's poll only drains host callbacks and
Worker/condition rings and never enters the OS, so it keeps revision 2's policy of
polling for cached host work. A lookup accepted by `Backend::resolve` (WASI 0.2)
counts as a pending native operation, like socket I/O.

`PollInfo::waits` and `TurnInfo::os_waits` now count only blocking waits;
`discovery_polls` counts zero-time native polls in both types. The two counters
sum to at most one. Callers that meant all invocations must add them; Now-only
benchmarks use `discovery_polls`. `zero_event_waits` retains its revision-2 meaning:
raw empty native calls **across both categories**, including EINTR and private
timeout events, never inferred from user completions. The no-spin gates retain
all previous numerical bounds and count both invocation categories.

Epoll (including timerfd), kqueue, direct IOCP and WASI p2 classify the effective
native timeout. WASI p3 classifies its actual wait-set step (wait versus poll),
including a deadline already completed during setup. A backend's own deadline
source is never native work (tl-i02): epoll drops its timerfd readiness, IOCP its
deadline packet, WASI p2 the deadline pollable it appends after the owned handles
and WASI p3 the deadline subtask it joins to the wait set, so a timeout-only wake
is a zero-event call. Socket, DNS and notifier events in the same call still count. Its existing cooperative
host yield remains part of discovery; the documented experimental scheduler
limitations remain. Web callback draining and IOCP Event-helper queue draining
report zero in both counters: neither performs a native wait/discovery call on
the turning thread. The opted-in helper's independent waits are outside the turn,
just as the GUI host's external wait is. No default helper or new collection API.

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

## Local connection deadlines

`Driver::pipe_connect_until(name, deadline, token)` adds an absolute loop-clock
connection deadline without changing `Open`, `Operation`, or the backend trait.
The core reserves its deadline heap at loop construction, exposes it through
`next_deadline()`, and cancels the native Connect when it expires. TimedOut is
terminal only after native acknowledgement. A completion collected before expiry
wins; explicit cancel/close before expiry keeps the usual Cancelled/Closed result.
Successful, failed and cancelled connects remove their deadlines. Native cancellation
errors keep their identity and the pending deadline for retry. Existing
`pipe_connect` remains unbounded and cancellable. Unsupported local transports
still report Unsupported on WASI/web.

Windows listeners reserve `max(backlog, 1)` pending overlapped instances and re-arm
fixed slots as accepts are consumed. Busy clients use asynchronous NPFS pipe-wait
requests so host turn deadlines and cancellation remain effective. Same-port
accepted/reattached pipes skip registered event waits; foreign-port transfers retain
the bridge. Tests cover backlog bursts, deadlines, close/drop quiescence and direct
versus bridged delivery; see [the follow-up report](lanes/iocp-followups.md).

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
WASI/web use a single-agent registry with the same completion semantics; their
wait deadlines participate in the loop's native/host scheduling. The optional web
Worker bridge uses an Atomics-backed queue to deliver condition updates on the
owner. See [WASI/web revision-2 behavior](wasm.md#revision-2-stdio-external-waits-and-executor).

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

`ProcessSpec::windows_hide` defaults false and is ignored on Unix. On Windows it
selects SW_HIDE; CREATE_NO_WINDOW additionally requires no inherited stdio.
`ProcessSpec::detached` defaults false, creates a new Unix session/process group
or Windows detached process/group, and excludes Windows children from the
process-wide lifetime job. It does not imply unref or relinquish loop ownership:
explicit close and loop Drop still terminate live owned children. WASI/web still
reject process spawning with Unsupported.

Windows lifetime jobs retain a single non-inheritable handle until parent death,
using libuv's silent-breakaway policy. Separate explicit tree-control jobs do not
use KILL_ON_JOB_CLOSE; releasing a normally exited leader preserves grandchildren.
Synchronous Windows handles have independent read/write FIFOs and workers,
including per-direction cancellation and quiescence before Closed/Drop. No
cross-direction ordering of regular-file offsets is promised on Windows.

CTRL_CLOSE follows libuv: an unsubscribed event returns FALSE to older host
handlers; subscribed Hup is queued before the handler thread sleeps for Windows'
bounded close period. That subscribed case prevents older handlers from running
under Windows' newest-first dispatch. The supported API cannot both continue the
chain and hold the handler thread. The host must exit within the OS close budget.

An Event helper pump error remains visible on every later turn/integration call,
including the core's queued-work path. Teardown joins the helper, recovers its
retained packets and drains cancellation with blocking port waits; unrecoverable
port failure aborts instead of spinning or freeing kernel-owned buffers. See
[the issue #11 decisions and verification](lanes/iocp-semantics.md).

The production Windows IOCP backend implements these resource/operation shapes:
named pipe accept/connect, duplicate stdio, duplicated sockets/handles, process waits and Job
Objects, console modes/resize and console signal dispatch. Keep OVERLAPPED storage
pinned through cancellation. Parent stdio pipe ends need overlapped operation;
child ends must work with ordinary synchronous child I/O. The socket-transfer wire
format uses OS-reported peer PIDs with WSADuplicateSocketW; no Unix fd leaks
through this trait. uid/gid and unavailable console signals must reject explicitly.

The new shared scenarios are public generic functions in
`turnloop-contract::{native_surface,executor_contract}`. Windows instantiates
them with IOCP, named pipes and the native child fixture, including the fixture's
actual stdio driver exercise. Windows console and process registration/lifetime
tests complement the shared scenarios; Unix-only openpty/sigaction/waitpid
assertions remain in native tests. Further Windows revision-2 coverage gaps are
listed in the IOCP lane report. Cancelling a Windows child exit watch alone
acknowledges immediately and leaves the child owned and running. Close with a
pending watch retains it until termination, then emits Cancelled before Closed.

WASI 0.2/0.3 and web production adapters implement this revision; WASI 0.3 remains
experimental. WASI stdio, single-agent external waits, Worker condition delivery
and executor contracts run through the required platform runners. Windows IOCP
runs in all three required native CI modes. Cross-checking is not runtime proof;
browser and Windows runtime status remains explicit in the root lane report.

## Descriptor handoff (§5a, issue #35)

`raw_transport` is the only place a native identity crosses the backend boundary,
and it crosses *outwards only*: the core returns the value to the host unchanged
and never acts on it. Ownership does not travel that way. It travels through the
existing `detach`, whose returned `Detached` now also converts into an owned
descriptor — `into_fd` on Unix, `into_socket`/`into_handle` on Windows — instead
of only into another loop's `attach`. That is what lets a host perform Node's
mid-stream `socket.upgradeToTLS` without choosing the transport at creation time.

Nothing in the trait changes for that conversion: `detach` already guarantees a
quiescent, unregistered resource, so the backend has nothing left to release and
the conversion is a move plus whatever the backend restores on adoption (Unix
status flags and termios, Windows console mode). A backend that cannot hand a
resource out keeps refusing in `detach`, as the IOCP backend does for pipe
listeners and connecting pipes.

Windows is the one platform where the handoff has a standing consequence. An IOCP
association cannot be undone, so it travels with the handle; quiescence makes it
inert, and the receiving host uses synchronous/non-blocking calls, tags
`OVERLAPPED.hEvent` with its low-order bit to suppress the completion packet, or
(sockets only) duplicates out of the association with `WSADuplicateSocketW`. An
untagged overlapped call would deliver a packet to the source loop's port with a
foreign `OVERLAPPED`; that loop reports `InvalidInput` from `turn` rather than
dereferencing it, which is the existing `entry` guard, not a new rule.

## Specification clarifications proposed for review

DESIGN §7.3 now reflects the NT timer decision already recorded in §15 question 3.
The following broader clarifications remain proposals.

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
