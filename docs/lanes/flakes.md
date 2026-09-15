# flakes — two CI flakes, `timer_precision` (#30) and the Windows handle count (#26)

Status: **implemented; two consecutive fully green CI runs on the final commit**
(`34956471189` and `34957283051`, 38/38 jobs each). Base `d5e4113` (main), branch
`lane/flakes`, commits `bd1fbb0` (#30) and `8610b97` (#26). Evidence for both was
collected on throwaway `diag/flake-evidence` runs, listed in the ledger.

Both flakes had the same shape — a gate whose constant was close enough to the
host's own floor that ordinary CI scheduling crossed it — and different answers.
`timer_precision` was measuring the right thing against the wrong bound, so the
bound is now measured. The Windows handle count was measuring the right thing at
the wrong moment, so the comparison is unchanged and only the sampling waits.

---

## #30 `timer_precision` — the bound was below the platform's own floor

### Root cause

`timer_precision` (`crates/turnloop-contract/src/lib.rs`) took twenty expiries of
a 250 µs deadline and required the median lateness to be under a **fixed** 500 µs
on native backends and 2 ms on WASI.

That constant was never far from what the platform itself can do. The M1 Windows
spike had already measured it: `spikes/iocp/WINDOWS_RESULTS.md` records
p50/p95/max lateness of **275.2 / 285.8 / 380.9 µs** for exactly this 250 µs
deadline through exactly this NT-packet route, on real hardware, and closes with
"this single host does not establish the minimum-version or VM precision matrix".
So the gate ran with under a factor of two of clear air on the one backend whose
floor had been measured, and GitHub's windows-2025 runners are VMs.

Measured on the diag run (`34954538795`, windows-2025, all three feature modes,
idle runner): loop p50 **285.1–298.5 µs**, p90 up to 799.3 µs, max up to 805 µs.
Two of twenty samples already exceed the 500 µs bound on an *idle* runner; it only
takes load to move the median past it. That is precisely what was reported:

| run | arm | reported |
|---|---|---|
| `34947726746` attempt 1 (main, `7e367cd`) | test-native (windows-2025, default) | `timer precision exceeded platform bound: median lateness 756.2µs` |
| `34950806076` attempt 1 (PR #29) | test-native (windows-2025) | median lateness 594.2 µs, within the hour |
| #22 | wasi (wasm32-wasip3, release) | median 2.31 ms against the 2 ms bound, samples **1.07–6.44 ms**, host load average 148–182 (`docs/lanes/tl-i06.md` row 41) |

Neither was a backend regression: both passed on re-run of the same commit, and
the main failure skipped the release workflow, which is how it blocked publishing
0.1.0-alpha.3.

The flake also reproduces off CI. On the shared macOS development box, with other
agents building, an unmodified run measured loop p50 **506.5 µs** — a failure of
the old gate — while the host's own sleep measured 566.2 µs in the same window.
The WASI half reproduces there too: a `wasm32-wasip2` release run measured loop
p50 **2.230 ms** against the old 2 ms bound, with the host's own sleep at 1.258 ms.

### What actually moves on windows-2025

The lateness distribution there is **bimodal**, and visibly so on an *idle*
runner. From the load-curve diag (`34957467265`, windows-2025, 20 samples per
row):

| busy threads | loop min | loop 2nd | loop p50 | loop p90 | loop max |
|---|---|---|---|---|---|
| 0 | 293.3 µs | 297.5 µs | 301.0 µs | **800.1 µs** | 807.1 µs |
| 1 | 301.7 µs | 302.4 µs | 306.1 µs | 311.9 µs | 419.6 µs |
| 2 | 297.9 µs | 297.9 µs | 301.7 µs | **800.7 µs** | 813.6 µs |
| 3 | 301.6 µs | 302.0 µs | 304.7 µs | 309.7 µs | 309.9 µs |
| 4 (= all vCPUs) | 1.841 ms | 4.352 ms | 8.752 ms | 21.33 ms | 27.41 ms |
| 8 | 33.38 ms | 33.41 ms | 44.46 ms | 57.36 ms | 57.91 ms |

There is a fast mode at ~300 µs and a slow mode at ~800 µs, and a couple of the
twenty samples land in the slow mode even with nothing else running. **756.2 µs
and 594.2 µs are that slow mode taking the median** — not the fast mode moving.
The old gate was a coin toss on how many of twenty samples landed in a bucket
that sits above its 500 µs constant.

That is exactly what the capability clause keys on: the fast mode is stable to
within 3 % from an idle runner up to three of four vCPUs busy, so the second-best
of twenty stays at ~300 µs — 1.65× inside the floor — while the median wanders.
The runner only loses the fast mode entirely once every vCPU is saturated, and at
that point the host's own sleep is equally hopeless (min 2.31 ms at four busy
threads, 33.31 ms at eight): no gate can certify sub-millisecond timing on a
machine that cannot schedule a thread, and this one fails rather than pretend.
That is the documented boundary, not a flake mode — the reported failures were a
2–2.7× degradation, two orders of magnitude short of it.

On the macOS box the same curve is flat: the second-best stays between 37.3 µs
and 58.4 µs at every load level from 0 to 8 spinners, because that host has ten
cores and never saturates.

### The fix

Two independent requirements replace the single fixed median, in a private
`precision` module beside the test. Every attempt reports min / 2nd / p50 / p90 /
max for **both** measurements and says which clause it missed.

1. **Capability** — the *second-best* expiry of twenty must still meet the same
   platform floor (500 µs native, 2 ms WASI). Load only ever makes a sample later,
   never earlier, so no amount of host noise or calibration error can excuse this
   clause; a wait floor or a deadline rounded up to milliseconds raises *every*
   sample, including these, and fails it. Second-best rather than best, so one
   lucky expiry cannot certify a backend and one stalled one cannot condemn it.
2. **Typical case** — the median must stay within `max(floor, 2 × the host's own
   median)`, where the host's median is measured with `std::thread::sleep` at the
   same 250 µs delay, **interleaved** with the expiries so a load spike moves the
   bound as well as its subject.

The calibration never touches the `Driver`. `std::thread::sleep` goes straight to
the OS primitive — a `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` timer on Windows
(the same object kind `backend/iocp/timer.rs` arms, and a host that cannot create
one fails `Loop::new` rather than relaxing this bound), `nanosleep` on Unix, and a
`wasi:clocks` monotonic pollable on WASI — so a backend that loses precision
cannot move it.

Retries are bounded at three and **granted only to a demonstrably loaded host**:
an attempt whose own sleep median stays inside half the quiet-host floor fails
immediately, because on such a host the loop is the only explanation. A wait floor
is systematic and fails every attempt regardless.

### Why the median clause needs the capability clause

Calibrating against the host's own sleep is accurate on three of four platforms
and badly pessimistic on the fourth. Measured on run `34954538795`:

| arm | loop p50 | host sleep p50 | ratio |
|---|---|---|---|
| windows-2025 default / executor / all-features | 298.3 / 298.5 / 285.1 µs | 293.1 / 291.6 / 280.5 µs | ~1.0 |
| ubuntu-24.04 default | 69.6 µs | 65.5 µs | ~1.06 |
| wasm32-wasip2 (release, Wasmtime 46) | 900.8 µs | 866.3 µs | ~1.04 |
| wasm32-wasip3 (release, Wasmtime 46) | 867.9 µs | 864.6 µs | ~1.00 |
| **macos-15 default** | **189.7 µs** | **1.657 ms** | **0.11** |

macos-15 runners coalesce `nanosleep` to about 1.7 ms while the kqueue path
resolves 190 µs, so on that arm the calibration alone would raise the allowance to
3.3 ms and hide a millisecond regression. The capability clause is what keeps that
arm honest, and it is the clause both mutations trip.

### Mutation proof

Neither mutation touches the calibration, so a gate that only calibrated would
have to catch them on the median; both are in fact caught by the capability
clause, on the first attempt, on a quiet host. Each mutation was applied to
`crates/turnloop/src/driver.rs:1239`, the one place the loop turns its deadline
into a wait:

```rust
let mut timeout = deadline.map(|d| d.saturating_duration_since(start));
// MUTATION A: 5 ms wait floor.
timeout = timeout.map(|d| if d.is_zero() { d } else { d.max(Duration::from_millis(5)) });
// MUTATION B: round every wait up to a whole millisecond.
timeout = timeout.map(|d| Duration::from_millis(d.as_nanos().div_ceil(1_000_000) as u64));
```

macOS arm64, `cargo test -p turnloop-contract --lib timer_bounds`:

| mutation | runs | result |
|---|---|---|
| A, 5 ms floor | 3/3 | **FAIL** — `loop min=5.05ms 2nd=5.42ms p50=5.82ms … floor 500µs MISSED`, worst case after 2 of 3 attempts |
| B, millisecond rounding | 3/3 | **FAIL** — `loop min=905.75µs 2nd=907.375µs p50=1.07ms … floor 500µs MISSED`, 1 attempt |
| B, under four CPU spinners | 3/3 | **FAIL** (against the earlier single-clause draft, which the load could not rescue either) |
| unmutated | 10/10 + 3/3 | **PASS**, allowance stayed at the 500 µs floor on every quiet run |

Mutation A's second attempt shows the design working end to end: the first attempt
had a loaded calibration (allowance 638.9 µs) and still failed, because
`2nd=5.42 ms` missed the floor.

The Windows arm of the same proof is **UNRUN** — no Windows host here, and running
a deliberately broken driver through CI was not worth a 30-minute run. The
capability clause is platform-independent (it compares the backend against the
same constant the old gate used), and the Windows numbers above show the clause
has 1.7× of margin on an idle runner.

---

## #26 the Windows handle count — a thread-pool wait packet, released asynchronously

### Root cause: not a leak

`loop_drop_terminates_live_children_and_releases_their_handles`
(`crates/turnloop-contract/tests/windows_lifetimes.rs`) spawns eight live children,
drops the loop, and compares `GetProcessHandleCount` against an exact baseline,
sixteen times. It failed once on windows-2025 in PR #24's run `34941544032` —
`left: 83, right: 82`, "live-child drop leaked a native handle" — on a PR that
only changed `release.yml` and `release.py`.

The outstanding handle is a **`WaitCompletionPacket`**, and the driver never owned
it. `backend/iocp/process.rs:622` learns a child exited through
`RegisterWaitForSingleObject`; the Windows thread pool backs each such
registration with a wait-completion packet of its own.
`UnregisterWaitEx(INVALID_HANDLE_VALUE)` in `Child::join` is the strongest join
Windows documents and it joins the **callback** — the pool then closes its own
packet afterwards, on its own threads. `GetProcessHandleCount` counts the whole
process, so it sees that packet until the pool is finished with it.

Measured directly, by walking this process's handle table by object type after
every cycle (diag run `34954538795`, 192 cycles, all three windows-2025 feature
modes):

| condition | table movements over 192 cycles |
|---|---|
| idle runner | **0**, all three modes |
| eight busy threads | **129 / 138 / 117** (default / all-features / executor) |

and every single movement is the same event:

```
round 0: 130 -> 131 handles; gained [0x458=WaitCompletionPacket]; released []
round 1: 131 -> 131 handles; gained [0x160=WaitCompletionPacket]; released [0x458=WaitCompletionPacket]
round 4: 131 -> 130 handles; gained []; released [0x414=WaitCompletionPacket]
```

The count oscillates 130 → 131 → 130, one `WaitCompletionPacket` at a time, and
always comes back. Nothing accumulates: the final count after 192 loaded cycles is
the same 130 the sweep started from. That is an asynchronous release, not a leak,
and it matches the load-dependence of the original failure — a runner busy enough
to delay a pool thread is exactly when a single sample lands on the 131.

### The fix

The comparison stays exact; only the moment of sampling is allowed to wait.
`settled(subject, baseline, &named)` requires the count to be **exactly** the
baseline again within a bounded five seconds, sleeping a millisecond between
polls so the pool thread that closes the packet gets a slot too. A genuinely
leaked handle never comes back, so it still fails — at the deadline, with a
message that says the wait already elapsed, so the reader knows this is not the
pool tearing down its own packet.

The deadline is validated, not assumed. Diag run `34955502692` ran the same
192-cycle loaded sweep *through* `settled()` on all three windows-2025 modes:

| mode | settled immediately | needed the wait | worst wait |
|---|---|---|---|
| default | 117 / 192 | 75 | **1.646 ms** |
| executor | 133 / 192 | 59 | **1.583 ms** |
| all-features | 143 / 192 | 49 | **1.614 ms** |

A third of the cycles on a loaded runner do have a packet outstanding — which is
the flake, reproduced 183 times — and every one of them converged inside two
milliseconds, against a five-second deadline. The release also does not need more
work to drive it: `settled()` only sleeps.

And the failure now names the handle, which is what turned this issue from a guess
into a measurement: `handle_table()` walks the process's kernel handle table
(handle values are multiples of four, so probing the low range covers a test
process) and types each entry with `NtQueryObject(ObjectTypeInformation)`;
`handle_diff()` reports what the table gained and released. Both calls are pure
queries — `ObjectTypeInformation` never blocks the way `ObjectNameInformation` can
on a synchronous pipe — so the scan cannot change the count it is explaining, and
it runs only to build a failure message. This needed `Wdk_Foundation` added to
`turnloop-contract`'s `windows-sys` features.

The two other whole-process handle comparisons in the same file
(`loop_drop_and_stale_wakers_release_windows_handles`,
`listener_reuse_and_busy_connect_drop_release_native_handles`) get the same
treatment: the reason — a process-wide counter can lag a per-loop teardown — is
generic, even though the measured evidence is for the child-wait path. On a host
with nothing outstanding `settled()` returns on its first check and costs nothing.

### Not done, and why

The packet exists only because child-exit notification goes through the OS thread
pool. `backend/iocp/timer.rs` already owns the alternative —
`NtCreateWaitCompletionPacket` / `NtAssociateWaitCompletionPacket` against the
driver's own port — and associating each child's process handle the same way would
delete the thread pool from this path entirely, make the packet ours, and close it
deterministically at `Child::drop`. That is a real improvement and a real change
to process-exit routing (`Child::ready`/`status`/`join`, completion-key
assignment, and `bridge.rs`, which registers the same way for the GUI event). It
is not a flake fix and the evidence says there is nothing leaking, so it is left
as a follow-up.

---

## Verification ledger

Host: macOS 15 arm64, shared with other builds. Toolchain `nightly-2026-08-20`
from `rust-toolchain.toml`. `CARGO_BUILD_JOBS=4` throughout.

| # | Command | Result |
|---|---|---|
| 1 | `cargo fmt --all --check` | **PASS** |
| 2 | `cargo clippy --locked -p turnloop -p turnloop-contract -p turnloop-io --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | **PASS** |
| 3 | the same, `--all-features` | **PASS** |
| 4 | the same, `--target x86_64-pc-windows-msvc` | **PASS** |
| 5 | the same, `--target x86_64-pc-windows-msvc --all-features` | **PASS** |
| 6 | `cargo test --locked -p turnloop -p turnloop-contract --no-fail-fast -- --test-threads=1` | **PASS**, 15 suites |
| 7 | `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` (Wasmtime 46.0.0, pinned) | **PASS**, exit 0 |
| 8 | `python3 -m unittest discover -s scripts/ci -p 'test_*.py'` | **PASS** |
| 9 | `cargo test -p turnloop-contract --lib timer_bounds`, unmutated, ×13 | **PASS** 13/13 |
| 10 | the same with mutation A (5 ms floor), ×3 | **FAIL** 3/3, capability clause |
| 11 | the same with mutation B (millisecond rounding), ×3 | **FAIL** 3/3, capability clause |
| 12 | the same with mutation B under four CPU spinners, ×3 | **FAIL** 3/3 |
| 13 | CI on `diag/flake-evidence`, run `34954538795` — handle-table sweep and calibration numbers on every arm | **PASS** as a measurement; the three windows-2025 arms fail by design (the diag tests panic to print their report) and `workflow-lint` fails on the diag-only `--nocapture` edit to `run-tests.py` |
| 14 | CI on `diag/flake-evidence`, run `34955502692` — `settled()` under eight busy threads on windows-2025 | **PASS**, 183 of 576 loaded cycles waited, worst wait 1.646 ms of 5 s, none reached the deadline (the diag test still panics by design to print its report) |
| 15 | CI on `diag/flake-evidence`, run `34956506068` — the gate under eight busy threads on windows-2025 | **FAIL by design**, and the reason is recorded above: at 200 % CPU the host's own sleep is 33 ms too. Superseded by row 16 |
| 16 | CI on `diag/flake-evidence`, run `34957467265` — the load curve, windows-2025 and macos-15 | **PASS** as a measurement; the gate holds at 0-3 of 4 busy vCPUs and fails only at full saturation |
| 17 | CI on `lane/flakes`, run `34955472126` (code identical to the final commit except `settled()`'s failure text) | **PASS**, 38/38 jobs green |
| 18 | CI on `lane/flakes`, run `34956471189`, final code commit `e4bc116` | **PASS**, 38/38 jobs green |
| 19 | CI on `lane/flakes`, run `34957283051`, same commit, consecutive | **PASS**, 38/38 jobs green |
| 16 | `python3 scripts/ci/run-tests.py native` on Windows / Linux | **UNRUN**, no host; covered by rows 13–15 |
| 17 | `python3 scripts/ci/run-tests.py web` / `node` | **UNRUN**, `wasm-bindgen-test-runner` not installed; neither file is compiled for `wasm32-unknown-unknown` tests, and cross-clippy covers the code |
| 22 | Mutation proof on the Windows CI arm | **UNRUN**, see #30 above |
