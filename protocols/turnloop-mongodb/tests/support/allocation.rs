//! Count only the thread synchronously executing the measured closure. Libtest's
//! receiver can allocate concurrently even with `--test-threads=1`.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

struct Counter;
#[global_allocator]
static ALLOCATOR: Counter = Counter;

pub fn is_tracking() -> bool {
    TRACK.try_with(Cell::get).unwrap_or(false)
}

fn count() {
    if is_tracking() {
        let _ = COUNT.try_with(|n| n.set(n.get() + 1));
    }
}

// SAFETY: every operation forwards the caller's unchanged allocation contract
// to System. Const-initialized TLS Cells neither allocate nor need destruction.
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: forwards the caller's valid layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: forwards the caller's valid layout.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: forwards the live allocation, matching layout and new size.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: pointer/layout identify a prior System allocation.
        unsafe { System.dealloc(ptr, layout) }
    }
}

pub fn allocations(work: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TRACK.set(false);
        }
    }
    assert!(!is_tracking(), "allocation measurements cannot nest");
    COUNT.set(0);
    TRACK.set(true);
    let reset = Reset;
    work();
    drop(reset);
    COUNT.get()
}

#[test]
fn allocator_counts_alloc_zeroed_and_realloc_on_the_measured_thread() {
    let layout = Layout::from_size_align(64, 8).expect("valid layout");
    let grown = Layout::from_size_align(128, 8).expect("valid layout");
    let mut pointer = std::ptr::null_mut();
    assert_eq!(
        allocations(|| {
            // SAFETY: nonzero layout; the returned allocation is checked below.
            pointer = unsafe { std::alloc::alloc(std::hint::black_box(layout)) };
            assert!(!pointer.is_null());
        }),
        1
    );
    assert_eq!(
        allocations(|| {
            // SAFETY: pointer is live, with the original layout and nonzero new size.
            pointer = unsafe { std::alloc::realloc(pointer, layout, grown.size()) };
            assert!(!pointer.is_null());
        }),
        1
    );
    // SAFETY: realloc succeeded and pointer now has the grown layout.
    unsafe { std::alloc::dealloc(pointer, grown) };
    assert_eq!(
        allocations(|| {
            // SAFETY: valid nonzero layout; allocation is checked before reading.
            pointer = unsafe { std::alloc::alloc_zeroed(std::hint::black_box(layout)) };
            assert!(!pointer.is_null());
            // SAFETY: all layout.size() bytes were allocated and initialized above.
            let bytes = unsafe { std::slice::from_raw_parts(pointer, layout.size()) };
            assert!(bytes.iter().all(|&b| b == 0));
        }),
        1
    );
    // SAFETY: pointer is the live zeroed allocation with its matching layout.
    unsafe { std::alloc::dealloc(pointer, layout) };
    assert_eq!(allocations(|| {}), 0, "each measurement resets its count");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn simultaneous_measurements_count_only_their_own_thread() {
    use std::sync::{Arc, Barrier};
    let rendezvous = Arc::new(Barrier::new(2));
    let peer = Arc::clone(&rendezvous);
    let worker = std::thread::spawn(move || {
        // Warm the synchronization machinery before either measurement.
        peer.wait();
        peer.wait();
        let inherited_tracking = is_tracking();
        let count = allocations(|| {
            for value in [7_u8, 9] {
                let bytes = std::hint::black_box(Box::new([value; 64]));
                assert_eq!(bytes[63], value);
                drop(bytes);
            }
        });
        peer.wait();
        (count, inherited_tracking)
    });
    rendezvous.wait();
    let owner_count = allocations(|| {
        rendezvous.wait();
        rendezvous.wait();
    });
    let (worker_count, inherited_tracking) = worker.join().expect("allocation worker");
    assert!(
        !inherited_tracking,
        "the parent's counter must not enable the worker"
    );
    assert_eq!(worker_count, 2, "worker did both allocations");
    assert_eq!(
        owner_count, 0,
        "concurrent allocations belong to the worker"
    );
}

#[test]
fn measurement_disables_counting_after_a_panic() {
    let panic = std::panic::catch_unwind(|| allocations(|| panic!("measured failure")));
    assert!(
        panic.is_err(),
        "the measured closure must execute and panic"
    );
    assert!(!is_tracking(), "unwinding must end the measurement");
    assert_eq!(allocations(|| {}), 0);
    assert_eq!(
        allocations(|| drop(std::hint::black_box(Box::new([3_u8; 64])))),
        1,
        "a subsequent measurement still detects allocations"
    );
}
