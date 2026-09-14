# core5 lane report

Status: implementation and requested local verification complete. Linux runtime
remains **UNRUN**; additional WASI 0.3 timing and Android lint failures are recorded
below. Base `fcea55e`; no commits are made by this agent (`.git` is read-only).

## Findings

- The reported between-round close/rebind hypothesis does **not** match the test:
  both loops and all 16 IPv6 sockets are created before `0..=ROUNDS`, remain bound
  through all 21 rounds, and drop only after every receive/send is verified.
- A separate production binding bug was found: `Unix::open` unconditionally
  enabled `SO_REUSEADDR` for UDP even when `UdpOpts::reuse_port` is false. Linux's
  `udp_lib_get_port` (used for IPv4 and IPv6) excludes an existing socket from its
  ephemeral-port conflict bitmap when both sockets have `sk_reuse`. Thus two live
  default sockets can share an endpoint and the kernel can deliver a self-send
  to the other socket. This explains the failure shape; the original CI failure
  lacks addresses, so its exact cause remains unconfirmed by runtime evidence.
  Source: [Linux v6.8 UDP port allocation](https://github.com/torvalds/linux/blob/v6.8/net/ipv4/udp.c#L141-L291).
- macOS rejects an explicit duplicate UDP bind with only `SO_REUSEADDR` for both
  IPv4 and IPv6 (local Python socket probe). This explains why that particular
  Linux binding defect need not reproduce on kqueue.
- Epoll registers full generational handle keys, deregisters before releasing
  owned descriptors, and Unix removes cached scheduling entries on release and
  detach. Polling validates full keys; native I/O executes synchronously against
  the matched resource. Timerfd uses a private key and the same dispatch path.
  No fd/generation misattribution found. Deterministic reuse regressions pass on
  macOS. The [epoll manual](https://man7.org/linux/man-pages/man7/epoll.7.html)
  documents the fd/open-file-description identity and duplicate-fd lifetime hazard;
  the implementation explicitly deregisters while the owned descriptor is live.
- Pooled receive length comes directly from `recvfrom`, independently of the
  lease. Each lease owns its vector and originating pool. Review found no route
  for a lease mix-up to change a one-byte syscall result to zero.

## Implemented

- Default Unix UDP no longer enables `SO_REUSEADDR`. TCP listeners and explicit
  UDP `reuse_port` retain their options. No polling, completion, generation or
  allocation path changes were needed.
- Deterministic IPv4/IPv6 tests inspect the actual default socket option, reject
  duplicate live binds, rebind after release, and preserve explicit sharing.
  The option regression FAILED before the fix on macOS (`SO_REUSEADDR = 4`),
  then passed unchanged after the fix.
- A separate regression arms/cancels a receive with a zero-byte packet queued
  and actual old-generation poll events collected, detaches, uses `dup2` to
  atomically close/reuse the exact owned descriptor, binds the exact old port,
  and reattaches under a new handle generation. It asserts stale identities do
  not affect the new operation, the empty socket really waits, only the new
  packet arrives with its correct source/op/bytes, and no duplicate follows.
  Keeping the destination fd owned throughout avoids overwriting another test's
  descriptor. Both IPv4 and IPv6 cases must execute.
- The original allocation test still keeps all 16 sockets bound for 21 rounds.
  It now asserts endpoint uniqueness across both loops before sending, prints
  fixture handles/endpoints outside the allocation window, checks cancellation
  and receive/send op IDs, handles, tokens, terminal status and buffer mode,
  and checks sender **before** length with round/loop/identity context. No stray
  result is ignored/retried. Pooled-result handling also avoids eagerly borrowing
  the other loop's potentially pending provided buffer (`map_or` evaluated its
  fallback unconditionally). Counts, capacity-one backpressure, every length
  (including zero), cancellation/drop work and zero-allocation limits remain.
- Other UDP contracts (steady allocation, shared echo, executor and WASI empty
  datagram) now assert distinct endpoints; completion-based cases check sender
  before length. These sockets likewise remain alive throughout their traffic.
- TCP audit: echo/pair, accept churn, transfer/handoff, no-spin and lifetime tests
  keep their listeners/streams alive until the associated work is done. Shared
  listeners intentionally use `reuse_port`; TCP connections are separate streams,
  not UDP receive queues. The independent `refused_connect_once` fixture closes
  a temporary listener before connecting: another process can claim that port in
  the gap. It is unchanged and documented below as a separate fixture issue.

## Verification

PASS: required design/contributing/integration and core/core3/CI/WASM lane reports read
in full; no applicable AGENTS.md found; working tree initially clean; installed
target inventory includes both Linux triples. PASS: local IPv4/IPv6 socket probe
above. Every verification command and result is recorded in
[docs/core5-commands.md](docs/core5-commands.md); raw logs are under `.tools/core5/`.
The final focused test passed **200/200 fresh processes per macOS mode** (default,
executor, all-features), **600/600 total**, with exactly one passed, zero failed,
zero ignored test required in every subprocess. This executes 192,000 measured
receives, 192,000 cancellations, plus warm-up/drop and nested steady UDP work.
The first 600-run campaign also passed; the final campaign repeated after the
provided-buffer borrow tightening. The final log audit verifies exactly 16 unique
live IPv6 endpoints in each of its 600 processes.
The new backend regressions pass 3/3 separately in every macOS mode.
Linux runtime is **UNRUN (no Linux host)**; Windows runtime likewise UNRUN.
No-tokio PASS on all eight target graphs and union, default/all features. Soak
PASS: 251 locked registry versions, only the inherited exact rustls exception.
Initial native strict Clippy FAIL: the new getsockopt safety comment needed to
be inside the multiline assertion; comment moved, no lint weakened.
Both Linux architectures: full-workspace Clippy PASS in all six modes. Initial
arm64 workspace Clippy FAIL: missing `aarch64-linux-gnu-gcc`; the first Zig retry
FAILed on cc-rs's vendor-bearing target spelling. A local wrapper translates
`aarch64-unknown-linux-gnu` to Zig's `aarch64-linux-gnu`, preserving the target,
and final checks PASS. Zig caches stay under `.tools/core5/`.

| Verification | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| Strict workspace/all-target Clippy on macOS | PASS default, executor, all features |
| Strict workspace/all-target Clippy, Linux x86_64 and arm64 | PASS all six modes on each triple; compilation only |
| `cargo +stable check --locked --workspace --all-targets --all-features` | PASS |
| `python3 scripts/ci/run-tests.py native` | PASS all three modes; workspace counts 229 / 237 / 245, independent contract counts 49 / 56 / 56; 1,210 passes including repetitions |
| `cargo test --locked --workspace -- --test-threads=1` after final test edit | PASS 229 tests; ignored fixture tests remain UNRUN |
| New backend tests, separately in each macOS mode | PASS 3 per mode, with actual IPv4/IPv6 bind and fd/port reuse |
| Core/contract all-target/all-feature Clippy, Windows, WASI 0.2/0.3, web, FreeBSD | PASS (p3 uses nightly-2026-09-07) |
| Core/contract all-target/all-feature Clippy, Android | FAIL inherited pinned-Clippy diagnostic in `portable_tests.rs:13`: asks for const thread-locals although both initializers are already const; no suppression/test exclusion added |
| Same Android Clippy with `--lib` | PASS libraries; does not replace the failed all-target check |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS 6 core + 22 debug + 22 release semantic + 9 release allocation tests; direct final allocation invocation without stdin FAILed at the required stdio subject; corrected fixture rerun PASS 9/9 with real stdin |
| WASI 0.3 equivalent | FAIL twice: release timer median 2.016459 ms exceeds unchanged 2 ms limit while other builds ran; 8 core, 23 debug and 10 allocation tests PASS. Isolated rerun failed at 2.080250 ms; debug/core/allocation suites again PASS. Release tests after the abort are UNRUN; no threshold change or further retry |
| `bash scripts/ci/no-tokio.sh` | PASS, eight targets plus union, default/all features |
| `python3 scripts/ci/soak.py` | PASS 251 locked versions; inherited rustls exception only |
| `python3 scripts/ci/check-paths.py`; `python3 scripts/ci/feature_modes.py`; `git diff --check` | PASS |

WASI runs use checksum-verified Wasmtime 46.0.0 installed by the existing script
under `.tools/`. Windows, Linux, FreeBSD and Android runtime, hosted GitHub CI,
and Linux instruction measurements remain **UNRUN (no host)**. Web browser/Node
runtime is **UNRUN in this lane** (raw UDP is unsupported there). Ignored external
server suites are **UNRUN**, including SQL sandbox limitations; no ignored/empty
suite is counted as executed. No release, deployment, dependency or policy change.
The Android diagnostic is outside changed files and is retained as an open gate;
production library compilation passed without weakening it. `file` verifies ring's
cross-built objects are ELF x86-64 / AArch64, not native Mach-O objects. Final diff
audit confirms no changes to Cargo.lock, manifests, soak, CI, timer implementation,
no-tokio policy or instruction baseline, and no new `unwrap()` calls.

## Deviations / proposed DESIGN changes

No dependency, soak setting, allocation threshold or wait path changed.
Proposed clarification: default UDP binds are exclusive; address/port sharing is
an explicit `reuse_port` opt-in. DESIGN.md remains unchanged.

## Open questions / next steps

- Linux runtime must confirm the default endpoint exclusivity and exact fd/port
  reuse regression. The original failure did not record endpoints or identities;
  live endpoint aliasing is supported by kernel source, not reproduced locally.
- `refused_connect_once` needs a portable bound-but-not-listening TCP reservation
  fixture (including WASI). Its existing close-before-connect race can cause a
  spurious success if an unrelated listener claims the port. This is separate
  from UDP and left for a focused fixture change, with no retries or skips added.
- WASI 0.3 release timing failed twice on this macOS Wasmtime host; investigate
  host timing separately. UDP/allocation checks passed; the full p3 gate is FAIL.
- Integrator should run the Linux commands below and commit the tree.
  The agent made no commits; checkpoint `d872373` appeared externally.
- Investigate the pinned Android Clippy macro diagnostic separately; do not
  suppress it or delete allocation coverage to obtain a green all-target gate.

## Linux x86_64 integrator commands (runtime UNRUN here)

Run from the repository root on a real Linux x86_64 host. The required arm64
runner should execute the same six CI modes. First run the ordinary positive-count
CI wrapper, which includes all three new Unix regressions and the full contracts:

```bash
python3 scripts/ci/run-tests.py native
```

Then run the exact allocation subject **200 times per mode**, serially, in fresh
processes. The following discovers the current six feature selections from the
required CI matrix, builds each test executable once, rejects missing/multiple
artifacts, and fails immediately on a test failure or anything other than exactly
one executed pass. It never retries or filters stray completions. Preserve the
per-iteration logs, especially endpoint/identity diagnostics, on any failure.

```bash
python3 - <<'PY'
import json
import pathlib
import platform
import re
import subprocess
import sys

assert sys.platform == 'linux' and platform.machine() == 'x86_64'
sys.path.insert(0, 'scripts/ci')
from feature_modes import native_modes

name = 'concurrent_udp_returns_survive_cancellation_and_loop_drop_without_allocating'
logs = pathlib.Path('.tools/core5-linux')
logs.mkdir(parents=True, exist_ok=True)
total = 0
modes = native_modes('linux')
assert len(modes) == 6
for mode, features in modes:
    build = subprocess.run(
        ['cargo', 'test', '--locked', '-p', 'turnloop-contract',
         '--test', 'allocations', *features, '--no-run', '--message-format=json'],
        text=True, stdout=subprocess.PIPE, check=True)
    artifacts = [json.loads(line) for line in build.stdout.splitlines()
                 if line.startswith('{')]
    binaries = [a['executable'] for a in artifacts
                if a.get('reason') == 'compiler-artifact'
                and a['target']['name'] == 'allocations' and a.get('executable')]
    assert len(binaries) == 1, binaries
    for iteration in range(1, 201):
        run = subprocess.run(
            [binaries[0], name, '--exact', '--test-threads=1', '--nocapture'],
            text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        log = logs / f'{mode}-{iteration:03}.log'
        log.write_text(run.stdout)
        if run.returncode or not re.search(
                r'test result: ok\. 1 passed; 0 failed; 0 ignored;', run.stdout):
            print(run.stdout)
            raise SystemExit(f'FAIL {mode} iteration {iteration}: {log}')
        total += 1
        if iteration % 25 == 0:
            print(f'PASS {mode} {iteration}/200', flush=True)
assert total == 1200
print('PASS 1200 exact UDP allocation tests across all six Linux modes')
PY
```

The macOS campaign uses the same build/artifact/count checks with
`native_modes(sys.platform)`: default, executor, all-features. No Linux runtime
pass is inferred from that campaign or the cross-compilation matrix.
