# WASI p3: what still prevents a strict bounded-turn claim

Status: 2026-09-14, wasip3 0.8.0+wasi-0.3.0, wit-bindgen 0.61.1,
nightly-2026-09-07, Wasmtime 46.0.0. The backend **must remain behind
`wasi-p3-experimental`**. Passing the current contracts does not prove the
scheduler properties below. No upstream issue or patch was submitted by this lane.

## Implemented and measured

Each loop retains one waitable set. Request slots and canonical return records
remain stable until acknowledgement. Each poll performs bounded guest work and
at most one `waitable-set.wait` or `waitable-set.poll`; it does not construct an
executor, call `block_on`, or loop waiting for a result. Ready work is capped by
the configured output/event budget. Cancellation synchronously quiesces borrowed
buffers before core returns a cancellation completion. UDP return storage remains
owned across turns, cancellations, and multiple live loops.

The idle-socket test actually fires sixty timers (twenty each at 0.5, 2 and 10 ms),
requiring at most two turns and at most one zero-event wait per expiry. It passes
in debug and release. Continuous `Now` turns also make real socket progress in
the TCP/post backlog and concurrent UDP allocation subjects. These are evidence
about the tested runtime, not a universal scheduler bound.

## Precisely unproven

1. **Host progress within a `Now` turn.** In the tested Wasmtime, nonblocking
   wait-set polling alone can starve pending host socket subtasks when guest code
   keeps turning without yielding. The backend calls one `yield_blocking()`
   (component `thread-yield`) before its one nonblocking poll when I/O is pending.
   There is no specified maximum work or scheduling delay before that yield
   returns. Thus one poll per turn does not establish a bounded `Now` return.
   `PollInfo.waits` counts the wait-set operation; it does not claim that yield
   is free, instantaneous, or a second bounded OS wait.
2. **Deadline composition.** A blocking wait joins a clock task at the precise
   deadline. Guest work is bounded, but there is no proof that a ready clock wins
   promptly against arbitrary other host/component work. Ordinary OS scheduling
   noise is expected; unbounded cooperative work inside a host yield is the
   additional concern. Wasmtime's measured ~1 ms wake granularity is a distinct
   issue and does not solve this scheduling question.
3. **Synchronous cancellation latency.** Cancellation currently uses synchronous
   component intrinsics for buffer quiescence. Tests prove ownership and observed
   completion ordering; they do not bound adversarial callee cleanup duration.
   Making cancellation asynchronous would need a core-visible pending
   acknowledgement, without exposing/reusing buffers early.
4. **ABI/context portability.** Raw lowering and context-slot-0 restoration rely
   on the pinned compiler/component linker convention. The debug GlobalAlloc
   entry trap occurs before a turnloop loop exists. The retained canonical
   allocator and allocation gates therefore run in release; debug uses std's
   canonical allocator. Other toolchains, concurrent guest export threads and
   other runtimes have not established compatible context/lifetime guarantees.
   The current storage scope supports multiple loops on one guest agent; it is
   not an implementation of arbitrary concurrent component exports.

## What would close these gaps

The [component concurrency design](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md)
provides wait/poll/yield operations, but it does not give this backend an explicit
host-work budget. Promotion needs either a host-progress primitive that services
already-ready subtasks with an explicit work bound and a deadline, or a normative
bound on the existing yield/poll combination. A poll primitive that leaves ready
host I/O permanently unscheduled is insufficient.

A supported **wit-bindgen persistent step API** could expose a retained task/wait
set, poll one bounded batch, report the next deadline and arrange a host wake.
It must preserve caller-owned return buffers through cancellation, support
allocation-free list lowering into retained storage, and document context and
allocator entry-stack ownership. The current convenience future executor is not
such an API. This is a requested feature, not a claim that an upstream issue
already implements it. The [context convention discussion](https://github.com/WebAssembly/component-model/issues/485)
is relevant to interoperability of guest runtimes and compiler state.

Before removing the feature gate: audit the specified host-work/cancellation
bounds, exercise adversarial unrelated host tasks and cancellation cleanup, run
multiple loop and concurrent-export lifetime tests on every supported runtime,
and rerun no-spin plus the unchanged zero-allocation subjects. No host scheduler
busy-spin workaround or wider turns-per-expiry limit is acceptable.
