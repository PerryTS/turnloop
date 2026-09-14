//! Test-only allocator instrumentation; production libraries forbid unsafe.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {static TRACK:Cell<bool>=const {Cell::new(false)};static COUNT:Cell<usize>=const {Cell::new(0)};}
struct Counter;
#[global_allocator]
static ALLOCATOR: Counter = Counter;
fn count() {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNT.try_with(|n| n.set(n.get() + 1));
    }
}
// SAFETY: each method forwards the unchanged allocation contract to System.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(); /* SAFETY: forwards caller's valid Layout. */
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(); /* SAFETY: forwards caller's valid Layout. */
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        /* SAFETY: pointer and Layout are forwarded unchanged to their allocator. */
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(); /* SAFETY: forwards the caller's allocation and new size. */
        unsafe { System.realloc(ptr, layout, size) }
    }
}
pub fn allocations(work: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TRACK.set(false);
        }
    }
    COUNT.set(0);
    TRACK.set(true);
    let reset = Reset;
    work();
    drop(reset);
    COUNT.get()
}
