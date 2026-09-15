# tl-i02 — WASI deadline-event accounting

Status: **implemented; macOS arm64 and Wasmtime 46 (p2 debug/release, p3
debug/release) verified locally. Linux, Windows and web runtime await the
integrator's CI.** Base `3cce439` (main), branch `lane/tl-i02`, commit `056a1c2`.
Root `LANE_REPORT.md` and the audit documents are byte-identical to `3cce439`.

This is an accounting defect, as the audit states. No WASI test spun before the
fix; the counter simply could not report a timeout-only wake, so DESIGN §10 rule
4a's "zero-event OS waits ≤ 1 per expiry" bound was vacuous on both WASI
backends: the quantity it bounds was always zero.

## Root cause per backend

**WASI 0.2** (`crates/turnloop/src/backend/wasi_p2.rs`). `poll` builds a parallel
pair of vectors: `handles` (the pollables passed to `wasi:io/poll`) and `owners`
(one `PollOwner` per *owned* handle — a socket direction or a DNS lookup). The
private deadline pollable is pushed onto `handles` **after** the owned ones and
deliberately gets no `owners` entry, so the dispatch loop ignores it via
`self.owners.get(index) => None`. The `PollInfo` argument, however, was
`self.indices.is_empty()`: every returned index counted, the deadline's included.
A quiet expiry returned exactly one index — the deadline — so the wait reported
`zero_event_waits == 0`, i.e. "this wait found native work".

**WASI 0.3** (`crates/turnloop/src/backend/wasi_p3.rs`). `poll` lowers
`wasi:clocks/monotonic-clock.wait-until` into a subtask and, when it does not
complete during setup, joins that subtask to the wait set. The step's
`empty` argument was `kind == 0`, i.e. "the wait set returned no event at all".
A quiet expiry returns exactly one event whose waitable *is* the private deadline
subtask, so `kind != 0` and the wait again reported `zero_event_waits == 0`. The
code already knew how to recognize that event — it used
`kind != 0 && waitable == task && code >= 2` to decide whether the subtask still
needed cancelling — but never fed that knowledge to `PollInfo`.

Native IOCP (`backend/iocp/mod.rs:1437`) and epoll's timerfd branch
(`backend/epoll.rs:241`) already normalize their own deadline source; kqueue's
timeout is the `timespec`, which never produces an event. Only the two WASI
backends were out of line with the `PollInfo` contract in `backend/mod.rs`.

## The fix

- **p2** (`wasi_p2.rs:684,693,697,704`): count native events in the existing
  dispatch loop — one per index that resolves to a `PollOwner::Socket` or
  `PollOwner::Dns` — and pass `native_events == 0` as `empty`. The deadline is
  the only handle without an owner, so it is excluded by construction, and a
  socket or DNS index arriving *in the same result* as the expiry still makes the
  call non-empty. No extra pass, no allocation, no change to dispatch.
- **p3** (`wasi_p3.rs:604,610,621,639`): hoist the existing recognition into
  `deadline_event` (`kind != 0` and the event's waitable is this turn's deadline
  subtask), reuse it for the unchanged cancellation decision, skip the operation
  scan for it (the deadline matches no operation, so this is equivalent and
  cheaper), and pass `kind == 0 || deadline_event` as `empty`. A step that
  returns a socket subtask event is still native work; any simultaneous expiry is
  left to the timer wheel, which reads the clock rather than this event.
- Contract text updated where it is stated: `backend/mod.rs` module rustdoc and
  the `PollInfo::zero_event_waits` doc now name all four private deadline sources
  (timerfd, IOCP deadline packet, WASI 0.2 pollable, WASI 0.3 subtask) and say
  that simultaneous I/O or notifier events keep the call non-empty. The same
  sentence is restated in `DESIGN.md` §10 rule 4a, `docs/BACKEND_REVISION_2.md`
  and `CONTRIBUTING.md`.

Nothing in the driver changed. `PollInfo::native`'s `waits`/`discovery_polls`
classification from tl-i01b is untouched: a private deadline wake is an empty
*blocking* wait, which is exactly what the split already expresses.

## Tests

**Shared contract** `turnloop_contract::quiet_deadlines` (`src/lib.rs:1316`) and
`quiet_deadline_accounting` (`src/lib.rs:1396`).

`quiet_deadlines(l, out, native_pending)` runs the three design delays
(0.5 ms / 2 ms / 10 ms) twenty times each against the caller's loop and
completion storage and returns `[os_waits, discovery_polls, zero_event_waits,
expiries]`. Per round it requires:

- `os_waits + discovery_polls == 1` and `zero_event_waits == 1` — exactly one
  native call, and it observed no native event. **This is the I02 assertion**;
  both unfixed backends report `zero_event_waits == 0` here.
- exactly one turn per expiry (`out.len() == 1` immediately after the wait), with
  the timer's handle, operation id and token, and `OpResult::Timer`;
- the deadline honoured exactly — `now >= at`, with lateness under 100 ms (the
  bound `timer_precision` already uses), so no wait floor rounds a 0.5 ms delay
  up and no wait returns early;
- the queued `Closed` turn makes no blocking wait, and a discovery poll only when
  a native operation is pending (`discovery_polls <= native_pending`), with
  `zero_event_waits <= discovery_polls`.

A round whose own setup is preempted past its delay leaves the expiry already due
at turn entry, which DESIGN §10 rule 3 lets spend the turn's one call on a
zero-timeout poll. Such a round still asserts the full I02 property (one call,
zero-event) but is **re-run instead of counted**, bounded at 16 consecutive
attempts. That is why the totals can be required exactly rather than as a `≤`
bound: a backend that only ever polls can never reach sixty blocking waits, and
16 consecutive failures at one delay fail the test outright. This matters in
practice — under an 8-way CPU load the p3 release binary hits a due-at-entry
round about 20 % of the time at 0.5 ms; a pre-turn `now < at` prediction was
raced and discarded (measured: 2/10 runs failed with `(1, 1, 0)`).

`quiet_deadline_accounting` runs that helper twice on one loop — idle, then with
a registered-but-idle pooled read on a loopback pair — asserts the two results
are **identical** and equal to `[60, 0, 60, 60]`, and then finishes with the
control the accounting needs: the *same* still-pending read takes real bytes
inside a deadline-bounded wait, and the exchange must make at least one native
call with **zero** zero-event waits. Both phases and the exchange are exercised
on every backend that instantiates it.

Instantiated at:

| Suite | Test |
|---|---|
| `crates/turnloop-contract/src/lib.rs:238` (native unit tests) | `native::quiet_deadlines_account_identically_idle_and_registered` |
| `crates/turnloop-contract/tests/wasi.rs:147` (p2 and p3, debug and release) | `quiet_deadline_accounting` |
| `crates/turnloop-contract/tests/windows.rs:20` | `quiet_deadline_accounting` |

**Allocation gate.** `quiet_deadline_waits_have_identical_accounting_without_allocations`
(`tests/allocations.rs:1111`) lost its `#[cfg(not(target_os = "wasi"))]` and now
runs on both WASI targets as well as the three native OSes. Its body now calls
the shared helper twice under the counting allocator — idle, then with a
registered-but-idle read — and asserts identical `[60, 0, 60, 60]` results and
zero allocations in **both** phases. Its previous assertions are preserved
exactly: `(os_waits, discovery_polls, zero_event_waits) == (1, 0, 1)` per expiry
(now expressed as one native call plus `os_waits == 1` for every counted round)
and `(0, 0, 0)` for the queued close in the idle phase.

No test was weakened. No numeric bound moved. `no_spin`'s `≤ 2 turns` /
`≤ 1 zero-event wait` per expiry is unchanged and now measures a live quantity on
WASI; so do `external_wait_deadlines_do_not_spin` (`tests/wasi.rs`), the
`turnloop-io` lingering-close bounds and the HTTP keep-alive bounds — all of them
saw `zero_event_waits == 0` from WASI before and now see the real count.

### Both failure directions proved

Every mutation below was applied to the committed tree, built and run; the fix
was restored immediately afterwards.

| Mutation | Suite | Result |
|---|---|---|
| p2 `native_events == 0` → `self.indices.is_empty()` (the pre-fix expression) | `wasi` contract, wasm32-wasip2 debug | **FAIL as intended**: `500µs deadline: one native call observing no native event; left: (1, 0) right: (1, 1)` |
| same | `allocations`, wasm32-wasip2 release | **FAIL as intended**, same assertion |
| p3 `kind == 0 \|\| deadline_event` → `kind == 0` (the pre-fix expression) | `wasi` contract, wasm32-wasip3 debug | **FAIL as intended**, same assertion |
| same | `allocations`, wasm32-wasip3 release | **FAIL as intended**, same assertion |
| p2 and p3 `empty` → `true` (over-normalize: call every wait empty) | `wasi` contract, both targets | **FAIL as intended**: `a native call that carried real I/O is not a zero-event wait; left: 1 right: 0` |

The first four prove the tests fail without the fix. The last proves they are not
satisfied by blanket-emptying every call, i.e. the backends still distinguish
simultaneous real I/O.

## Verification

Tree `056a1c2`, macOS arm64, `CARGO_BUILD_JOBS=4`. Plain `cargo` is
nightly-2026-08-20; WASI 0.3 uses nightly-2026-09-07. wasmtime 46.0.0 (the
`scripts/ci/tools.json` pin) was linked into this clone's `.tools/bin` from an
existing local install.

| Command | Result |
|---|---|
| `cargo fmt --check` | **PASS** |
| `python3 scripts/ci/check-paths.py` | **PASS** (1490 tracked files, 257 references) |
| `python3 scripts/ci/feature_modes.py` | **PASS** (6 features / 18 native arms) |
| `git diff --check 3cce439..HEAD` | **PASS** |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS** |
| same with `--all-features` | **PASS** |
| same strict flags, `-p turnloop -p turnloop-contract -p turnloop-io --all-targets`: x86_64-unknown-linux-gnu (default, `turnloop/epoll-timerfd`, all-features), x86_64-pc-windows-msvc (default, all-features), wasm32-wasip2 (default, all-features), wasm32-unknown-unknown (default, all-features), wasm32-wasip3 all-features | **PASS** (10 invocations) |
| wasm32-wasip3 **default** features, same package set | **FAIL, inherited**: `method read_buffer is never used` in `turnloop` (lib). Reproduced on unmodified `3cce439` in a scratch worktree; CI clippies p3 with `--all-features`, which passes |
| `cargo +stable check --locked --workspace --all-targets --all-features` | **PASS** |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --locked --workspace --all-features --no-deps` | **PASS** |
| `cargo test --locked --workspace -- --test-threads=1` | **PASS**, exit 0, 285 tests |
| `python3 scripts/ci/run-tests.py native` (default, executor, all-features; workspace + every member) | **PASS**, exit 0, 1645 tests |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | **PASS**, exit 0: core lib 11, contracts 34 debug + 34 release, allocations 12 |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | **PASS**, exit 0: core lib 13, contracts 34 + 34, allocations 13 |
| `bash scripts/ci/no-tokio.sh` | **PASS** (all graphs, default + all features) |
| `python3 scripts/ci/soak.py` | **PASS**: 251 locked versions; inherited rustls 0.23.45 exception only, expires 2026-09-21 |
| Stability: new contract test, p2 debug wasm, 12 runs under 8-way CPU load | **PASS** 12/12 |
| Stability: allocation gate, p3 release wasm, 15 runs under 8-way CPU load | **PASS** 15/15 (2/10 before the due-at-entry re-run was added) |
| Stability: native contract test and native allocation gate, 15 runs each under load | **PASS** 30/30 |
| `python3 scripts/ci/run-tests.py native` on Linux x86_64/arm64 (all modes incl. timerfd/SIGCHLD fallbacks) | **UNRUN**, no host |
| `python3 scripts/ci/run-tests.py native` on Windows (incl. the new `quiet_deadline_accounting` arm) | **UNRUN**, no host |
| `python3 scripts/ci/run-tests.py web` / `node` | **UNRUN**: `wasm-bindgen-test-runner` not installed. The web contract lists are unchanged; cross-clippy for wasm32-unknown-unknown compiles the new code |
| `python3 scripts/ci/run-tests.py protocol` / `protocol-wasi` with real servers | **UNRUN**, not affected by the change |
| `cargo run --release -p turnloop-bench -- --portable` | **UNRUN**, not affected: the bench runs on the native backend, which this change does not touch |
| `python3 scripts/ci/check-ci.py` | **UNRUN**: requires `GH_TOKEN` |

## Needs CI confirmation / notes for the integrator

- **Windows:** `quiet_deadline_accounting` is new in the `contract!` list in
  `tests/windows.rs` and is compiled by cross-clippy only here. IOCP already
  normalizes its deadline packet, so the expected result is a pass; the arm that
  needs a real look is the Event-helper mode, where a turn makes neither kind of
  call and the helper's own wait is outside the turn — that mode never reaches
  `quiet_deadlines`' blocking-wait branch, and if a GUI-host configuration does,
  its counts are the thing to read.
- **Linux:** both epoll modes. The timerfd fallback normalizes its private timer
  already (`epoll.rs:241`); `epoll_pwait2` returns zero on timeout, so both
  should give `[60, 0, 60, 60]`. This is the first test that requires that
  exactly, per expiry, rather than as a `≤` bound.
- **The audit documents were deliberately not edited.** `docs/DESIGN_AUDIT.md:141`
  still records the defect ("WASI counters count private deadline events as
  native work (`wasi_p2.rs:684`, `wasi_p3.rs:619`), contrary to
  `backend/mod.rs:204`") and `docs/REMAINING_WORK.md` still carries I02 in its
  audited form, following the tl-i01b precedent of leaving the audit as written.
  Both rows are now stale and are the integrator's to refresh.
- **Not in scope, observed while reading p3:** when a socket subtask event and
  the deadline complete in the same wait-set step, the step returns one of them.
  If it returns the socket event, the expiry is still delivered in the same turn
  by the timer wheel (it reads the clock, not the event), and the call correctly
  counts as native work. No fairness question arises at the two-event scale, but
  a deadline storm against many ready sockets is I04's territory, not measured
  here.
