# tl-i03 — compressed command allocations on WASI

Status: **implemented and verified on both WASI targets**. Base `3cce439`, branch
`lane/tl-i03`, macOS arm64 (Darwin 25.5.0). The existing MongoDB zero-allocation
gate now passes on `wasm32-wasip2` and `wasm32-wasip3` in all four
compressed/uncompressed × raw/coordinator modes, and it is wired into the
required WASI CI selection. MySQL was checked under its own target allocator,
had exactly the same defect, and is fixed and gated the same way. No dependency
was forked, added, removed or version-bumped; `Cargo.lock` is untouched; no
threshold was relaxed and no test was weakened.

## Root cause

`protocols/turnloop-mongodb/src/connection.rs` compressed each `OP_COMPRESSED`
command with one retained `flate2::Compress`, calling `Compress::reset()` before
every message because each MongoDB compressed message is its own RFC 1950 stream.
`reset()` is allocation-free natively and is *not* allocation-free on wasm32:

- `miniz_oxide-0.9.1/src/deflate/core.rs:1583-1606` stores the LZ code buffer as
  `Box<[u8; LZ_CODE_BUF_SIZE]>` on `target_arch = "wasm32"` (an inline array
  everywhere else), and its wasm constructor builds it with
  `vec![0; LZ_CODE_BUF_SIZE].into_boxed_slice()`.
- `CompressorOxide::reset()` (`core.rs:496`) replaces that buffer wholesale:
  `self.lz = LZOxide::new();`.

So one reset per message is one heap allocation per message on every wasm target
— WASI 0.2, WASI 0.3 and the web — which DESIGN §3 goal 3 / §10.1 forbid. This is
the defect `docs/lanes/proto-fix1.md` recorded as an open release blocker and
`docs/AUDIT_EVIDENCE.md` reproduced as 1,000 allocations for 1,000 commands.
No warm-up can absorb it: the allocation is repeated by every reset.

`miniz_oxide` 0.9.1 is the newest published version (crates.io index checked:
0.7.2 … 0.9.1), so there is no upstream release to move to.

## Fix

New `zlib.rs` module in each affected crate (`protocols/turnloop-mongodb/src/zlib.rs`,
`protocols/turnloop-mysql/src/zlib.rs`). **Nothing resets any more.** The
compressor is created in raw-deflate mode once per connection and kept for its
lifetime; each message is emitted as its own complete zlib stream by writing the
RFC 1950 wrapper directly around a full-flushed deflate segment:

1. Write the two-byte zlib header `78 9C` (CM 8, CINFO 7, FDICT 0, FCHECK making
   the pair a multiple of 31 — RFC 1950 §2.2; FLEVEL is informational and inflate
   ignores it).
2. `Compress::compress_vec(input, out, FlushCompress::Full)`. flate2's documented
   contract for a full flush is that all pending output is written, the stream is
   left on a byte boundary, and the compression state is reset "so that
   decompression can restart from this point" — upstream `flate2` asserts exactly
   that in `src/mem.rs::test_full_flush`, and `miniz_oxide` implements it by
   emitting an empty stored block (`core.rs:1892`) and clearing the match
   dictionary and hash chains (`core.rs:2520`).
3. Append the two-byte final empty fixed-Huffman block `03 00` (BFINAL 1,
   BTYPE 01, end-of-block symbol). It is valid precisely because step 2 left the
   stream byte-aligned, and it is what makes the stream terminate for an
   `inflate`/`uncompress` caller.
4. Append the big-endian Adler-32 of the uncompressed message (RFC 1950 §9),
   computed by a small allocation-free helper.

Completion is proved rather than assumed: the helper checks the returned
`Status`, that `total_in` advanced by exactly `input.len()`, and that spare
capacity remained after the flush (`compress_vec` writes only into spare
capacity, so leftover capacity is the zlib idiom for "avail_out was never
exhausted", and it also holds the six-byte trailer). Callers reserve through
`bound(len)`, which is comfortably above zlib's own `deflateBound`, so the whole
append is allocation-free once the output buffer has warmed.

Cost: about six bytes per message versus a `Finish`-terminated stream (the
five-byte full-flush marker plus the two-byte terminator, less the padding a
`Finish` would have needed). Compression ratio is unchanged within a message;
cross-message history was never used, because every message was an independent
stream before this change too.

MySQL's compressed framing (`protocols/turnloop-mysql/src/codec.rs`) had the
identical `encoder.reset()`-per-frame shape and now uses the same helper; the
`encoded` scratch buffer is cleared and reserved instead of `resize`d, and the
"send it plain if compression did not help" decision now compares the produced
stream length instead of the (no longer per-message) `total_out`.

### Why not the alternatives

- **Fork `miniz_oxide`.** Avoided, as the item asks. It would have to be packaged
  for downstream consumers the way `turnloop-zstd-decoder` was, for a one-line
  upstream defect.
- **Switch flate2 to the `zlib-rs` backend.** Would add a large new
  unsafe-heavy dependency to every target and every crate that uses flate2, and
  restart the seven-day soak, to fix one reset path.
- **Accept the allocation on wasm only.** Rejected: DESIGN requires the budget in
  every configuration, and a per-target exception is exactly the untested
  configuration the repository's gate rules warn about.

## Evidence

All numbers are from this clone. "before" is `git checkout 3cce439 --
protocols/turnloop-mongodb protocols/turnloop-mysql` in the fixed tree, i.e. the
unchanged base sources under the same toolchains and the same wasmtime.

### The gate, per target (allocations over 1,000 measured commands)

| Suite | Target / profile | before | after |
| --- | --- | ---: | ---: |
| `turnloop-mongodb --test allocations` | native aarch64-apple-darwin, debug | 0 | 0 |
| `turnloop-mongodb --test allocations` | `wasm32-wasip2`, debug (CI `wasi-tests`) | **1000** (FAIL) | **0** (PASS) |
| `turnloop-mongodb --test allocations` | `wasm32-wasip2`, release | **1000** (FAIL) | **0** (PASS) |
| `turnloop-mongodb --test allocations` | `wasm32-wasip3`, release (CI `wasi-p3-release-tests`) | **1000** (FAIL) | **0** (PASS) |
| `turnloop-mysql --test allocations` | native aarch64-apple-darwin, debug | 0 | 0 |
| `turnloop-mysql --test allocations` | `wasm32-wasip2`, debug | **1000** (FAIL) | **0** (PASS) |
| `turnloop-mysql --test allocations` | `wasm32-wasip2`, release | **1000** (FAIL) | **0** (PASS) |
| `turnloop-mysql --test allocations` | `wasm32-wasip3`, release | **1000** (FAIL) | **0** (PASS) |

Before the fix the first compressed assertion aborted the wasm process, so the
compressed *coordinator* mode never ran at all on either target; it now runs.

### All four MongoDB modes, both WASI targets

Per-mode lines are printed by the gate itself (they are new; the counts they
print are asserted, not merely displayed). `wasm32-wasip2` debug and
`wasm32-wasip3` release both produced exactly:

```
zlib=false operation=false: 1000 measured commands, 2000 measured rows, 0 allocations
zlib=false operation=true: 1000 measured commands, 2000 measured rows, 0 allocations
zlib=true operation=false: 1000 measured commands, 2000 measured rows, 0 allocations
zlib=true operation=true: 1000 measured commands, 2000 measured rows, 0 allocations
```

plus, in the same binary, the positive allocator calibration
(`allocator_counts_alloc_zeroed_and_realloc_on_the_measured_thread`, which proves
alloc/alloc_zeroed/realloc each count exactly one on the measuring thread). The
allocator itself is untouched: no counting was disabled, scoped away or moved off
the measured thread.

MySQL prints, on both targets:

```
compression=false: 1000 measured pings, 0 allocations
compression=false: 1000 measured queries, 1010 rows, 0 allocations
compression=true: 1000 measured pings, 0 allocations
compression=true: 1000 measured queries, 1010 rows, 0 allocations
```

### The gate cannot pass without its subject

Two additions make "green" mean the compressed path actually ran:

- `tests/allocations.rs` now asserts the outgoing opcode inside the measured
  loop — `OP_COMPRESSED` when `zlib=true`, `OP_MSG` when not — and asserts the
  executed command count (1,002 per mode: two warm-ups plus 1,000 measured).
- `tests/protocol.rs::compressed_commands_are_independent_zlib_streams` runs 64
  consecutive commands on one connection and decodes each one with a **fresh**
  `flate2::read::ZlibDecoder` that never saw the previous message, then asserts
  the decompressed bytes equal, byte for byte, the `OP_MSG` an uncompressed
  connection produces for the same command, that the declared uncompressed size
  matches, and that all 64 frames were genuinely smaller than their input.

For MySQL, `codec.rs::consecutive_compressed_frames_decode_with_the_upstream_codec`
encodes 32 consecutive compressed frames and decodes them with
`mysql_common`'s *independent* compressed codec, which is what a real server
would do.

`src/zlib.rs` carries its own unit tests in both crates: RFC 1950 Adler-32
vectors (including a two-window input), 64 consecutive messages each decoded by a
fresh decompressor, a steady-state loop over incompressible input asserting the
output `Vec` never reallocates and never exceeds `bound`, and empty/one-byte
messages.

## CI wiring

`protocols/turnloop-mongodb/Cargo.toml` and `protocols/turnloop-mysql/Cargo.toml`:

```toml
wasi-tests = ["asynchronous", "allocations"]
wasi-p3-release-tests = ["asynchronous", "allocations"]
```

This is the same shape `turnloop-postgres` already uses, and it is the selection
`scripts/ci/run-tests.py` reads: the `protocol-wasi` suite runs every
`wasi-tests` target of every `protocol`/`codec`/`adapter` member, in release on
`wasm32-wasip3` for the targets also listed in `wasi-p3-release-tests`.
`checked_tests` fails the job on a non-zero exit and on a zero passed-test count,
so the gate can neither be skipped nor pass vacuously. Verified by running the
real runner, not by reading it — see the command ledger.

## MySQL finding

**MySQL has the same defect.** `protocols/turnloop-mysql/src/codec.rs` called
`encoder.reset()` once per compressed frame, and its own allocation gate
(`turnloop-mysql --test allocations`, which measures 1,000 compressed pings and
1,000 compressed 512-byte-row queries under the crate's own target allocator)
reported **1,000 allocations, expected 0** on `wasm32-wasip2` and
`wasm32-wasip3` — for the compressed configuration only, exactly like MongoDB.
That suite was **not** part of any WASI CI selection before this lane, which is
why it had never gone red. It is fixed with the same helper and added to both
WASI selections.

`turnloop-http` also depends on flate2 but only ever *decompresses*
(`flate2::Decompress`); `Decompress::reset` does not allocate on wasm (the 1,000
observed allocations correspond one-to-one with compressor resets while the
decompressor was resetting on the same iterations), so no other crate is
affected. No other `flate2::Compress` user exists in the workspace.

## Verification

Toolchains: pinned `nightly-2026-08-20` (default), `nightly-2026-09-07` for
`wasm32-wasip3`, stable 1.98.0. WASI runs use the checksum-verified wasi-sdk 34.0
installed by `scripts/ci/install-wasm-toolchain.py` (SHA-256 re-verified in this
clone) and wasmtime **46.0.0**, the version `scripts/ci/tools.json` pins.

| Command | Result |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings -D clippy::undocumented_unsafe_blocks` | PASS |
| Same, `--all-features` | PASS |
| Same, `--all-features --target wasm32-wasip2`, debug and release | PASS, PASS |
| Same, `--all-features --target wasm32-wasip3` (nightly-2026-09-07), debug and release | PASS, PASS |
| Same, `--all-features --target wasm32-unknown-unknown` | PASS |
| `cargo doc --locked --workspace --all-features --no-deps` with `RUSTDOCFLAGS=-D warnings` | PASS |
| `cargo +stable check --locked --workspace --all-targets --all-features` (stable is 1.97.1 here, i.e. the declared MSRV) | PASS |
| `cargo +1.98.0 check --locked --workspace --all-targets --all-features` | PASS |
| `cargo test --locked --workspace -- --test-threads=1` | PASS: **294 passed**, 13 ignored, 0 failed (64 groups) |
| `cargo test --locked --workspace --all-features -- --test-threads=1` | PASS: **361 passed**, 20 ignored, 0 failed (80 groups) |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip2` | PASS: 88 tests |
| `python3 scripts/ci/run-tests.py wasi --target wasm32-wasip3` | PASS: 91 tests |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip2` | PASS: 92 tests over 21 suites, **including the newly selected `turnloop-mongodb --test allocations` and `turnloop-mysql --test allocations`** |
| `python3 scripts/ci/run-tests.py protocol-wasi --target wasm32-wasip3` | PASS: 92 tests over 21 suites, same two new selections in release |
| `python3 scripts/test-servers.py --services mongodb run python3 scripts/ci/run-tests.py protocol --package turnloop-mongodb` | PASS: `real_mongodb` and `async_server`; the real-server case reports "standalone, 3-member replica set, SCRAM SHA-1/SHA-256/default, TLS, **zlib**, CRUD, cursors, transactions, retry identity and change stream events" — a live mongod accepted every new compressed command |
| `python3 scripts/test-servers.py --services mysql run python3 scripts/ci/run-tests.py protocol --package turnloop-mysql` | PASS: `server` (3, including `auth_prepared_transactions_compression_and_infile`) and `async_server`; a live mysqld accepted the new compressed frames |
| `bash scripts/ci/no-tokio.sh` | PASS: eight targets plus the union, default and all features |
| `python3 scripts/ci/soak.py` | PASS: 251 locked registry versions; only the pre-existing rustls exception; seven-day resolver policy still active |
| `python3 scripts/ci/check-paths.py` | PASS: 1492 tracked files |
| `python3 scripts/ci/feature_modes.py` | PASS: 6 public features, 18 required native CI arms |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py'` | PASS: 98 tests |
| `git diff --check` | PASS |
| Baseline reruns of the four WASI gate configurations on unmodified `3cce439` sources | FAIL as expected, 1000 allocations each — see the table above |
| Browser/Node runtime | **UNRUN.** Neither MongoDB nor MySQL has a web test target, so there is nothing to execute there; `wasm32-unknown-unknown` is covered by strict Clippy only. The defect and the fix are both keyed on `target_arch = "wasm32"`, which includes the web target. |
| Linux and Windows native runtime | **UNRUN** (no machine here). Both are pure-Rust paths with no platform code; the integrator's matrix covers them. |

## Deviations, dead ends and notes

- The first design considered was leaving `reset()` in place and forking
  `miniz_oxide` to retain its boxed LZ buffer. It was dropped: the item prefers no
  fork, and a fork would have to be published for downstream consumers.
  Swapping flate2 to its `zlib-rs` backend was also dropped — it adds a large new
  dependency to every crate and target and restarts the soak, to fix one reset.
- `FlushCompress::Full` was chosen over `Sync` deliberately: `Sync` flushes and
  byte-aligns but does **not** clear the match dictionary, so the following
  message would back-reference the previous one and would not decode standalone.
  The protocol/upstream-decoder tests above exist to keep that distinction honest.
- `mysql_common`'s `PacketCodec` decodes our frames in `codec.rs`'s new test, and
  the `turnloop-mysql` allocation suite already decoded the client's frames with
  it; that independent implementation is why the MySQL change is not resting on
  our own decompressor agreeing with our own compressor.
- `.tools/` holds this lane's logs (`.tools/tl-i03/*.log`), the wasi-sdk 34.0 that
  `scripts/ci/install-wasm-toolchain.py` verified by SHA-256 in this clone, and a
  symlink to a wasmtime 46.0.0 binary extracted from an archive whose SHA-256
  matches `scripts/ci/tools.json` exactly
  (`ab4bdab6ea42a3245cda91cdc6e0430491c4b78ecd643406fc1764ccddbdcd25`). `.tools/`
  is git-ignored; nothing under it is committed.
- No `Cargo.lock`, dependency, feature, soak-policy, workflow or DESIGN change was
  made. The only manifest edits are the two `wasi-tests` / `wasi-p3-release-tests`
  lines.

## For the integrator

These audit rows are now false and can be updated when the audit is refreshed;
this lane deliberately did not edit the audit documents:

- `docs/REMAINING_WORK.md` I03.
- `docs/AUDIT_EVIDENCE.md:104` "WASI 0.2 compressed MongoDB: FAIL, subject ran".
- `docs/AUDIT_MARKERS.md:119` ("MongoDB compressed allocations are omitted by the
  WASI manifest allowlist") and `:145`.
- `docs/DESIGN_AUDIT.md:34`, `:137` and `:150` where they cite the compressed
  WASI MongoDB failure and the unselected `Cargo.toml:41` allowlist.
- `docs/INTEGRATION_REPORT.md:110`.
- `docs/lanes/proto-fix1.md`'s "Open release blocker" and its next-step 2.
