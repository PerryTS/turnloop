//! Bounded lock-free slot queue. Posts have no FIFO ordering guarantee.
//! A stalled producer owns only its own slot; consumers can drain every other
//! published slot. Scans are bounded by capacity and never wait for a producer.
use crate::sync::{AtomicUsize, Ordering, UnsafeCell};
use std::mem::MaybeUninit;
struct Slot<T> {
    state: AtomicUsize,
    value: UnsafeCell<MaybeUninit<T>>,
}
pub(crate) struct Queue<T> {
    slots: Box<[Slot<T>]>,
    enqueue: AtomicUsize,
    dequeue: AtomicUsize,
    occupied: AtomicUsize,
}
// SAFETY: state 1 belongs exclusively to a producer, state 3 to a consumer.
// Release/Acquire transfers initialized Send values between these owners.
unsafe impl<T: Send> Sync for Queue<T> {}
// SAFETY: no slot is externally borrowed; only Send values can cross threads.
unsafe impl<T: Send> Send for Queue<T> {}
impl<T> Queue<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity >= 2 && capacity.is_power_of_two());
        Self {
            slots: (0..capacity)
                .map(|_| Slot {
                    state: AtomicUsize::new(0),
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                })
                .collect(),
            enqueue: AtomicUsize::new(0),
            dequeue: AtomicUsize::new(0),
            occupied: AtomicUsize::new(0),
        }
    }
    pub fn push(&self, value: T) -> std::result::Result<(), T> {
        let start = self.enqueue.fetch_add(1, Ordering::Relaxed);
        for offset in 0..self.slots.len() {
            let s = &self.slots[start.wrapping_add(offset) & (self.slots.len() - 1)];
            if s.state
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.occupied.fetch_add(1, Ordering::Relaxed);
                s.value.with_mut(|p| {
                    // SAFETY: state 1 is held exclusively by this producer.
                    unsafe {
                        (*p).write(value);
                    }
                });
                s.state.store(2, Ordering::Release);
                return Ok(());
            }
        }
        Err(value)
    }
    pub fn pop(&self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        let start = self.dequeue.fetch_add(1, Ordering::Relaxed);
        for offset in 0..self.slots.len() {
            let s = &self.slots[start.wrapping_add(offset) & (self.slots.len() - 1)];
            if s.state
                .compare_exchange(2, 3, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                let v = s.value.with_mut(|p| {
                    // SAFETY: state 3 exclusively owns the initialized value.
                    unsafe { (*p).assume_init_read() }
                });
                self.occupied.fetch_sub(1, Ordering::Relaxed);
                s.state.store(0, Ordering::Release);
                return Some(v);
            }
        }
        None
    }
    pub fn is_empty(&self) -> bool {
        self.occupied.load(Ordering::Acquire) == 0
    }
}
impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        while self.pop().is_some() {}
    }
}

#[cfg(all(test, loom))]
mod models {
    use super::*;
    use crate::sync::Arc;
    #[test]
    fn two_producers_deliver_once() {
        loom::model(|| {
            let q = Arc::new(Queue::new(2));
            let a = q.clone();
            let b = q.clone();
            let t1 = loom::thread::spawn(move || a.push(11).expect("slot"));
            let t2 = loom::thread::spawn(move || b.push(22).expect("slot"));
            t1.join().expect("producer");
            t2.join().expect("producer");
            let a = q.pop().expect("first");
            let b = q.pop().expect("second");
            assert_eq!(a + b, 33);
            assert_ne!(a, b);
            assert!(q.pop().is_none());
            q.push(44).expect("reused slot");
            assert_eq!(q.pop(), Some(44));
        });
    }
}

#[cfg(all(test, loom))]
mod publication_models {
    use super::*;
    use crate::sync::Arc;
    #[test]
    fn consumer_races_publication_and_slot_reuse() {
        loom::model(|| {
            let q = Arc::new(Queue::new(2));
            q.push(10).expect("first value");
            let producer = q.clone();
            let t = loom::thread::spawn(move || producer.push(20).expect("second value"));
            let first = q.pop();
            t.join().expect("producer");
            let mut sum = first.unwrap_or(0);
            let mut count = usize::from(first.is_some());
            while let Some(n) = q.pop() {
                sum += n;
                count += 1;
            }
            assert_eq!(sum, 30);
            assert_eq!(count, 2);
            q.push(30).expect("recycle");
            assert_eq!(q.pop(), Some(30));
            assert!(q.is_empty());
        });
    }
}
