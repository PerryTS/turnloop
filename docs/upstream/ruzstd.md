# ruzstd upstream submission draft

Not submitted. Maintainers can copy the following title and body into an upstream
issue or pull request at https://github.com/KillingSpark/zstd-rs.

## Title

Reuse FSE probability storage and borrow predefined sequence distributions

## Body

Repeated independent Zstandard frames allocate even when a `FrameDecoder` is
retained and reset after warm-up. With predefined literal-length, offset and
match-length tables, I measure six allocations per frame: three temporary
`Vec::from` distributions and three `probs.to_vec()` replacements inside
`FSETable::build_from_probabilities`. Both 0.8.3 and 0.9.0 contain these sites.
The public frame APIs cannot opt out of these private table-building allocations.
Disabling the hash feature affects checksums, not these allocations.

Please retain the existing `symbol_probabilities` allocation with `clear` followed
by `extend_from_slice`, and pass borrowed slices for predefined distributions.
The table contents and error behavior are unchanged. Larger distributions can
still grow storage. This does not promise allocation-free construction or every
possible dictionary/frame shape.

A counting-allocator regression is below. The independent fixture was generated
by Node 26 `zlib.zstdCompressSync`; it expands to the exact bytes asserted by the
test. On upstream 0.8.3 the final assertion sees 6000 allocations across 1000
frames; with the patch it sees zero. `reset`, full consumption, output bytes,
completion and frame counts are all asserted. No allocation threshold is relaxed.

The original source is ruzstd 0.8.3, commit
`1c7aafb8e668f9ea2f44e6155bb7429e2442a3c1`, directory `ruzstd/`, MIT,
by Moritz Borcherding. Registry archive SHA-256:
`a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8`.
The soaked 0.9.0 archive was also inspected (SHA-256
`a252f5e20f038fe7b4ea53e073e65398d652c864cc162fc77c56c2f13717b888`).

### Patch (against 0.8.3)

```diff
--- a/ruzstd/src/fse/fse_decoder.rs
+++ b/ruzstd/src/fse/fse_decoder.rs
@@ -131,7 +131,8 @@
         if acc_log == 0 {
             return Err(FSETableError::AccLogIsZero);
         }
-        self.symbol_probabilities = probs.to_vec();
+        self.symbol_probabilities.clear();
+        self.symbol_probabilities.extend_from_slice(probs);
         self.accuracy_log = acc_log;
         self.build_decoding_table()
     }
--- a/ruzstd/src/decoding/sequence_section_decoder.rs
+++ b/ruzstd/src/decoding/sequence_section_decoder.rs
@@ -326,7 +326,7 @@
             vprintln!("Use predefined ll table");
             scratch.literal_lengths.build_from_probabilities(
                 LL_DEFAULT_ACC_LOG,
-                &Vec::from(&LITERALS_LENGTH_DEFAULT_DISTRIBUTION[..]),
+                &LITERALS_LENGTH_DEFAULT_DISTRIBUTION[..],
             )?;
             scratch.ll_rle = None;
         }
@@ -359,10 +359,9 @@
         }
         ModeType::Predefined => {
             vprintln!("Use predefined of table");
-            scratch.offsets.build_from_probabilities(
-                OF_DEFAULT_ACC_LOG,
-                &Vec::from(&OFFSET_DEFAULT_DISTRIBUTION[..]),
-            )?;
+            scratch
+                .offsets
+                .build_from_probabilities(OF_DEFAULT_ACC_LOG, &OFFSET_DEFAULT_DISTRIBUTION[..])?;
             scratch.of_rle = None;
         }
         ModeType::Repeat => {
@@ -396,7 +395,7 @@
             vprintln!("Use predefined ml table");
             scratch.match_lengths.build_from_probabilities(
                 ML_DEFAULT_ACC_LOG,
-                &Vec::from(&MATCH_LENGTH_DEFAULT_DISTRIBUTION[..]),
+                &MATCH_LENGTH_DEFAULT_DISTRIBUTION[..],
             )?;
             scratch.ml_rle = None;
         }
@@ -447,7 +446,7 @@
     table
         .build_from_probabilities(
             LL_DEFAULT_ACC_LOG,
-            &Vec::from(&LITERALS_LENGTH_DEFAULT_DISTRIBUTION[..]),
+            &LITERALS_LENGTH_DEFAULT_DISTRIBUTION[..],
         )
         .unwrap();
 
```

### Regression test (`ruzstd/tests/reuse.rs`)

```rust
//! The published decoder must retain its tables across independent frames.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
use ruzstd::decoding::FrameDecoder;
thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
struct Counter;
fn count() {
    if COUNTING.get() {
        ALLOCATIONS.set(ALLOCATIONS.get() + 1);
    }
}
// SAFETY: allocation ownership and layouts are forwarded unchanged to System.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller provides a valid nonzero layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller provides the matching live allocation and layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: the caller provides the live allocation, original layout and valid new size.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;

#[test]
fn default_sequence_tables_reuse_allocations_and_preserve_bytes() {
    let encoded = [
        0x28, 0xb5, 0x2f, 0xfd, 0x20, 0x38, 0xcd, 0x00, 0x00, 0x98, 0x61, 0x6c, 0x6c, 0x6f, 0x63,
        0x61, 0x74, 0x69, 0x6f, 0x6e, 0x20, 0x70, 0x72, 0x6f, 0x66, 0x69, 0x6c, 0x65, 0x20, 0x01,
        0x00, 0xd9, 0x33, 0xc3,
    ];
    let body = b"allocation profile allocation profile allocation profile";
    let mut decoder = FrameDecoder::new();
    let mut output = [0; 4096];
    let mut frames = 0;
    let mut decode = || {
        let mut input = encoded.as_slice();
        decoder.reset(&mut input).expect("valid frame header");
        let (read, written) = decoder
            .decode_from_to(input, &mut output)
            .expect("valid sequences");
        assert_eq!(read, input.len());
        assert_eq!(&output[..written], body);
        assert!(decoder.is_finished());
        assert_eq!(decoder.can_collect(), 0);
        frames += 1;
    };
    for _ in 0..5 {
        decode();
    }
    ALLOCATIONS.set(0);
    COUNTING.set(true);
    for _ in 0..1000 {
        decode();
    }
    COUNTING.set(false);
    assert_eq!(frames, 1005);
    assert_eq!(
        ALLOCATIONS.get(),
        0,
        "a published decoder cannot allocate per frame"
    );
}
```

Run `cargo test -p ruzstd --test reuse` before and after applying the patch.
Our independent downstream crate also runs this regression on WASI, plus HTTP
streaming/reset/corruption/truncation/concatenation tests and strict allocation
gates. Upstream corpus, dictionary and checked-in fuzz regressions are retained.

## Downstream integration notes (not part of the issue body)

Until a fixed upstream release has passed the unchanged seven-day soak, HTTP
uses a versioned `turnloop-zstd-decoder` dependency. There is no root patch and no
patch requirement for crates.io consumers. Native HTTP defaults to reference
zstd; `pure-rust-zstd` exercises the portable decoder on native CI too.

The fork also adapts crate names, modern lint/safety comments, native-only
C-reference tests, and restores upstream fixtures omitted from the registry
archive. The internal rustc-dep-of-std build mode is omitted from this standalone
package. Those packaging changes are deliberately absent from the proposed FSE
patch. `dictionary/frequency.rs` uses i64 instead of isize so its 2654435761
constant and multiplication compile with identical 64-bit arithmetic on wasm32;
that independent portability fix should be submitted separately.
