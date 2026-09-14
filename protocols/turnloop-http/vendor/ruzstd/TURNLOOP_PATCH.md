# Local allocation patch

Upstream ruzstd 0.8.3, MIT. The downloaded .crate SHA-256 was verified against Cargo.lock before copying: `a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8`. Original source commit is in `.cargo_vcs_info.json`.

Only protocol implementation changes: `FSETable::build_from_probabilities` clears/extends its existing probability vector; default sequence-table distributions are borrowed slices instead of temporary Vecs. This removes six allocations per default-table zstd frame. No decoding semantics or tests were weakened. Measured by `tests/allocations.rs` on wasm32-wasip2. All other source files are upstream copies. Remove this vendor override when upstream ships equivalent reuse (respecting the 7-day soak).

The normalized manifest omits upstream benchmark/dev-only dependencies; benchmark source and unit tests remain in the source snapshot, but are not workspace targets.
