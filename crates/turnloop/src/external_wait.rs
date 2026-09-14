//! A process-wide helper parks on registered host conditions and exact deadlines.
use crate::{Error, ErrorKind, Instant, Result};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

/// Shared host wait condition. Hosts may provide their own atomic storage and call
/// `notify` after changing it. No predicate callback runs inside a loop turn.
#[derive(Clone)]
pub struct WaitCondition {
    value: Arc<AtomicU64>,
    generation: Arc<AtomicU64>,
}
impl WaitCondition {
    /// Create shared atomic storage and start the single waiter helper if needed.
    pub fn new(value: u64) -> Result<Self> { Self::from_atomic(Arc::new(AtomicU64::new(value))) }
    /// Register host-owned atomic storage. Call `notify` after external mutations.
    pub fn from_atomic(value: Arc<AtomicU64>) -> Result<Self> {
        service::initialize()?;
        Ok(Self { value, generation: Arc::new(AtomicU64::new(0)) })
    }
    /// Read the current value with acquire ordering.
    pub fn load(&self) -> u64 { self.value.load(Ordering::Acquire) }
    /// Store a new value and wake registered waiters for this condition.
    pub fn store(&self, value: u64) { self.value.store(value, Ordering::Release); self.notify(); }
    /// Wake current registrations, including when the value remains unchanged.
    pub fn notify(&self) { service::notify(self); }
}
/// Why an external wait completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitResult {
    /// The value already differed from the expected value at registration.
    NotEqual,
    /// The host notified this condition after registration.
    Notified,
    /// The exact monotonic deadline expired.
    TimedOut,
}

#[cfg(not(target_arch = "wasm32"))]
mod service {
    use super::*;
    use crate::{OpId, blocking::{WorkOutput, WorkPort, WorkResult}};
    use std::sync::{Condvar, Mutex, OnceLock};
    const CAPACITY: usize = 16_384;
    struct Wait { op: OpId, port: Arc<WorkPort>, condition: WaitCondition, generation: u64, expected: u64, deadline: Option<Instant> }
    struct Service { waits: Mutex<Vec<Option<Wait>>>, changed: Condvar }
    static SERVICE: OnceLock<Result<Arc<Service>>> = OnceLock::new();
    fn get() -> Result<&'static Arc<Service>> {
        SERVICE.get_or_init(|| {
            let service = Arc::new(Service { waits: Mutex::new((0..CAPACITY).map(|_| None).collect()), changed: Condvar::new() });
            let s = service.clone();
            std::thread::Builder::new().name("turnloop-external-wait".into()).spawn(move || s.run()).map_err(Error::from)?;
            Ok(service)
        }).as_ref().map_err(|&e| e)
    }
    pub(super) fn initialize() -> Result<()> { get().map(|_| ()) }
    pub(super) fn notify(condition: &WaitCondition) {
        if let Ok(s) = get() {
            // Holding the same mutex as registration and parking closes lost-wake
            // windows even when the host changed its atomic outside this service.
            let _waits = s.waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            condition.generation.fetch_add(1, Ordering::Release);
            s.changed.notify_one();
        }
    }
    impl Service {
        fn run(&self) {
            let mut waits = self.waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                let now = Instant::now();
                let mut earliest: Option<Instant> = None;
                for slot in waits.iter_mut() {
                    let Some(w) = slot else { continue; };
                    let result = if w.condition.load() != w.expected || w.condition.generation.load(Ordering::Acquire) != w.generation {
                        Some(WaitResult::Notified)
                    } else if w.deadline.is_some_and(|d| d <= now) { Some(WaitResult::TimedOut) } else { None };
                    if let Some(result) = result {
                        let w = slot.take().expect("ready wait");
                        w.port.complete(WorkResult { op: w.op, result: Ok(WorkOutput::ExternalWait(result)) });
                    } else if let Some(at) = w.deadline { earliest = Some(earliest.map_or(at, |old| old.min(at))); }
                }
                waits = if let Some(at) = earliest {
                    self.changed.wait_timeout(waits, at.saturating_duration_since(Instant::now())).unwrap_or_else(std::sync::PoisonError::into_inner).0
                } else { self.changed.wait(waits).unwrap_or_else(std::sync::PoisonError::into_inner) };
            }
        }
    }
    pub(crate) fn submit(op: OpId, port: Arc<WorkPort>, condition: &WaitCondition, expected: u64, deadline: Option<Instant>) -> Result<()> {
        let s = get()?;
        let mut waits = s.waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if condition.load() != expected {
            port.complete(WorkResult { op, result: Ok(WorkOutput::ExternalWait(WaitResult::NotEqual)) });
            return Ok(());
        }
        let slot = waits.iter_mut().find(|w| w.is_none()).ok_or(Error::new(ErrorKind::ResourceLimit))?;
        *slot = Some(Wait { op, port, condition: condition.clone(), generation: condition.generation.load(Ordering::Acquire), expected, deadline });
        s.changed.notify_one(); Ok(())
    }
    pub(crate) fn cancel(op: OpId) {
        if let Ok(s) = get() {
            let mut waits = s.waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(slot) = waits.iter_mut().find(|slot| slot.as_ref().is_some_and(|w| w.op == op)) {
                let w = slot.take().expect("cancelled wait");
                w.port.complete(WorkResult { op, result: Err(Error::new(ErrorKind::Cancelled)) });
                s.changed.notify_one();
            }
        }
    }
    pub(crate) fn close(owner: u64) {
        if let Some(Ok(s)) = SERVICE.get() {
            let mut waits = s.waits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for slot in waits.iter_mut() { if slot.as_ref().is_some_and(|w| w.op.owner() == owner) { *slot = None; } }
            s.changed.notify_one();
        }
    }
}
#[cfg(target_arch = "wasm32")]
mod service {
    use super::*;
    pub(super) fn initialize() -> Result<()> { Err(Error::new(ErrorKind::Unsupported)) }
    pub(super) fn notify(condition: &WaitCondition) { condition.generation.fetch_add(1, Ordering::Release); }
    pub(crate) fn submit(_: crate::OpId, _: Arc<crate::blocking::WorkPort>, _: &WaitCondition, _: u64, _: Option<Instant>) -> Result<()> { Err(Error::new(ErrorKind::Unsupported)) }
    pub(crate) fn cancel(_: crate::OpId) {}
    pub(crate) fn close(_: u64) {}
}
pub(crate) use service::{submit, cancel, close};
