# wasm3 lane report — verification in progress

Continuation on `lane/wasm2`, 2026-09-14. Read the previous report, DESIGN.md,
CONTRIBUTING.md, integration report and wasm/core/CI/Mongo lane reports completely.
No applicable AGENTS.md. No Git mutations; integrator checkpoints are external.

## Implemented

- p3 UDP canonical lists now use retained release storage shared across live loops,
  with explicit consumption/cancellation ownership. Original 100-datagram gate
  and expanded concurrent/cancellation/IPv6 gate pass at **zero allocations**.
- Whole p3 workspace, including MongoDB/BSON, builds and lints. A p3-only custom
  getrandom linker shim calls scalar wasi:random, with byte/BSON and zero-allocation
  subjects. No registry dependency update or soak exception.
- Bare p2/p3 timer programs reproduce Wasmtime 46's ~1 ms lateness. Per the
  integrator's decision, WASI release median ≤2 ms is documented in DESIGN §7.4
  and §7.6. Native <500 µs and all no-spin limits are unchanged. Raw numbers live
  in docs/wasm.md.
- p3 allocation gates explicitly restricted to release because the pinned debug
  custom allocator traps during pre-main get-arguments canonical lowering. Debug
  semantics/no-spin and release semantics/precision/allocations remain required.
- docs/upstream/wasi-p3-wait.md records the unproven yield/cancellation/context
  bounds and upstream features required for promotion. p3 remains experimental.
- Linux browser CI pins checksum-verified Chrome for Testing/matching driver,
  Firefox/geckodriver; owned drivers write verbose/trace failure logs; zero browser
  execution fails. Installer and adversarial lifecycle tests pass.

## Verification so far

PASS: full p2 runner (3 unit, 17 debug +17 release contracts, 5 release allocation
subjects); p3 contracts (18 debug +18 release), original/expanded release allocation
subjects (6), canonical lifetime unit test; native all-feature Clippy/stable check;
p3 whole-workspace release Clippy; workflow lint; 18 CI adversarial tests; Linux
browser asset download/checksum/extraction verification on Mac.

Broad native/cross-target gates and final Node/browser reruns are still running.
Every command is recorded in `.tools/wasm3/commands.jsonl`; the final report will
include their full PASS/FAIL/UNRUN ledger. Browser execution remains required on
Linux: the current Mac attempt reached ChromeDriver but Chrome session creation
failed (`Chrome instance exited`); the earlier integrator reproduced driver
SIGKILL even outside the sandbox. Neither is a browser test pass.

## Deviations and next steps

Only the explicitly authorized WASI precision/release-profile decisions change
verification scope; zero allocations and no-spin limits remain unchanged. The
new entropy shim is an explicit shared linker crate so independently used protocol
crates do not introduce duplicate custom symbols. Downstream users must supply the
p3 rustflag themselves. Allocation setup retains UDP slot capacity until the last
backend drops; debug continues to use std-owned canonical lists.

Complete broad verification, audit final changes, document every remaining
platform limitation and hand off the coherent tree for integration. Linux/Windows
runtime, GitHub browser execution and unavailable SQL fixtures are UNRUN here.
