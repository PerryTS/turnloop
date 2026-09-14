//! Shared executor scheduling for sans-I/O connection pools. Policies stay in
//! protocol crates; this module owns sockets, waiter wakeups and absolute timers.
use crate::{Backend, ExecutorHandle, Instant, error};
use std::{cell::RefCell, future::{Future, poll_fn}, io, ops::{Deref,DerefMut}, pin::Pin, rc::Rc, task::{Context, Poll, Waker}, time::Duration};
use turnloop::{Handle, JoinHandle, Sleep};

pub enum Action<I,L> { Connect(I), Close(I), Remove(I), Acquired { token: u64, lease: L }, Failed { token: u64, error: io::Error }, Ended }
pub trait Policy: 'static {
    type Id: Copy + Eq;
    type Lease: Copy;
    fn id(lease: Self::Lease) -> Self::Id;
    fn checkout(&mut self, token: u64, now: Instant, deadline: Instant) -> io::Result<()>;
    fn cancel(&mut self, token: u64, now: Instant);
    fn connected(&mut self, id: Self::Id, now: Instant) -> io::Result<()>;
    fn connect_failed(&mut self, id: Self::Id, now: Instant) -> io::Result<()>;
    fn checkin(&mut self, lease: Self::Lease, now: Instant, destroy: bool) -> io::Result<()>;
    fn closed(&mut self, id: Self::Id, now: Instant) -> io::Result<()>;
    fn event(&mut self) -> Option<Action<Self::Id,Self::Lease>>;
    fn next_deadline(&self) -> Option<Instant>;
    fn expire(&mut self, now: Instant);
    fn end(&mut self) -> io::Result<()>;
}
pub trait Connection: 'static {
    fn reusable(&self) -> bool;
    fn handle(&self) -> Option<Handle>;
}
pub trait Connector<B: Backend>: 'static {
    type Connection: Connection;
    fn connect(&self, executor: &ExecutorHandle<B>, deadline: Instant) -> impl Future<Output=io::Result<Self::Connection>> + 'static;
}
type Connecting<C> = Pin<Box<dyn Future<Output=io::Result<C>>>>;
struct Entry<I,C> { id: I, client: Option<C>, connecting: Option<Connecting<C>>, handle: Option<Handle> }
struct Waiter<L> { token: u64, result: Option<io::Result<L>>, waker: Option<Waker> }
struct State<P: Policy,C> {
    policy: P,
    entries: Vec<Entry<P::Id,C>>,
    waiters: Vec<Waiter<P::Lease>>,
    token: u64,
    changed: Option<Waker>,
    ended: bool,
    end_wakers: Vec<Waker>,
    failure: Option<String>,
}
struct Owner<B: Backend,P: Policy,C> {
    state: Rc<RefCell<State<P,C>>>,
    executor: ExecutorHandle<B>,
    _task: JoinHandle<()>,
}
/// Cloneable pool. Checkout and release reuse retained slots after warm-up.
pub struct Pool<B: Backend,P: Policy,C> { owner: Rc<Owner<B,P,C>> }
impl<B: Backend,P: Policy,C> Clone for Pool<B,P,C> { fn clone(&self) -> Self { Self { owner: self.owner.clone() } } }
fn wake(waker: &Option<Waker>) { if let Some(w) = waker { w.wake_by_ref(); } }
impl<B: Backend + 'static,P: Policy,C: Connection> Pool<B,P,C> {
    pub fn new<F: Connector<B,Connection=C>>(executor: &ExecutorHandle<B>, policy: P, connector: F, connect_timeout: Duration, capacity: usize) -> io::Result<Self> {
        let state = Rc::new(RefCell::new(State { policy, entries: Vec::with_capacity(capacity), waiters: Vec::with_capacity(capacity), token: 0, changed: None, ended: false, end_wakers: Vec::new(), failure: None }));
        let shared = state.clone();
        let exec = executor.clone();
        let mut timer: Option<(Instant,Sleep<B>)> = None;
        let task = executor.spawn_local(poll_fn(move |cx| {
            let mut state = shared.borrow_mut();
            state.changed = Some(cx.waker().clone());
            let result = drive(&mut state, &exec, &connector, connect_timeout, &mut timer, cx);
            if let Err(e) = result {
                state.failure = Some(e.to_string());
                for waiter in &state.waiters { wake(&waiter.waker); }
                for w in &state.end_wakers { w.wake_by_ref(); }
                // Physical sockets close even if their client is currently leased.
                for entry in &state.entries { if let Some(h) = entry.handle { let _ = exec.driver().close(h, turnloop::Token(0)); } }
                state.entries.clear();
                return Poll::Ready(());
            }
            if state.ended { Poll::Ready(()) } else { Poll::Pending }
        })).map_err(error)?;
        Ok(Self { owner: Rc::new(Owner { state, executor: executor.clone(), _task: task }) })
    }
    pub fn total(&self) -> usize { self.owner.state.borrow().entries.len() }
    /// Apply a topology-driven policy change and wake the pool's scheduler.
    pub fn update_policy(&self, update: impl FnOnce(&mut P)) {
        let mut state = self.owner.state.borrow_mut();
        update(&mut state.policy);
        wake(&state.changed);
    }
    pub async fn acquire(&self, at: Instant) -> io::Result<Lease<B,P,C>> {
        let token = {
            let mut state = self.owner.state.borrow_mut();
            if let Some(e) = &state.failure { return Err(io::Error::other(e.clone())); }
            state.token = state.token.checked_add(1).ok_or_else(|| io::Error::other("pool token exhausted"))?;
            let token = state.token;
            state.policy.checkout(token, self.owner.executor.now(), at)?;
            state.waiters.push(Waiter { token, result: None, waker: None });
            wake(&state.changed);
            token
        };
        let mut pending = Pending { owner: &self.owner, token, done: false };
        let result = crate::deadline(&self.owner.executor, at, poll_fn(|cx| {
            let mut state = self.owner.state.borrow_mut();
            if let Some(e) = &state.failure { return Poll::Ready(Err(io::Error::other(e.clone()))); }
            let Some(index) = state.waiters.iter().position(|w| w.token == token) else { return Poll::Ready(Err(io::Error::other("missing pool waiter"))); };
            if let Some(result) = state.waiters[index].result.take() {
                state.waiters.swap_remove(index);
                return Poll::Ready(result.and_then(|lease| {
                    let entry = state.entries.iter_mut().find(|e| e.id == P::id(lease)).ok_or_else(|| io::Error::other("missing pooled connection"))?;
                    let client = entry.client.take().ok_or_else(|| io::Error::other("connection already leased"))?;
                    Ok(Lease { owner: self.owner.clone(), client: Some(client), lease })
                }));
            }
            state.waiters[index].waker = Some(cx.waker().clone());
            Poll::Pending
        })).await;
        pending.done = result.is_ok();
        result
    }
    pub async fn end(&self) -> io::Result<()> {
        {
            let mut state = self.owner.state.borrow_mut();
            state.policy.end()?;
            wake(&state.changed);
        }
        poll_fn(|cx| {
            let mut state = self.owner.state.borrow_mut();
            if let Some(e) = &state.failure { return Poll::Ready(Err(io::Error::other(e.clone()))); }
            if state.ended { return Poll::Ready(Ok(())); }
            if !state.end_wakers.iter().any(|w| w.will_wake(cx.waker())) { state.end_wakers.push(cx.waker().clone()); }
            Poll::Pending
        }).await
    }
}
fn drive<B: Backend,P: Policy,C: Connection,F: Connector<B,Connection=C>>(state: &mut State<P,C>, executor: &ExecutorHandle<B>, connector: &F, timeout: Duration, timer: &mut Option<(Instant,Sleep<B>)>, cx: &mut Context<'_>) -> io::Result<()> {
    let now = executor.now();
    state.policy.expire(now);
    loop {
        while let Some(action) = state.policy.event() {
            match action {
                Action::Connect(id) => state.entries.push(Entry { id, client: None, connecting: Some(Box::pin(connector.connect(executor, now + timeout))), handle: None }),
                Action::Close(id) => {
                    if let Some(i) = state.entries.iter().position(|e| e.id == id) {
                        let entry = state.entries.swap_remove(i);
                        if let Some(h) = entry.handle { let _ = executor.driver().close(h, turnloop::Token(0)); }
                        drop(entry);
                    }
                    state.policy.closed(id, now)?;
                }
                Action::Remove(id) => { if let Some(i) = state.entries.iter().position(|e| e.id == id) { state.entries.swap_remove(i); } }
                Action::Acquired { token, lease } => {
                    if let Some(w) = state.waiters.iter_mut().find(|w| w.token == token) {
                        w.result = Some(Ok(lease)); wake(&w.waker);
                    } else { state.policy.checkin(lease, now, true)?; }
                }
                Action::Failed { token, error } => {
                    if let Some(w) = state.waiters.iter_mut().find(|w| w.token == token) { w.result = Some(Err(error)); wake(&w.waker); }
                }
                Action::Ended => { state.ended = true; for w in &state.end_wakers { w.wake_by_ref(); } }
            }
        }
        let mut progressed = false;
        for entry in &mut state.entries {
            if let Some(future) = &mut entry.connecting {
                if let Poll::Ready(result) = future.as_mut().poll(cx) {
                    entry.connecting = None;
                    progressed = true;
                    match result {
                        Ok(client) => { entry.handle = client.handle(); entry.client = Some(client); state.policy.connected(entry.id, now)?; }
                        Err(_) => state.policy.connect_failed(entry.id, now)?,
                    }
                }
            }
        }
        if !progressed { break; }
    }
    let at = state.policy.next_deadline();
    if timer.as_ref().map(|(at,_)| *at) != at { *timer = at.map(|at| (at, executor.sleep_until(at))); }
    if let Some((_,sleep)) = timer { if let Poll::Ready(result) = Pin::new(sleep).poll(cx) { result.map_err(error)?; *timer = None; cx.waker().wake_by_ref(); } }
    Ok(())
}
struct Pending<'a,B: Backend,P: Policy,C> { owner: &'a Rc<Owner<B,P,C>>, token: u64, done: bool }
impl<B: Backend,P: Policy,C> Drop for Pending<'_,B,P,C> {
    fn drop(&mut self) {
        if !self.done {
            let mut state = self.owner.state.borrow_mut();
            if let Some(i) = state.waiters.iter().position(|w| w.token == self.token) {
                if let Some(Ok(lease)) = state.waiters.swap_remove(i).result { let _ = state.policy.checkin(lease, self.owner.executor.now(), true); }
                else { state.policy.cancel(self.token, self.owner.executor.now()); }
            }
            wake(&state.changed);
        }
    }
}
/// Exclusive client ownership. A cancelled query makes reusable() false; release
/// then destroys the connection instead of handing wire state to another caller.
pub struct Lease<B: Backend,P: Policy,C: Connection> { owner: Rc<Owner<B,P,C>>, client: Option<C>, lease: P::Lease }
impl<B: Backend,P: Policy,C: Connection> Deref for Lease<B,P,C> { type Target = C; fn deref(&self) -> &C { self.client.as_ref().expect("owned lease") } }
impl<B: Backend,P: Policy,C: Connection> DerefMut for Lease<B,P,C> { fn deref_mut(&mut self) -> &mut C { self.client.as_mut().expect("owned lease") } }
impl<B: Backend,P: Policy,C: Connection> Drop for Lease<B,P,C> {
    fn drop(&mut self) {
        let mut state = self.owner.state.borrow_mut();
        if let Some(client) = self.client.take() {
            if let Some(entry) = state.entries.iter_mut().find(|e| e.id == P::id(self.lease)) {
                let destroy = !client.reusable();
                entry.client = Some(client);
                let _ = state.policy.checkin(self.lease, self.owner.executor.now(), destroy);
            }
        }
        wake(&state.changed);
    }
}
