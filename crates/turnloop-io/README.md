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

Native TCP/pipes and WASI sockets use the identical generic stream code. Browser
raw sockets/listeners return Unsupported; browser HTTP uses host fetch. Windows
uses the production Backend implementation when integrated.
