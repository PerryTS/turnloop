//! Per-loop child and signal registrations. All completion storage is reserved.
use super::{poller::{Poller, last_error}, signals::{self, Subscription}, unix::Detached};
use crate::{backend::{Event, Operation, Outcome, Request}, *};
use std::{os::{fd::{AsRawFd, OwnedFd}, unix::process::ExitStatusExt}, process::Child};

struct ChildState {
    child: Child,
    status: Option<ExitStatus>,
    group: bool,
    pidfd: Option<OwnedFd>,
    fallback: Option<Subscription>,
    ready: bool,
}
impl ChildState {
    fn reap(&mut self) -> Result<Option<ExitStatus>> {
        if self.status.is_none() {
            if let Some(status) = self.child.try_wait().map_err(Error::from)? {
                self.status = Some(ExitStatus { code: status.code(), signal: status.signal() });
            }
        }
        Ok(self.status)
    }
    fn kill(&mut self, signal: Signal, group: bool) -> Result<()> {
        if self.status.is_some() { return Err(Error::new(ErrorKind::NotFound)); }
        if group && !self.group { return Err(Error::new(ErrorKind::InvalidInput)); }
        let pid = self.child.id() as i32;
        let signal = signals::number(signal)?;
        // SAFETY: child remains unreaped, so its positive PID cannot be reused;
        // negative PID is only permitted for the group we created for this child.
        if unsafe { libc::kill(if group { -pid } else { pid }, signal) } < 0 { return Err(last_error()); }
        Ok(())
    }
}
impl Drop for ChildState {
    fn drop(&mut self) {
        if self.status.is_none() {
            let _ = self.kill(Signal::Kill, self.group);
            // Child::wait retries EINTR and reaps only this owned child, never others.
            let _ = self.child.wait();
        }
    }
}
enum Kind {
    Child(ChildState),
    Signal(Signal, Subscription),
}
struct Entry { handle: Handle, kind: Kind, op: Option<OpId>, cancelled: bool }
pub(super) struct Services { entries: Vec<Option<Entry>>, notifier: Option<Notifier> }
impl Services {
    pub fn new(capacity: usize) -> Self { Self { entries: (0..capacity).map(|_| None).collect(), notifier: None } }
    pub fn set_notifier(&mut self, notifier: Notifier) { self.notifier = Some(notifier); }
    fn notifier(&self) -> Result<Notifier> { self.notifier.clone().ok_or(Error::new(ErrorKind::InvalidInput)) }
    pub fn contains(&self, h: Handle) -> bool { self.entries.get(h.index()).and_then(Option::as_ref).is_some_and(|e| e.handle == h) }
    pub fn signal(&mut self, h: Handle, signal: Signal) -> Result<()> {
        let ticket = signals::subscribe(signal, self.notifier()?)?;
        self.entries[h.index()] = Some(Entry { handle: h, kind: Kind::Signal(signal, ticket), op: None, cancelled: false });
        Ok(())
    }
    pub fn child<P: Poller>(&mut self, h: Handle, child: Child, group: bool, poller: &mut P) -> Result<()> {
        let mut state = ChildState { child, group, status: None, pidfd: None, fallback: None, ready: true };
        // NOTE_EXIT can precede waitpid visibility on a heavily loaded kqueue.
        // A shared SIGCHLD subscription supplies a later reaping opportunity,
        // without polling or a thread per child. Subscribe before the first check.
        #[cfg(turnloop_backend = "kqueue")]
        { state.fallback = Some(signals::subscribe(Signal::Chld, self.notifier()?)?); }
        match poller.process(state.child.id(), h.key()) {
            Ok(fd) => state.pidfd = fd,
            Err(e) => {
                // A child can exit between spawn and kqueue registration. Reap
                // immediately before interpreting registration failure.
                if state.reap()?.is_none() {
                    #[cfg(turnloop_backend = "epoll")]
                    if matches!(e.os, Some(libc::ENOSYS | libc::EINVAL | libc::EPERM)) {
                        state.fallback = Some(signals::subscribe(Signal::Chld, self.notifier()?)?);
                        // Subscription-before-second-check closes the SIGCHLD race.
                        state.reap()?;
                    } else { return Err(e); }
                    #[cfg(turnloop_backend = "kqueue")]
                    return Err(e);
                }
            }
        }
        self.entries[h.index()] = Some(Entry { handle: h, kind: Kind::Child(state), op: None, cancelled: false });
        Ok(())
    }
    pub fn submit(&mut self, request: &Request) -> Result<()> {
        let e = self.entries.get_mut(request.handle.index()).and_then(Option::as_mut).filter(|e| e.handle == request.handle).ok_or(Error::new(ErrorKind::NotFound))?;
        if e.op.is_some() || !matches!((&e.kind, &request.operation), (Kind::Child(_), Operation::ProcessExit) | (Kind::Signal(..), Operation::WatchSignal)) { return Err(Error::new(ErrorKind::InvalidInput)); }
        e.op = Some(request.op); e.cancelled = false; Ok(())
    }
    pub fn cancel(&mut self, op: OpId) -> bool {
        for e in self.entries.iter_mut().flatten() {
            if e.op == Some(op) { e.cancelled = true; return true; }
        }
        false
    }
    pub fn ready(&mut self, key: u64) -> bool {
        if let Some(e) = self.entries.get_mut(key as u32 as usize).and_then(Option::as_mut).filter(|e| e.handle.key() == key) {
            if let Kind::Child(state) = &mut e.kind { state.ready = true; }
            return true;
        }
        false
    }
    fn runnable(e: &Entry) -> bool {
        e.op.is_some() && (e.cancelled || match &e.kind {
            Kind::Child(c) => c.ready || c.status.is_some() || c.fallback.as_ref().is_some_and(|s| s.ready()),
            Kind::Signal(_, ticket) => ticket.ready(),
        })
    }
    pub fn has_work(&self) -> bool { self.entries.iter().flatten().any(Self::runnable) }
    pub fn poll(&mut self, events: &mut Vec<Event<Detached>>) {
        for e in self.entries.iter_mut().flatten() {
            if events.len() == events.capacity() { break; }
            if !Self::runnable(e) { continue; }
            let (result, terminal) = if e.cancelled { (Ok(Outcome::Cancelled), true) } else { match &mut e.kind {
                Kind::Signal(signal, ticket) => { if !ticket.take() { continue; } (Ok(Outcome::Signal(*signal)), false) }
                Kind::Child(c) => {
                    c.ready = false;
                    if let Some(ticket) = &c.fallback { ticket.take(); }
                    match c.reap() { Ok(Some(status)) => (Ok(Outcome::Exited(status)), true), Ok(None) => continue, Err(e) => (Err(e), true) }
                }
            }};
            let op = e.op.expect("runnable operation");
            if terminal { e.op = None; }
            events.push(Event { op, terminal, result });
        }
    }
    pub fn kill(&mut self, h: Handle, signal: Signal, group: bool) -> Result<()> {
        let e = self.entries.get_mut(h.index()).and_then(Option::as_mut).filter(|e| e.handle == h).ok_or(Error::new(ErrorKind::NotFound))?;
        if let Kind::Child(c) = &mut e.kind { c.kill(signal, group) } else { Err(Error::new(ErrorKind::InvalidInput)) }
    }
    pub fn release<P: Poller>(&mut self, h: Handle, poller: &mut P) {
        if self.contains(h) {
            if let Some(Entry { kind: Kind::Child(c), .. }) = self.entries[h.index()].as_ref() {
                poller.remove_process(c.child.id(), c.pidfd.as_ref().map(AsRawFd::as_raw_fd));
            }
            self.entries[h.index()] = None;
        }
    }
}
