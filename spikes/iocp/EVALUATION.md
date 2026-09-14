# compio-driver IOCP source review

Recommendation: **own the thin IOCP implementation; borrow techniques**. This is a
source-level recommendation, not a performance result. All Windows execution and
allocation/cycle measurements are **UNRUN** on the macOS host.

## Reproducible source

Reviewed published **compio-driver 0.12.5** (2026-08-18), compatible with the seven-day
soak, rather than an unpinned repository head. Its `.cargo_vcs_info.json` identifies
commit `1ff9aedeaf69939459fa45aef664d0a7eec856a0`, directory `compio-driver`.
Downloaded crate SHA-256:
`293e8086a35f52b5002402937cf4e69b5e414917e511c29c5e7ba2cebe6ef7b3`.
Source was downloaded for inspection only; compio is not a dependency.

- [Published API](https://docs.rs/compio-driver/0.12.5/compio_driver/struct.Proactor.html)
- [Exact source tree](https://github.com/compio-rs/compio/tree/1ff9aedeaf69939459fa45aef664d0a7eec856a0/compio-driver/src)

## Operations and buffers

`Proactor::push_with_extra` constructs `Key::new` before submitting, including on
immediate success. A pending submission returns `Key<T>`; a ready submission returns
`BufResult<usize, T>`. `poll` records results internally; the consumer obtains its
operation back through typed `pop(key)`. It does not fill a caller-supplied token
completion batch. [Public push/pop implementation](https://docs.rs/crate/compio-driver/0.12.5/source/src/lib.rs)

`ErasedKey::new` creates a `ThinCell<RawOp<Carrier<T>>>`, then initializes self
references after pinning. `RawOp` holds platform `Extra` first, cancellation state,
result/optional waker and the operation/control pair. The first-field layout lets
the kernel's `OVERLAPPED` pointer locate the operation. Keys have reference-counted
ownership; pending kernel work retains a reference. `take_result` requires unique
ownership and moves the operation out. Reusing a buffer does not reuse the key
allocation. [Key storage](https://docs.rs/crate/compio-driver/0.12.5/source/src/key.rs)

`Recv<T: IoBufMut>` and `Send<T: IoBuf>` retain the buffer in the operation; their
control records keep the system slice descriptor stable. Vectored operations build
a `Vec<SysSlice>`. A wrapper for turnloop's externally rooted stable memory is
possible, but it must uphold both libraries' lifetime contracts. The normal receive
path submits a real buffer; turnloop's idle zero-byte probe followed by a pooled
nonblocking read needs a custom operation/second stage.
[Socket operations](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/op/socket/iocp.rs)

The thin-cell dependency's `ThinCell::new` allocates its inner record; there is no
public compio hook to submit a reusable caller-owned operation slab. Separately,
`CompletionPort::poll` constructs `Vec::with_capacity(1024)` on each call. These are
direct source-level conflicts with turnloop's steady-state allocation gate, even
if a token adapter itself uses preallocated tables.
[Thin-cell source](https://docs.rs/crate/thin-cell/0.2.1/source/src/)
[IOCP poll](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/cp/mod.rs)

## Synchronous completion and cancellation

Attach sets both skip-port-on-success and skip-event-on-handle. The socket helpers
return `Poll::Ready` on immediate success, and `Pending` on `ERROR_IO_PENDING`, so
the driver does not await a second packet for a synchronous result. The same source
normalizes several pipe/error statuses into successful zero-byte results. Turnloop
must instead define EOF/truncation/reset handling per operation and preserve OS
errors where the contract requires them.
[Windows result helpers](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/pal/windows/mod.rs)

`OpCode::cancel` for sockets calls `CancelIoEx`; `ERROR_NOT_FOUND` is tolerated.
Pending operation ownership survives until the kernel result. At the public API,
`cancel` sets cancellation state and may return an already completed uniquely owned
operation, or return `None` and request cancellation. This is an ownership-oriented
interface, not an exactly-once `Cancelled` token event. An adapter would need to
retain cancellation bookkeeping and arrange completion reporting after the kernel
has actually finished, including cancel-vs-success races and close ordering.
[Cancellation API](https://docs.rs/crate/compio-driver/0.12.5/source/src/lib.rs)
[Driver cancellation](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/mod.rs)

## Waits, threads and integration

The default IOCP mode uses one port per driver. Its wait truncates `Duration` to
milliseconds and is nonalertable. `poll` can call stored wakers through result
delivery; custom operation methods also run in the driver. Internal-only wakers
could enqueue turnloop tokens, but host code must never be installed there.
That adaptation still cannot remove the upstream allocations.
[Per-driver port](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/cp/multi.rs)
[Driver poll](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/mod.rs)

`iocp-global` adds a process-global collector thread and reposts to per-driver
ports. It addresses routing of handles whose association cannot change, but adds
a thread and a port hop; it does not expose turnloop's auto-reset GUI event.
Default per-port routing also reposts entries whose stored driver differs, which
requires the original port to keep being serviced.
[Global collector](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/cp/global.rs)

`OpType::Event` does exist: the backend maintains a wait map. The default uses
threadpool waits with a boxed callback context; `iocp-wait-packet` uses the NT wait
packet APIs. Thus “no event support” would be an incorrect assessment. Neither
path by itself supplies a high-resolution per-loop deadline plus the host event
integration and shutdown contract. The NT path creates a packet per wait, while
turnloop can reuse one per loop after dequeue.
[Event wait selection](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/wait/mod.rs)
[Threadpool waits](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/wait/thread_pool.rs)
[NT packet waits](https://docs.rs/crate/compio-driver/0.12.5/source/src/sys/driver/iocp/wait/packet.rs)

## Decision and next evidence

Adopting the public backend requires token bookkeeping, cancellation-terminal
reporting, pooled-read staging, precision timers, GUI integration, and allocation
changes upstream. A private fork would own much of the maintenance while retaining
compio's operation model and additional dependencies beyond windows-sys. Own a
small stable-slab backend instead, and borrow the provider-specific extension
lookup, immediate/pending distinction, stable control storage, result conversion,
and explicit callback teardown techniques.

Before changing this recommendation, request public APIs for caller-owned reusable
op storage and completion batches, then measure allocations and cycles on Windows.
No comparative throughput or precision advantage has been measured here.

## Timer choice

Both probes use Windows 10 1803+ high-resolution waitable timers, relative 100ns
deadlines rounded **up**, and no timeBeginPeriod. APC is the documented Win32 path
for direct turns; it must be armed and waited on the same thread and an unrelated
APC can end a turn early. Alertable waiting may execute APCs installed by the host,
so D1's “no user code” guarantee needs an APC caveat or the packet path.
[Timer API](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer)

Prefer the packet path for an opt-in GUI helper, since its consumer can be a
different thread. The Nt* APIs are documented Microsoft devnotes, exported from
ntdll, with no supported minimum OS stated and no SDK header/import library. The
spike dynamically resolves all three and reports Unsupported when absent; it
does not claim an unconditional stable Win32 support contract.
[Creation](https://learn.microsoft.com/en-us/windows/win32/devnotes/ntcreatewaitcompletionpacket)
[Association](https://learn.microsoft.com/en-us/windows/win32/devnotes/ntassociatewaitcompletionpacket)

`AlreadySignaled` does not justify synthesizing another completion. Association
is one-shot and reusable after dequeue. Cancellation's `STATUS_PENDING` does not
authorize packet reuse; the original packet must drain. Kernel holds the wait
object references, and our timer packets carry only opaque integer/null contexts.
[Cancellation](https://learn.microsoft.com/en-us/windows/win32/devnotes/ntcancelwaitcompletionpacket)

Runtime precision/support matrix remains UNRUN: test on the actual minimum Windows
version, current Windows client/server and CI virtual machines before choosing the
production default or claiming sub-millisecond observed precision.
