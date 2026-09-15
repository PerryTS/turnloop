# ProcessSpec: extra descriptors and session control — issue #38

Base `6c41265`, branch `lane/procspec`, macOS arm64 + Linux x86_64 + Windows CI.

Perry's P2 migration moved child stdout/stderr, dgram and signals onto turnloop,
but child spawn and exit could not move: `ProcessSpec.stdio` is exactly three
entries, so `child_process.fork()`'s IPC channel at descriptor 3, `spawn`'s
arbitrary `stdio` array and a pty child's controlling terminal had nowhere to go.

## 1. Decision

**Implement (1) extra descriptors and (2) session control. Do not implement (3)
`Loop::adopt_process`.**

### Why extra descriptors and session control

The three things a host cannot supply from outside a spawn are the descriptor
plan, the session, and the controlling terminal: all of them happen between
`fork` and `exec`, inside the window turnloop owns. Everything else Perry's
spawn path does — program resolution, shell selection, argv, environment
composition, the *value* of `NODE_CHANNEL_FD`, pty allocation and its termios —
is policy the host already owns and keeps owning.

Turnloop owning the child's identity is also what makes the guarantees in the
issue true. Exactly-once completion, the kill/close ordering and grandchild
cleanup all rest on the loop holding the child from before it runs: on Windows
the Job Object has to be assigned while the child is still suspended, or its
descendants are not in the job at all.

### Why not adoption

`Loop::adopt_process(pid, …)` was evaluated and rejected, on four grounds.

1. **It does not deliver the guarantee it would be adopted for.** On Windows a
   process must be assigned to a job *before* it creates descendants;
   `AssignProcessToJobObject` on a running process leaves its existing
   grandchildren outside. So an adopted process has strictly weaker tree
   cleanup than a spawned one, and `kill_group` would have to be `InvalidInput`
   for it. The issue requires grandchild cleanup to be kept.
2. **The portable signature does not exist.** A pid is not a safe identity on
   Windows: `OpenProcess` by pid races pid reuse, so an honest API takes an
   `OwnedHandle` there and a pid on Unix. That is a platform-divergent public
   API in a crate whose entire point is one contract on six backends.
3. **It answers a question the descriptor work already answers.** The reason
   adoption was attractive was that Perry could keep its own `pre_exec` and fd
   plan. The survey of Perry's spawn paths (below) shows that plan is exactly
   `setsid`, `dup2` and `TIOCSCTTY` — all three are now `ProcessSpec` fields,
   so there is nothing left for the host's hook to do.
4. **A mode that exists is a decision that has not been made.** Two ways to own
   a child, with different reaping rules and different tree semantics, is a
   configuration matrix nobody would exercise on every platform.

**If it is ever added, this is the reaping rule it must satisfy**, and it is
worth recording because it is the part that is easy to get wrong. turnloop must
reap an adopted child with a *targeted* wait — `waitpid(pid, …)`, never
`waitpid(-1, …)`, `wait()`, or a negative process-group pid — because a wait for
any child consumes whichever child exits first, including another subsystem's.
turnloop already satisfies this for the children it spawns:
`ChildState::reap` calls `std::process::Child::try_wait`, which is
`waitpid(pid, WNOHANG)`, and the SIGCHLD subscription only *notifies*; it never
reaps. Adoption would additionally need the converse contract stated in the API:
adopting a pid transfers reaping ownership of that pid to the loop, and the host
must not wait for it any more. Perry can honour that — the survey found exactly
one raw `waitpid` in the whole workspace (`pty/native.rs:239`, targeted and
blocking) and every other reap goes through `std::process::Child`, which is
targeted too. `a_sibling_waiter_and_the_loop_keep_their_own_children` asserts
the half that exists today, in both orders.

### Which pre-exec behaviour turnloop owns

Perry's current child hooks are `setsid` (detached spawns, `child_process/options.rs:149`
and `registry.rs:224`), `dup2` of the IPC socket onto the channel descriptor
(`child_process/fork.rs:282`), `dup2` + `F_SETFD` for each extra stdio entry
(`child_process/options.rs:297`, `:314`, `:325`), and, in the pty path's raw
`fork`, `setsid` + `TIOCSCTTY` + `dup2` (`pty/native.rs:198–222`). uid/gid are
delegated to std. Nothing resets signal masks or dispositions, sets `umask`, or
closes descriptor ranges.

turnloop now owns all of it. In the order the child executes them:

| # | step | source |
|---|---|---|
| 1 | `dup2` of stdin, stdout, stderr | std, from `ProcessSpec::stdio` |
| 2 | `setgroups`/`setgid`/`setuid` | std, from `ProcessSpec::uid`/`gid` |
| 3 | `chdir` | std, from `ProcessSpec::cwd` |
| 4 | `setpgid(0, 0)` | std, from `ProcessSpec::new_process_group` |
| 5 | empty the signal mask, `SIGPIPE` back to `SIG_DFL` | std, unconditional |
| 6 | `setsid()` | turnloop, from `ProcessSpec::detached` |
| 7 | `ioctl(0, TIOCSCTTY, 0)` | turnloop, from `ProcessSpec::controlling_terminal` |
| 8 | `dup2` of each extra descriptor onto its number | turnloop, from `ProcessSpec::extra` |

Steps 6–8 are one hook, so their order is fixed rather than emergent. std runs
`pre_exec` closures last, immediately before `exec`, which is what makes step 7
correct: descriptor 0 is already the child's stdin by then, so a pty child
claims the terminal it was actually given.

The host keeps: program resolution and shell policy, argv, environment
composition including the `NODE_CHANNEL_FD` name and value, `openpty` and its
termios, and the decision of which descriptor number is the channel.

## 2. API

```rust
pub const MAX_CHILD_FD: u32 = 255;

pub struct ChildFd {
    pub number: u32,            // 3..=MAX_CHILD_FD, unique within a spec
    pub source: ChildFdSource,
}

pub enum ChildFdSource {
    Null,                       // the platform null device, read/write
    Pipe,                       // one way: the child writes, the parent reads
    Duplex,                     // both ways; Node's 'pipe' and 'ipc' entries
    Handle(Handle),             // duplicate a transport this loop owns
}

pub struct ProcessSpec {
    // ... unchanged fields ...
    pub extra: Vec<ChildFd>,
    pub controlling_terminal: bool,
}

impl Loop {
    pub fn spawn(&mut self, spec: &ProcessSpec, token: Token) -> Result<Process>;
    pub fn spawn_extra(
        &mut self,
        spec: &ProcessSpec,
        token: Token,
        parents: &mut [Option<Handle>],
    ) -> Result<Process>;
}
```

`parents` has one slot per `ChildFd`, in the same order, and receives the parent
end of each: a readable handle for `Pipe`, a readable and writable one for
`Duplex`, `None` for `Null` and `Handle`. Keeping it a caller-supplied slice is
what keeps `Process` `Copy` and keeps the driver free of a per-spawn allocation
of its own; `spawn` is `spawn_extra` with an empty slice and refuses a spec that
has extras, because their parent ends would have nowhere to go.

There is deliberately no `Inherit` source: inheriting the host's own descriptor
number means clearing close-on-exec on a descriptor turnloop does not own. Adopt
it (`Detached::from_fd` / `from_handle`, then `attach`) and pass the handle.

`NODE_CHANNEL_FD` is not a turnloop concept. The host writes it into
`ProcessSpec::env` next to the `ChildFd` it names, on both platforms, with the
same value.

### Rejected plans

All of these are `InvalidInput`, and create nothing — no child, no handle, no
completion, and `parents` is left all `None`:

- `parents.len() != spec.extra.len()`
- a number outside `3..=MAX_CHILD_FD`, or repeated within one spec
- `spawn` (rather than `spawn_extra`) with a non-empty `extra`
- `controlling_terminal` without `detached`
- `controlling_terminal` on Windows is `Unsupported`

## 3. Per-platform behaviour

| | Unix (epoll/kqueue) | Windows (IOCP) | WASI 0.2/0.3, web |
|---|---|---|---|
| how the child sees a number | the descriptor number itself | a C run-time descriptor, published in `STARTUPINFOW.lpReserved2` | `Unsupported` (no processes) |
| `Duplex` | `socketpair(AF_UNIX, SOCK_STREAM)`, as libuv creates every child pipe | duplex named-pipe instance: `PIPE_ACCESS_DUPLEX` for the overlapped parent end, `GENERIC_READ\|GENERIC_WRITE` synchronous for the child | — |
| `Pipe` | `pipe2(O_CLOEXEC)`, or `pipe` + `FD_CLOEXEC` on Darwin | `PIPE_ACCESS_INBOUND` named-pipe instance | — |
| `Null` | `/dev/null`, `O_RDWR\|O_CLOEXEC` | `NUL`, `GENERIC_READ\|GENERIC_WRITE` | — |
| `Handle(h)` | `F_DUPFD_CLOEXEC` of the loop's transport | `DuplicateHandle` with inheritance | — |
| placement | one `pre_exec` hook `dup2`s each source onto its number | the inherited-descriptor block plus `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` | — |
| `controlling_terminal` | `setsid` then `TIOCSCTTY` on descriptor 0 | `Unsupported` | `Unsupported` |
| grandchild cleanup | process group, `kill_group` | Job Object, `TerminateJobObject` | — |

**Unix placement.** Sources are relocated above the highest target number with
`F_DUPFD_CLOEXEC` *before* the fork, so the child hook can `dup2` them in any
order without an earlier target having overwritten a later source. `dup2` leaves
the new descriptor without `FD_CLOEXEC`, which is exactly what makes that number
survive `exec`. Adding a hook takes std off its `posix_spawn` fast path, as
`detached` already did.

**Unix: the target numbers are reserved across the fork.** This one is a bug
that was found and fixed during the work, and it is worth stating because it is
invisible until it bites. `Command::spawn` creates its own exec-error pipe
*after* the standard streams and immediately before the fork, with no
relocation of its own (`sys::pipe::pipe()`, or a `SOCK_SEQPACKET` pair on
Linux), so it takes the lowest free descriptor numbers — which is precisely what
the sources vacate when they are lifted above the targets. If its write end
landed on a target number, the child hook would `dup2` over it, the parent's
read would see end-of-file, and **a failed `exec` would be reported as a
successful spawn**. turnloop therefore holds every *free* target number in the
parent, with a close-on-exec duplicate, from before `Command::spawn` until the
child exists; a number the parent already uses cannot be handed to std either,
so those need nothing. `a_failed_exec_is_reported_even_at_the_lowest_free_descriptor_numbers`
probes the two lowest free numbers, releases them, uses them as the targets and
requires `NotFound`. With the reservation removed it fails, which is what makes
it a test of the reservation rather than of a coincidence.

**Windows placement.** The inherited-descriptor block is the C run-time's own
format — a descriptor count, one flag byte per descriptor, then one handle per
descriptor, packed without padding — and it is how libuv and Node give a child
numbered descriptors at all. Flags come from `GetFileType`: `FOPEN`, plus
`FPIPE` for a pipe and `FDEV` for a character device. Numbers below the highest
one in use but not claimed are present and closed (`INVALID_HANDLE_VALUE`).
Because this is Node's own convention, `NODE_CHANNEL_FD=3` names the same thing
in a child here as it does under Node, and `_get_osfhandle(3)` in the child
returns the inherited handle. A child that does not use the C run-time inherits
the handles but has no number for them; that is the same limitation libuv has.

**Windows `Handle(h)` caveat.** A transport the loop created itself — a named
pipe or a socket — is overlapped, and a duplicate of it in the child is
overlapped too. A child that reads it with plain
blocking calls will not work; it must use overlapped I/O, or the host must pass
a synchronous handle it adopted (`Detached::from_handle` on a `CreatePipe` end,
which is what the contract test does). libuv's `UV_INHERIT_STREAM` has the same
property. On Unix the equivalent caveat is the status flags: an adopted
transport is non-blocking, and the child's duplicate shares that file
description.

**`Pipe` is a real pipe, not a half-shut socket pair.** Darwin refuses
`shutdown(SHUT_RD)` on a `socketpair` end with `ENOTCONN` (verified directly),
so the direction has to come from the object rather than from a later call.

## 4. Tests

`crates/turnloop-contract/tests/process_fds.rs`, all running on Unix and
Windows unless noted:

- `five_descriptors_carry_bytes_both_ways_through_a_channel_handoff` — the
  issue's headline case. A child with descriptors 0–4, where 3 and 4 are
  `Duplex`; the parent writes `ping-3`/`ping-4` and the child answers
  `pong-3`/`pong-4` on the same descriptors. The child finds descriptor 3 by
  reading `NODE_CHANNEL_FD` out of its environment and 4 out of a second
  variable, so the handoff is proven rather than hard-coded. Exit status,
  write completions, and one `Closed` per handle are all asserted.
- `extra_sources_cover_one_way_pipes_the_null_device_and_adopted_transports` —
  `Pipe` at 3, `Null` at 4, `Handle` at 5 where the handle is an OS pipe the
  test created outside the loop and attached. The child writes to 3 and 5, and
  reads 4 to end-of-file, which is what the null device must do.
- `close_orders_cancel_before_closed_for_a_child_holding_extra_descriptors` —
  closing a live child with two extra descriptors yields exactly one
  `Cancelled` then one `Closed`, in that order per handle (the rig records
  terminal results in arrival order and compares the sequence, rather than only
  counting them), with `op` present on the cancellation and absent on the
  `Closed`. No exit is reported for the cancelled watch, the child is reaped
  (`ECHILD` on Unix), the extras close independently, and a further turn
  produces nothing.
- `rejected_descriptor_plans_create_nothing` — every rejection above, each
  asserting `!driver.alive()` and an untouched `parents` slice afterwards.
- `a_failed_exec_is_reported_even_at_the_lowest_free_descriptor_numbers` (Unix)
  — the descriptor-reservation regression test described in §3.
- `a_sibling_waiter_and_the_loop_keep_their_own_children` — a plain
  `std::process::Command` child exiting 7 (then 11) beside loop-owned children
  exiting 23, in both orders: the loop reports its own child's status while the
  sibling is unreaped, and a host reap does not consume the loop's child.
- `a_process_group_with_extra_descriptors_still_kills_its_grandchild` — the
  leader reports its grandchild's identity, the test pins it, `kill_group`
  terminates the tree, and the grandchild is polled until gone.
- `a_detached_child_can_claim_its_stdin_as_a_controlling_terminal` (Unix) —
  `openpty`, the slave attached and passed as stdin, `detached` +
  `controlling_terminal`; the child reports that it leads its own group and
  session, that `/dev/tty` opens, and that it is the terminal's foreground
  group.

`crates/turnloop-contract/tests/allocations.rs`:

- `extra_child_descriptor_traffic_allocates_nothing_after_spawn` — eight
  children with two extra `Duplex` descriptors each. Spawning is charged to
  setup, as every other resource creation is; the counted window covers the
  writes, the reads, the exits and the closes, and must be zero. The gate
  asserts its subject ran (16 writes, 8 exits, 32 closes) rather than merely
  that nothing threw, and it was sabotage-checked: a single planted
  `vec![0u8; 8]` inside the window fails it.

The fixture (`crates/turnloop-contract/src/bin/native_child.rs`) gains
`channel`, `extra-sources`, `exit-with` and `tty-session` modes. Its descriptor
I/O goes through the number on Unix and through `_get_osfhandle` on Windows,
which is exactly how `uv_pipe_open` turns Node's `NODE_CHANNEL_FD` into a
stream, so the Windows test exercises the convention rather than a turnloop
private path.

## 5. Commands

| command | result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` (macOS arm64) | PASS |
| `cargo clippy -p turnloop -p turnloop-contract -p turnloop-io --all-targets --target x86_64-unknown-linux-gnu -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| `cargo clippy … --target x86_64-pc-windows-msvc …` | PASS |
| `cargo clippy … --target wasm32-wasip2 …` | PASS |
| `cargo clippy … --target wasm32-unknown-unknown …` (web) | PASS |
| `cargo clippy --workspace --all-targets --target wasm32-wasip2 --all-features …` | UNRUN locally (no wasm C toolchain for `ring` on this host); run by CI `lint-wasm` — PASS |
| `cargo clippy … --target wasm32-wasip3 …` (nightly-2026-09-07) | UNRUN locally (toolchain not installed); run by CI `wasi (wasm32-wasip3)` and `protocol-wasi (wasm32-wasip3)` — PASS |
| `cargo test -p turnloop-contract --test process_fds -- --test-threads=1` (macOS arm64) | PASS (8/8) |
| `cargo test -p turnloop-contract --test allocations extra_child_descriptor -- --test-threads=1` (macOS arm64) | PASS |
| `cargo test --workspace --no-fail-fast -- --test-threads=1` (macOS arm64) | PASS |
| `cargo test --workspace --no-fail-fast -- --test-threads=1` (Linux x86_64, build box) | PASS for everything in this lane. One pre-existing, unrelated failure: `filesystem::permission_denied_is_reported`, because that box runs as root and root ignores the read-only mode bit; CI's Linux arms, which are not root, pass it. |
| `cargo test -p turnloop-contract --test process_fds --test allocations -- --test-threads=1` (Linux x86_64, build box, final commit) | PASS (8/8 and 16/16) |
| `bash scripts/ci/no-tokio.sh` | PASS (15 policy rows) |
| `python3 scripts/ci/soak.py` | PASS (251 locked versions, 1 active security exception) |
| `bash scripts/ci/install-wasmtime.sh` then `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS (11 + 43 + 43 + 13 tests) |
| CI on `lane/procspec`, run `35013478648` (all four OS arms, Windows x86_64 runtime in three feature modes) | PASS |
| CI on `lane/procspec`, final run `FINAL_RUN` | FINAL_RESULT |

### Sabotage checks

Two gates were shown to fail when their subject is removed, rather than being
assumed to work:

- the allocation gate, with one planted `vec![0u8; 8]` inside the counted window;
- the descriptor reservation, by deleting it and watching
  `a_failed_exec_is_reported_even_at_the_lowest_free_descriptor_numbers` report
  a successful spawn of a program that does not exist.

### Not covered

- `ChildFdSource::Handle` of an overlapped loop transport on Windows: the child
  must drive it with overlapped I/O. The contract test passes a synchronous
  adopted handle instead, which is the shape a host actually wants.
- Descriptor passing (`SCM_RIGHTS`) *over* an extra `Duplex` descriptor. The
  transport is an `AF_UNIX` stream, so `send_handle`/`recv_handle` apply to it
  like any other loop pipe, but no test exercises that combination yet; it is
  what `cluster.fork()` will need.
- No `adopt_process`, by decision (§1).
