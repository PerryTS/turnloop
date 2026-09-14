# Local allocation patch

Upstream ruzstd 0.8.3, MIT. The downloaded .crate SHA-256 was verified against Cargo.lock before copying: `a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8`. Original source commit is in `.cargo_vcs_info.json`.

Original allocation patch: `FSETable::build_from_probabilities` clears/extends its existing probability vector; default sequence-table distributions are borrowed slices instead of temporary Vecs. This removes six allocations per default-table zstd frame. No decoding semantics or tests were weakened. Measured by HTTP `tests/allocations.rs` and this crate’s `tests/reuse.rs` on native/WASI. All other source files are upstream copies. Replace this fork when upstream ships equivalent reuse (respecting the 7-day soak).


Source commit: `1c7aafb8e668f9ea2f44e6155bb7429e2442a3c1` (`ruzstd/`).
This fork is published independently, with explicit registry versions on every
workspace edge. Upstream source/tests and their fixture expectations are retained;
portable tests use embedded fixture bytes on wasm, and C-reference encoder tests
remain native-only. Lint/safety documentation and package naming are adapted to
the unified workspace. Internal rustc-only build dependencies are omitted.
Maintainer submission text: `docs/upstream/ruzstd.md` in the turnloop repository.

Additional portability change: dictionary/frequency.rs uses i64 hash arithmetic
instead of isize (the prime 2654435761 does not fit wasm32 isize). Existing
frequency expectations are retained. Rustfix-generated explicit unsafe blocks
and documented pointer invariants enforce the workspace unsafe lint. All native
upstream test cases remain active; reference-C-only tests/benchmarks are gated
away from WASM. The restore contains 101 decode frames, 207 dictionary frames,
and 47 fuzz artifacts, all verified against upstream Git blob hashes.
