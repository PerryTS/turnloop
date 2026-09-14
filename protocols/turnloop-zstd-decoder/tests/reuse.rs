//! The published decoder must retain its tables across independent frames.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
use turnloop_zstd_decoder::decoding::FrameDecoder;
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
