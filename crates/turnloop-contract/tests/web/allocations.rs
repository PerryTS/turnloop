use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
struct Counting;
// Each worker/agent measures only its own synchronous turn.
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}
fn record() {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNT.try_with(|n| n.set(n.get() + 1));
    }
}
// SAFETY: forwards the complete allocator contract unchanged to System. Counter
// updates use const-initialized TLS and never allocate or call JavaScript.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: caller supplied valid layout, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: caller supplied valid layout, forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        // SAFETY: pointer/layout/new size obey the allocator contract.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout identify a prior System allocation.
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[wasm_bindgen_test::wasm_bindgen_test]
fn allocator_detects_work_on_the_measured_agent() {
    COUNT.set(0);
    ACTIVE.set(true);
    let bytes = std::hint::black_box(Box::new([7_u8; 64]));
    ACTIVE.set(false);
    assert_eq!(bytes[63], 7);
    assert_eq!(COUNT.get(), 1, "the guest allocator must detect real work");
}

pub fn measure<T>(f: impl FnOnce() -> T) -> T {
    COUNT.set(0);
    ACTIVE.set(true);
    let result = f();
    ACTIVE.set(false);
    assert_eq!(COUNT.get(), 0, "Rust steady-state allocations");
    result
}
