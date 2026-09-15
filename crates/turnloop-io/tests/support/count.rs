//! Per-thread accounting around client future polls; unrelated server tasks and
//! test-harness work cannot hide or inflate the measured adapter allocations.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}
struct Counter;
fn record() {
    if ACTIVE.try_with(Cell::get).unwrap_or(false) {
        let _ = COUNT.try_with(|n| n.set(n.get() + 1));
    }
}
// SAFETY: all allocation contracts, layouts and pointers are forwarded to System.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        record();
        // SAFETY: unchanged valid caller layout.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        record();
        // SAFETY: unchanged valid caller layout.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        record();
        // SAFETY: unchanged caller pointer/layout/new size.
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: unchanged pointer/layout from the caller.
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static ALLOCATOR: Counter = Counter;
struct Scope(bool);
impl Scope {
    fn enter() -> Self {
        Self(ACTIVE.with(|a| a.replace(true)))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|a| a.set(self.0));
    }
}
pub struct Measured<F> {
    future: F,
    count: usize,
}
pub fn measure<F: Future>(future: F) -> Measured<F> {
    Measured { future, count: 0 }
}
impl<F: Future> Future for Measured<F> {
    type Output = (F::Output, usize);
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: structural pin projection; future is never moved, including Drop.
        let this = unsafe { self.get_unchecked_mut() };
        let before = COUNT.with(Cell::get);
        let scope = Scope::enter();
        // SAFETY: future remains pinned for the lifetime of this wrapper.
        let result = unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx);
        drop(scope);
        this.count += COUNT.with(Cell::get) - before;
        result.map(|v| (v, this.count))
    }
}
pub async fn prove_counter() {
    let (_, n) = measure(async {
        std::hint::black_box(Box::new([0u8; 128]));
    })
    .await;
    assert!(n > 0, "allocation counter must observe the subject thread");
}
