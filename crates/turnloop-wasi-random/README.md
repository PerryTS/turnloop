# turnloop-wasi-random

Explicit linker shim for `wasm32-wasip3` consumers of getrandom 0.3.4/0.4.3.
The workspace's target-specific rustflags select `getrandom_backend="custom"`;
this crate defines its single `__getrandom_v03_custom` symbol. Both pinned
getrandom generations declare that symbol. Link once with
`use turnloop_wasi_random as _;`. Do not supply another custom definition.
Downstream applications must also supply the p3-only rustflag (Cargo config is
not inherited from dependencies). Other targets use their normal entropy backend.

The implementation fills even uninitialized destinations with scalar
`wasi:random/random@0.3.0.get-random-u64` calls, without lists or heap allocation.
There is no deterministic fallback. WASI's random capability is required.
See `docs/wasm.md` in the repository for selection evidence and executed tests.
