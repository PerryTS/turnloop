//! The published decoder must retain its tables across independent frames.
use std::alloc::{GlobalAlloc, Layout, System};
use turnloop_zstd_decoder::decoding::FrameDecoder;
// P3 has one guest agent, but its allocator can run before task-local storage
// exists. Keep only that target's counter in statics; every threaded target uses
// const-initialized TLS, including wasm with atomics.
#[cfg(all(target_os = "wasi", target_env = "p3"))]
mod tracking {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
    pub fn count() {
        if ENABLED.load(Relaxed) {
            ALLOCATIONS.fetch_add(1, Relaxed);
        }
    }
    pub fn start() {
        ALLOCATIONS.store(0, Relaxed);
        ENABLED.store(true, Relaxed);
    }
    pub fn finish() -> usize {
        ENABLED.store(false, Relaxed);
        ALLOCATIONS.load(Relaxed)
    }
}
#[cfg(not(all(target_os = "wasi", target_env = "p3")))]
mod tracking {
    use std::cell::Cell;
    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }
    pub fn count() {
        if ENABLED.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        }
    }
    pub fn start() {
        ALLOCATIONS.set(0);
        ENABLED.set(true);
    }
    pub fn finish() -> usize {
        ENABLED.set(false);
        ALLOCATIONS.get()
    }
}
struct Counter;
// SAFETY: allocation ownership and layouts are forwarded unchanged to System.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tracking::count();
        // SAFETY: the caller provides a valid nonzero layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller provides the matching live allocation and layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        tracking::count();
        // SAFETY: the caller provides the live allocation, original layout and valid new size.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;

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
    tracking::start();
    for _ in 0..1000 {
        decode();
    }
    let allocations = tracking::finish();
    assert_eq!(frames, 1005);
    assert_eq!(
        allocations, 0,
        "a published decoder cannot allocate per frame"
    );
}

// A standalone harness avoids libtest's WASI 0.3 CLI-argument lowering, which
// invokes the compiler-generated custom allocator shim without a valid stack.
// Each existing test runs unconditionally; a panic fails the test executable.
fn main() {
    tracking::start();
    let probe = std::hint::black_box(Box::new([7_u8; 64]));
    std::hint::black_box(&probe);
    drop(probe);
    assert!(
        tracking::finish() > 0,
        "allocation instrumentation must detect work"
    );
    default_sequence_tables_reuse_allocations_and_preserve_bytes();
    println!("test default_sequence_tables_reuse_allocations_and_preserve_bytes ... ok");
    println!("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;");
}
