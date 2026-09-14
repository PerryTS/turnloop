# turnloop-zstd-decoder

A publishable fork of **ruzstd 0.8.3**, by Moritz Borcherding, under the
upstream MIT [license](https://github.com/PerryTS/turnloop/blob/main/protocols/turnloop-zstd-decoder/LICENSE). Upstream: <https://github.com/KillingSpark/zstd-rs>.
The fork retains sequence-table allocations across decoded frames. HTTP uses it
on WASI and browser wasm; the library is portable Rust on native targets too.

Use `decoding::FrameDecoder` and retain it across `reset` calls. Warm up with the
largest expected frame shape; growth may allocate. The public decoder API follows
ruzstd 0.8.3. No consuming-workspace patch is required.

See `UPSTREAM.md` for provenance and the complete fork scope. The upstream README
is preserved as `UPSTREAM-README.md`. The Rust standard library's internal build feature is
not exposed by this standalone package.
