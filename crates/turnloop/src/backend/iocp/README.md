# Windows IOCP backend

One completion port belongs to each loop. Normal turns have no driver thread.
Requests use core generational IDs, per-direction FIFO queues, fixed operation
storage and a separate pinned kernel slab. Cancellation retains buffers, accepted
sockets and OVERLAPPED storage until acknowledgement; destruction drains pending
I/O before releasing caller memory.

- TCP uses provider-specific AcceptEx/ConnectEx and overlapped receive/send.
  Pooled idle reads arm zero-byte receives, then acquire a lease at readiness.
  Supported IFS handles skip success packets; other providers keep normal IOCP
  notifications. UDP preserves datagram boundaries and source addresses.
- Timed waits use high-resolution waitable timers and dynamically resolved NT
  completion packets. Missing NT exports return an error. GQCSEx is nonalertable
  and has no competing coarse timeout when an exact timer is armed. No global
  timer-period change is made. See the measured decision in
  [WINDOWS_RESULTS.md](../../../../../spikes/iocp/WINDOWS_RESULTS.md).
- Named pipes use overlapped, byte-mode, local-only instances. Socket passing is
  a private cooperative control protocol: WSADuplicateSocket targets the actual
  peer PID queried from the OS, and send acceptance retains independent socket
  ownership. Do not mix payload reads with receive-handle operations on a control
  stream. This is not a general-purpose untrusted wire protocol.
- IOCP association survives duplication. Quiescent transferred sockets and pipe
  endpoints suppress their old port's notifications with the OVERLAPPED event's
  low bit; one-shot registered event waits forward completion to the new port.
  Callbacks are joined before slot reuse. Named-pipe listeners cannot be detached;
  socket listeners and connected pipe endpoints can. Reuse-port is unsupported.
- Child creation uses an explicit inherited handle list, correctly quoted argv,
  Unicode environment, suspended creation and optional Job Object assignment
  before resume. Parent stdio ends are overlapped; child ends are synchronous.
  One-shot process waits publish to the owning notifier. Close terminates live
  owned children, then acknowledges exit before Closed. uid/gid and non-Kill
  process signals return Unsupported; console signal subscriptions are separate.
- Standard streams are duplicated, preserving the host originals. Synchronous
  files/pipes reserve one worker at adoption; operations on a handle are FIFO,
  with no worker allocation needed when the first read receives a buffer lease.
  Imported handles are classified by native file mode before submission;
  overlapped pipes use routed I/O, and other overlapped files are unsupported.
  One process-wide cancellation helper handles the race before a worker enters
  ReadFile, without blocking turns or polling idle handles. Console input workers
  translate key records to UTF-8 and dispatch resize records to WinCh subscribers.
  Resize delivery requires the console input reader to be active. Captured console
  modes are restored on close/drop; input and output VT modes are distinguished.
- Console handlers fan out Int/Break/Hup to independent loop subscriptions.
  Unsupported Unix signals fail explicitly. Hup from CTRL_CLOSE is best-effort:
  Windows can terminate the process before the host next turns the loop.
- Integration::Event opts into a sole-consumer helper with a bounded queue and
  an auto-reset event. Timer resets carry generations so old forwarded packets
  cannot complete a later deadline. Hosts call turn(Now) after their GUI wait.
- Core provides timer semantics, liveness, pooled buffers, the parking handshake,
  shared blocking jobs, external waits and executor behavior.

The tests in `turnloop-contract/tests/windows*.rs` instantiate the same shared
scenarios as Unix, plus native console, GUI, cancellation and handle-count checks.
The shared allocation and executor suites also execute on Windows. Run contracts
serially because console state, process resources and allocation counters are
part of the subjects being measured.
