#[cfg(loom)]
pub(crate) use loom::cell::UnsafeCell;
#[cfg(loom)]
pub(crate) use loom::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[cfg(not(loom))]
pub(crate) use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
#[cfg(not(loom))]
pub(crate) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);
#[cfg(not(loom))]
impl<T> UnsafeCell<T> {
    pub fn new(v: T) -> Self {
        Self(std::cell::UnsafeCell::new(v))
    }
    pub fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        f(self.0.get())
    }
}
