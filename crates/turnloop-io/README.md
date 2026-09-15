# turnloop-io

Shared, runtime-free async glue for turnloop protocol adapters. TCP and pipe
`AsyncIo` handles and `TlsStream` implement the same `Stream` trait. The host owns
`LocalExecutor` and calls `turn`; adapters never drive the loop or start threads.

## Getting started on turnloop

Enable a protocol crate's `turnloop` feature. Create an executor, clone its handle,
and spawn a local task. Connect with `handle.connect(address, options).await` or
accept with `Listener::bind(&handle, address)?.accept().await`. Use `deadline` with
one absolute backend-clock deadline for a whole exchange.

## Adapter pattern

1. Keep the sans-I/O core and its retained buffers in the connection.
2. Feed only bytes actually received. Preserve unconsumed bytes across events and
   upgrades. On no progress, await a transport read; never wake/poll an idle core.
3. Use `drain` to write core output, flush, then acknowledge it. Do not clear output
   while delivery is pending. Partial writes and errors retain ownership.
4. A request owns its connection lease until its response body is fully consumed.
   Dropping an incomplete exchange closes the stream rather than returning a
   partially consumed protocol to the pool. AsyncIo cancellation retains submitted
   staging until the backend's exactly-once terminal completion.
5. Allocate retained buffers at connection creation/warm-up, never per I/O call.
   Application-owned headers/messages and crypto state have separate accounting.

## Half-close and lingering close

`close` releases the whole transport. `shutdown` (the `HalfClose` trait, implemented
by `AsyncIo`, `TlsStream` and `Transport`) flushes accepted writes and ends only the
write direction: TLS sends close_notify, then TCP `shutdown(SHUT_WR)` (IOCP
`SD_SEND`, WASI `shutdown(send)`). Reads continue until the peer's EOF and the handle
stays open until `close` or drop. UDP, Windows named pipes and non-socket Unix
descriptors return `Unsupported`.

A server should not close a socket while peer bytes are unread: that sends RST, and
on macOS and Windows a received RST discards data the peer has not read yet (the
tail of the final response). `linger_close(stream, scratch, deadline)` flushes and
half-closes, reads and discards into caller-retained scratch until the peer's EOF
or the deadline, then closes. It waits only on one read and one executor timer (no
spin) and allocates nothing per read; `Lingered` reports how it ended and how many
bytes were discarded.

Native TCP/pipes and WASI sockets use the identical generic stream code. Browser
raw sockets/listeners return Unsupported; browser HTTP uses host fetch. Windows
uses the production Backend implementation when integrated.

## Pools and DNS

`pool::Pool` schedules the existing protocol pool policies with one manager task,
retained waiters and real executor deadlines. A lease owns the connection across
awaits. Cancelling a waiter removes its request; returning an incomplete exchange
retires the transport. Pool replacement/end observes the backend's physical close
acknowledgement through `ExecutorHandle::close`.

`resolve` uses native blocking DNS or WASI 0.2 `ip-name-lookup`. `dns::query` returns
native SRV/TXT records through `ExecutorHandle::blocking`, keeping system resolver
calls off the loop thread. Native worker count/queue bounds remain those of the
host's turnloop configuration. WASI exposes no SRV/TXT capability; use resolved
seeds when the protocol needs those records.
