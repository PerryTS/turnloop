# Backend draft (compiled by the standalone spike)

`trait-v0` was absent at initial inspection and after completion of the mechanism
spikes/source review. DESIGN §6 contains a public Loop sketch, not an internal
Backend trait. This draft therefore fixes the Windows ownership and completion
boundary without pretending that a guessed trait is core's agreed interface.

Implemented: stable preallocated OVERLAPPED slab, generational operation/handle
identities, caller-owned read/write buffers, TCP WSARecv/WSASend, internal zero-byte
read stage, ReadFile/WriteFile/ConnectNamedPipe, synchronous completion queue,
cancel-vs-success normalization, cancel-before-Closed ordering with output
backpressure, one wait per direct turn, APC deadline, parked notifier and opt-in
event helper. No heap allocation on read/write/turn after construction. Capacity
exhaustion rejects before submission. Windows tests assert actual transfers,
allocation count, notifier syscall count, and cancellation order; all **UNRUN**.

AcceptEx/ConnectEx, child process/job/wait, synchronous stdio and console mechanisms
are implemented in the sibling spike modules and have their own test binaries.
They are not yet public operations of this draft. The draft is a compile-checked
integration starting point, **not a complete windlass Loop or shipping backend**.

## Adaptation required when core tags

1. Replace `Handle`, `OpId`, `Token`, `Completion`, `ResultKind` and `TurnInfo` with
   core's generational identities, backend events and error representation. Preserve
   the distinction between rejected submissions and accepted immediate completion.
2. Embed/reuse the stable kernel storage according to core's operation table
   ownership. The current boxed slice cannot move/grow while operations are pending.
   Keep slots until their completion has reached the caller, not just CancelIoEx.
3. Use core's timer heap and effective timeout; current draft handles the wait
   deadline only. Core emits logical timer completions. Decide APC caveat vs NT
   packet default using EVALUATION.md and Windows precision results.
4. Replace the draft wake handshake with core's loom-checked notifier interface.
   Event mode must stay parked while the GUI host owns its wait, including before
   the first turn. Internal queued work must re-signal on partial output drains.
5. Map `IdleRead -> ReadReady` to a **private** follow-up: acquire core's pooled
   buffer, nonblocking recv, re-arm on WSAEWOULDBLOCK, then emit Read/Eof. Never expose
   ReadReady in the public completion API. Lease sizing/lifetime belongs to core.
6. Port the AcceptEx/ConnectEx state machines from `src/tcp.rs`; retain the extra
   accept socket/address storage and install SO_UPDATE_ACCEPT/CONNECT_CONTEXT
   before the connection is surfaced. Secondary accept-socket ownership must also
   survive cancel/close. Make accept rearm use slab storage, not a new allocation.
7. Hook pipe listener instance creation and client-first completion handling into
   core's handles. Windows named-pipe listening uses instances, unlike Unix accept.
8. Wire `src/process.rs`, `src/stdio.rs`, `src/console.rs` to core's per-loop post
   queues and process-wide services. The probe queues are bounded mutex queues,
   not core's lock-free Poster; no callback may settle host work. Keep one-shot
   process waits and their unregister barrier. Console close is only best-effort.
9. Liveness/ref/unref counters, multishot rearm/terminal events, lease pools, cross-
   loop transfer and queue model checking remain core/integration responsibilities.
   Draft liveness scans handles and is explicitly not the O(1) public contract.
10. The helper currently requires turn(Now). Deadline composition belongs to the
    host or a reusable NT packet timer; copy the demonstrated helper/timer pairing.
    An IOCP handle must never be returned as the host's waitable Event handle.

Direct Drop cancels/drains before resource release and can block. A broken port
during teardown aborts because returning could let the host free kernel-owned
buffers. Production shutdown needs an explicit contract for an unrecoverable driver
failure; do not replace this with premature frees. No support for rebinding an
already-associated handle is claimed; Windows association survives duplication.
