//! Reusable regular-file jobs on the shared blocking pool; never file I/O in poll.
use super::{poller::last_error, unix::Detached};
use crate::{backend::{Event, Operation, Outcome, Request}, blocking::ReusableWork, queue::Queue, *};
use std::{os::fd::{AsRawFd, OwnedFd}, sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, Ordering}}};
struct Job { request: Request, fd: OwnedFd }
// SAFETY: accepted file requests contain only owned bytes/fds or host byte regions
// whose exclusive/immutable lifetime extends through completion or driver drop.
// No Rc, pool lease or thread-affine host object is moved to a worker.
unsafe impl Send for Job {}
enum FileOutcome { Read(usize), Wrote(usize), Eof, Cancelled }
struct FileEvent { op: OpId, result: Result<FileOutcome> }
struct Port {
    results: Queue<FileEvent>,
    notifier: Mutex<Option<Notifier>>,
    running: Mutex<usize>,
    quiescent: Condvar,
}
struct Slot { job: Mutex<Option<Job>>, cancel: AtomicBool, port: Arc<Port> }
impl ReusableWork for Slot {
    fn run(&self) {
        let job = self.job.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take().expect("submitted file job");
        let op = job.request.op;
        let result = if self.cancel.load(Ordering::Acquire) { Ok(FileOutcome::Cancelled) } else { execute(job) };
        // Kernel buffer access has ended before publication. Slot reuse is safe
        // after the terminal event is delivered, including cancellation races.
        assert!(self.port.results.push(FileEvent { op, result }).is_ok(), "reserved file completion credit");
        if let Some(notifier) = self.port.notifier.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() { let _ = notifier.notify(); }
        let mut running = self.port.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *running -= 1; self.port.quiescent.notify_all();
    }
}
pub(super) struct Files { slots: Vec<Arc<Slot>>, active: Vec<Option<OpId>>, port: Arc<Port>, config: PoolConfig, pool: BufferPool, leases: Vec<Option<BufLease>> }
impl Files {
    pub fn new(config: &Config, pool: BufferPool) -> Self {
        let port = Arc::new(Port { results: Queue::new(config.max_operations.max(2).next_power_of_two()), notifier: Mutex::new(None), running: Mutex::new(0), quiescent: Condvar::new() });
        Self { slots: (0..config.max_operations).map(|_| Arc::new(Slot { job: Mutex::new(None), cancel: AtomicBool::new(false), port: port.clone() })).collect(), active: vec![None; config.max_operations], port, config: config.blocking_pool, pool, leases: (0..config.max_operations).map(|_| None).collect() }
    }
    pub fn set_notifier(&mut self, notifier: Notifier) { *self.port.notifier.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notifier); }
    pub fn submit(&mut self, mut request: Request, fd: OwnedFd) -> Result<()> {
        if !matches!(request.operation, Operation::Read { multishot: false, .. } | Operation::Write(_) | Operation::Writev(_)) { return Err(Error::new(ErrorKind::Unsupported)); }
        let op = request.op;
        if let Operation::Read { buf: ReadBuf::Pooled, multishot: false } = request.operation {
            let mut lease = self.pool.acquire().ok_or(Error::new(ErrorKind::ResourceLimit))?;
            let bytes = lease.writable();
            // SAFETY: the loop retains this exclusive lease until the worker's
            // terminal acknowledgement, or until all jobs quiesce during Drop.
            request.operation = Operation::Read { buf: ReadBuf::Provided(unsafe { IoBufMut::from_raw_parts(bytes.as_mut_ptr(), bytes.len()) }), multishot: false };
            self.leases[op.index()] = Some(lease);
        }
        let slot = &self.slots[op.index()];
        slot.cancel.store(false, Ordering::Release);
        *slot.job.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Job { request, fd });
        *self.port.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        if let Err(e) = crate::blocking::reusable(self.config, slot.clone()) {
            slot.job.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
            *self.port.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner) -= 1;
            self.leases[op.index()] = None;
            return Err(e);
        }
        self.active[op.index()] = Some(op); Ok(())
    }
    pub fn cancel(&self, op: OpId) -> bool {
        if self.active.get(op.index()) != Some(&Some(op)) { return false; }
        self.slots[op.index()].cancel.store(true, Ordering::Release); true
    }
    pub fn has_work(&self) -> bool { !self.port.results.is_empty() }
    pub fn poll(&mut self, events: &mut Vec<Event<Detached>>) {
        while events.len() < events.capacity() {
            let Some(event) = self.port.results.pop() else { break; };
            self.active[event.op.index()] = None;
            let mut lease = self.leases[event.op.index()].take();
            let result = event.result.map(|r| match r {
                FileOutcome::Read(n) => { if let Some(b) = &mut lease { b.set_len(n); } Outcome::Read { n, lease } },
                FileOutcome::Wrote(n) => Outcome::Wrote(n),
                FileOutcome::Eof => Outcome::Eof,
                FileOutcome::Cancelled => Outcome::Cancelled,
            });
            events.push(Event { op: event.op, terminal: true, result });
        }
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        for op in self.active.iter().flatten() { self.slots[op.index()].cancel.store(true, Ordering::Release); }
        let mut running = self.port.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while *running != 0 { running = self.port.quiescent.wait(running).unwrap_or_else(std::sync::PoisonError::into_inner); }
    }
}
fn execute(mut job: Job) -> Result<FileOutcome> {
    let fd = job.fd.as_raw_fd();
    match &mut job.request.operation {
        Operation::Read { buf, .. } => {
            let ReadBuf::Provided(buf) = buf else { return Err(Error::new(ErrorKind::InvalidInput)); };
            let (ptr, len) = (buf.as_mut_ptr(), buf.len());
            let n = loop {
                // SAFETY: operation owns stable writable memory until this job ends.
                let n = unsafe { libc::read(fd, ptr.cast(), len) };
                if n >= 0 { break n as usize; }
                let e = last_error(); if e.os != Some(libc::EINTR) { return Err(e); }
            };
            if n == 0 { Ok(FileOutcome::Eof) } else { Ok(FileOutcome::Read(n)) }
        }
        Operation::Write(buf) => { write_all(fd, buf.as_slice()).map(FileOutcome::Wrote) }
        Operation::Writev(bufs) => {
            let mut n = 0; for buf in bufs.bufs.iter().flatten() { n += write_all(fd, buf.as_slice())?; }
            Ok(FileOutcome::Wrote(n))
        }
        _ => Err(Error::new(ErrorKind::Unsupported)),
    }
}
fn write_all(fd: i32, mut bytes: &[u8]) -> Result<usize> {
    let length = bytes.len();
    while !bytes.is_empty() {
        // SAFETY: synchronous regular-file write from initialized owned/provided bytes.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n < 0 { let e = last_error(); if e.os == Some(libc::EINTR) { continue; } return Err(e); }
        if n == 0 { return Err(Error::new(ErrorKind::BrokenPipe)); }
        bytes = &bytes[n as usize..];
    }
    Ok(length)
}
