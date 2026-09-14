use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct Counting;
static ACTIVE: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
fn record() { if ACTIVE.load(Ordering::Relaxed) { COUNT.fetch_add(1, Ordering::Relaxed); } }
// SAFETY: forwards the complete allocator contract unchanged to System. Counter
// updates use initialized statics and never allocate or call JavaScript.
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
pub fn measure<T>(f: impl FnOnce() -> T) -> T {
    COUNT.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
    let result=f();
    ACTIVE.store(false, Ordering::Relaxed);
    assert_eq!(COUNT.load(Ordering::Relaxed),0,"Rust steady-state allocations");
    result
}
