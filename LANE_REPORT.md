# core3 lane report

Work in progress. Scope: idle instruction regression, child registration ESRCH,
and signal fan-out/unsubscribe races. Starting revision: 9e9d8aa.

Read DESIGN.md and CONTRIBUTING.md completely, integration report, core/CI lane
reports and core2 command history. No applicable AGENTS.md. The inherited ci-fix4
report is preserved in `.tools/core3/inherited-ci-fix4-report.md` and Git history.

## Findings and implementation

- Replaced Services::has_work/poll and Files::has_work/start capacity scans with
  reserved readiness queues; idle checks are O(1). File pool backpressure keeps
  a separate queue, resumed only on lease availability; cancellation can progress
  without a lease. Signal publication is coalesced per handle and queue generations
  are removed after dispatcher unsubscribe synchronizes. A new loom model covers
  publication, coalescing and slot reuse.
- ESRCH during kqueue/pidfd registration now waits for/reaps only the owned exiting
  PID during spawn, caches its normal status and delivers one terminal completion.
  No waiting loop was added to turn. Regression injects ESRCH while WNOHANG reports
  no status, delays normal exit, and asserts token/op/status plus final ECHILD.
- Kqueue now uses SIG_IGN for ordinary subscribed signals, and SIG_DFL for SIGCHLD
  (default ignore preserves child wait status). The original handler is restored
  under the subscription registry mutex only after the last subscriber leaves.
  Kqueue observes generation before ordinary signal delivery. Previously all four
  recipients could unsubscribe and restore SIG_DFL before the queued handler ran.
  A blocked-delivery subprocess deterministically dies on the original code and
  survives after the fix. Each stress test performs 256 four-loop fan-out rounds.
- The first repeated-test campaign exposed the same window for the caught SIGCHLD
  handler (run 5). Its default-ignore disposition now avoids ordinary pending
  handler delivery throughout the subscription; the campaign restarted at zero.
- New file allocation/backpressure regression found Darwin std mutexes allocating
  lazily for previously unused operation slots. Mutex storage is reserved at loop
  setup, including the shared service queue. All zero thresholds remain unchanged.
- Callgrind's owned Loop arguments were dropped inside each measured function.
  Return them to explicit unmeasured teardown: disposal is a one-time lifecycle
  cost, not 100 idle/notify/timer operations. The Linux baseline remains unchanged;
  integrator records/reviews any boundary-corrected candidate on real Linux CI.
- Preliminary macOS cgu=1: idle ~58.4–58.9k -> ~11.7k instructions/turn; notify
  ~58.2–58.9k -> ~11.7k; timer cancellation/delivery ~14.37k -> ~2.1k. Capacity
  probes confirm flat final idle cost versus prior linear growth. Final interleaved
  measurements and retained raw evidence are pending completion of reliability runs.

## Verification

See [complete command ledger](docs/core3-commands.md). Final contract reliability: **50/50 consecutive PASS runs**, 52 tests each, zero
failures. Every run includes 256 four-loop signal rounds and 256 distinct child
exits. Native all-feature Clippy and the new allocation gate PASS. The ten
all-feature workspace repetitions are running; remaining cross-target checks,
stable, loom, Miri, no-tokio and soak are pending. Linux Callgrind and native
Linux/Windows runtimes are UNRUN (no host). No instruction baseline or gate changed.

## Deviations / questions / next steps

No dependency changes, soak overrides or DESIGN changes. Implement event-driven
service/file scheduling, deterministic race regressions and stress tests, then
measure before/after and run all required verification. Integrator commits the
working tree because .git is read-only.

## Source evidence / proposed specification clarification

The [XNU signal implementation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_sig.c)
issues NOTE_SIGNAL before checking ignored/caught disposition in psignal; its
postsig_locked default branch assumes fatal delivery. The [FreeBSD implementation](https://github.com/freebsd/freebsd-src/blob/main/sys/kern/kern_sig.c)
also issues the kqueue notification before its ignored-signal check. Both preserve
child wait status under SIGCHLD's default disposition. This explains why merely
serializing subscription bookkeeping cannot make a no-op handler safe on kqueue.
Proposed §7.2 clarification: use SIG_IGN for subscribed ordinary signals and
SIG_DFL for SIGCHLD, whose default is ignore without auto-reaping. DESIGN.md itself
is unchanged. Linux retains the async-signal-safe self-pipe handler.

## Reliability progress

- Final contract campaign: PASS **50/50**, zero failures, 52 tests per run.
  Signal churn alone proves 51,200 deliveries across 12,800 four-loop rounds.
  The concurrent-child contract proves 12,800 distinct reaped exits.
- All-feature workspace campaign: first five runs PASS, 237 test passes each;
  continuing to ten. `RUST_TEST_THREADS=1` follows CONTRIBUTING's required
  process-wide signal and allocator isolation.
- External-service ignored tests are UNRUN in these commands. SQL initialization,
  browser sandbox startup, Linux/Windows native runtimes, and Callgrind are not
  claimed as tested by a native Rust suite or cross-compilation.
