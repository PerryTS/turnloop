//! Optional local futures executor. The host calls [`LocalExecutor::turn`]; user
//! futures run only after the driver's completion collection has returned.
//!
//! `futures-io` is the only extra dependency. Tasks allocate once when spawned;
//! polls and warmed I/O/timer operations use fixed tables and retained buffers.
//! Borrowed futures-io buffers are copied into/from executor-owned staging memory,
//! so Pending never extends a caller buffer's lifetime. Writes are buffered;
//! flush or close the adapter to confirm delivery to the underlying transport.
//!
//! ```
//! # #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd"))]
//! # fn main() -> turnloop::Result<()> {
//! use std::time::Duration;
//! use turnloop::{backend::Platform, Config, LocalExecutor, Timeout};
//! let mut executor = LocalExecutor::<Platform>::new(Config::default())?;
//! let handle = executor.handle();
//! let task = executor.spawn_local(async move {
//!     handle.sleep(Duration::from_millis(2)).await.expect("timer");
//!     42
//! })?;
//! while !task.is_finished() {
//!     executor.turn(Timeout::After(Duration::from_secs(1)))?;
//! }
//! // A host can poll the JoinHandle, or await it from another local task.
//! assert!(task.is_finished());
//! # Ok(()) }
//! # #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd")))]
//! # fn main() {}
//! ```
use crate::{backend::Backend, *};
use futures_io::{AsyncRead, AsyncWrite};
use std::{
    cell::{Cell, RefCell, RefMut},
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
const TAG: u64 = 1 << 63;

/// Fixed executor capacities, allocated at construction.
#[derive(Clone, Copy, Debug)]
pub struct ExecutorConfig {
    /// Simultaneously live local tasks.
    pub tasks: usize,
    /// Simultaneously pending futures and I/O operations.
    pub operations: usize,
    /// Retained staging bytes per operation; writes can return partial progress.
    pub buffer_size: usize,
}
impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            tasks: 1024,
            operations: 1024,
            buffer_size: 16 * 1024,
        }
    }
}
struct IoSlot {
    generation: u32,
    used: bool,
    abandoned: bool,
    op: Option<OpId>,
    timer: Option<Handle>,
    result: Option<OpResult>,
    waker: Option<Waker>,
    bytes: Box<[u8]>,
    offset: usize,
}
#[derive(Clone, Copy)]
struct Key {
    index: usize,
    generation: u32,
}
impl Key {
    fn token(self) -> Token {
        Token(TAG | ((self.generation as u64) << 32) | self.index as u64)
    }
}
struct TaskWake {
    ready: AtomicBool,
    cancelled: AtomicBool,
    notifier: Notifier,
}
impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.store(true, Ordering::Release);
        let _ = self.notifier.notify();
    }
}
struct Task {
    future: Pin<Box<dyn Future<Output = ()>>>,
    wake: Arc<TaskWake>,
}
struct Shared<B: Backend> {
    // Driver must quiesce native I/O before the slot buffers are destroyed.
    driver: RefCell<Driver<B>>,
    slots: RefCell<Vec<IoSlot>>,
    tasks: RefCell<Vec<Option<Task>>>,
    occupied: RefCell<Vec<bool>>,
    closed: Cell<bool>,
}
impl<B: Backend> Shared<B> {
    fn reserve(&self, cx: &Context<'_>) -> Result<Key> {
        let mut slots = self.slots.borrow_mut();
        for (index, slot) in slots.iter_mut().enumerate() {
            if !slot.used && slot.generation < 0x7fff_ffff {
                slot.generation += 1;
                slot.used = true;
                slot.abandoned = false;
                slot.waker = Some(cx.waker().clone());
                slot.offset = 0;
                return Ok(Key {
                    index,
                    generation: slot.generation,
                });
            }
        }
        Err(Error::new(ErrorKind::ResourceLimit))
    }
    fn free(&self, key: Key) {
        let mut slots = self.slots.borrow_mut();
        let slot = &mut slots[key.index];
        debug_assert_eq!(slot.generation, key.generation);
        let result = slot.result.take();
        slot.used = false;
        slot.op = None;
        slot.waker = None;
        slot.timer = None;
        drop(slots);
        if let Some(
            OpResult::Accepted { conn, .. }
            | OpResult::PipeAccepted { conn }
            | OpResult::HandleReceived { handle: conn },
        ) = result
        {
            let _ = self.driver.borrow_mut().close(conn, Token(0));
        }
    }
    fn abandon(&self, key: Key) {
        let (op, timer, done) = {
            let mut slots = self.slots.borrow_mut();
            let slot = &mut slots[key.index];
            if !slot.used || slot.generation != key.generation {
                return;
            }
            slot.abandoned = true;
            slot.waker = None;
            (slot.op, slot.timer.take(), slot.result.is_some())
        };
        if let Some(timer) = timer {
            let _ = self.driver.borrow_mut().close(timer, Token(0));
        } else if let Some(op) = op {
            self.driver.borrow_mut().cancel(op);
        }
        if done || op.is_none() {
            self.free(key);
        }
    }
    fn result(&self, key: Key, cx: &Context<'_>) -> Option<OpResult> {
        let mut slots = self.slots.borrow_mut();
        let slot = &mut slots[key.index];
        if slot.waker.as_ref().is_none_or(|w| !w.will_wake(cx.waker())) {
            slot.waker = Some(cx.waker().clone());
        }
        slot.result.take()
    }
    fn dispatch(&self, completion: Completion) {
        if completion.token.0 & TAG == 0 {
            return;
        }
        let key = Key {
            index: completion.token.0 as u32 as usize,
            generation: ((completion.token.0 & !TAG) >> 32) as u32,
        };
        let (waker, abandoned, timer) = {
            let mut slots = self.slots.borrow_mut();
            let Some(slot) = slots.get_mut(key.index) else {
                return;
            };
            if !slot.used || slot.generation != key.generation {
                return;
            }
            let timer = slot.timer.take();
            slot.result = Some(completion.result);
            (slot.waker.take(), slot.abandoned, timer)
        };
        if let Some(timer) = timer {
            let _ = self.driver.borrow_mut().close(timer, Token(0));
        }
        if abandoned {
            self.free(key);
        } else if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// A `!Send` executor whose owning host explicitly drives every turn.
pub struct LocalExecutor<B: Backend> {
    shared: Rc<Shared<B>>,
    out: Completions,
}
impl<B: Backend> LocalExecutor<B> {
    /// Construct a loop and executor with default retained capacities.
    pub fn new(config: Config) -> Result<Self> {
        Self::with_config(config, ExecutorConfig::default())
    }
    /// Construct with explicit fixed capacities. No hidden loop-driving thread.
    pub fn with_config(config: Config, executor: ExecutorConfig) -> Result<Self> {
        if executor.tasks == 0
            || executor.operations == 0
            || executor.operations > u32::MAX as usize
            || executor.buffer_size == 0
        {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let driver = Driver::<B>::new(config)?;
        Ok(Self {
            shared: Rc::new(Shared {
                driver: RefCell::new(driver),
                slots: RefCell::new(
                    (0..executor.operations)
                        .map(|_| IoSlot {
                            generation: 0,
                            used: false,
                            abandoned: false,
                            op: None,
                            timer: None,
                            result: None,
                            waker: None,
                            bytes: vec![0; executor.buffer_size].into_boxed_slice(),
                            offset: 0,
                        })
                        .collect(),
                ),
                tasks: RefCell::new((0..executor.tasks).map(|_| None).collect()),
                occupied: RefCell::new(vec![false; executor.tasks]),
                closed: Cell::new(false),
            }),
            out: Completions::with_capacity(config.events_per_turn),
        })
    }
    /// Clone a local handle for constructing streams and futures inside tasks.
    pub fn handle(&self) -> ExecutorHandle<B> {
        ExecutorHandle {
            shared: self.shared.clone(),
        }
    }
    /// Borrow the loop for synchronous resource creation/configuration. Tokens
    /// with their top bit set are reserved; executor turn consumes completions.
    pub fn driver(&self) -> RefMut<'_, Driver<B>> {
        self.shared.driver.borrow_mut()
    }
    /// Spawn a local future. Dropping its JoinHandle requests cancellation.
    pub fn spawn_local<F: Future + 'static>(&self, future: F) -> Result<JoinHandle<F::Output>> {
        self.handle().spawn_local(future)
    }
    /// Poll each runnable task at most once. Returns the number actually polled.
    pub fn run_ready(&mut self) -> usize {
        let count = self.shared.tasks.borrow().len();
        let mut polls = 0;
        for i in 0..count {
            let task = {
                let mut tasks = self.shared.tasks.borrow_mut();
                if tasks[i]
                    .as_ref()
                    .is_none_or(|t| !t.wake.ready.swap(false, Ordering::AcqRel))
                {
                    continue;
                }
                tasks[i].take().expect("ready task")
            };
            let mut task = task;
            let done = if task.wake.cancelled.load(Ordering::Acquire) {
                true
            } else {
                let waker = Waker::from(task.wake.clone());
                polls += 1;
                task.future
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_ready()
            };
            if done {
                self.shared.occupied.borrow_mut()[i] = false;
                drop(task);
            } else {
                self.shared.tasks.borrow_mut()[i] = Some(task);
            }
        }
        polls
    }
    /// Drive one bounded loop turn and then one pass of runnable futures. Pending
    /// tasks that woke themselves keep this turn nonblocking, without being repolled
    /// repeatedly inside the same call.
    pub fn turn(&mut self, timeout: crate::Timeout) -> Result<TurnInfo> {
        self.run_ready();
        let ready = self
            .shared
            .tasks
            .borrow()
            .iter()
            .flatten()
            .any(|t| t.wake.ready.load(Ordering::Acquire));
        let idle = !self.alive();
        let info = self.shared.driver.borrow_mut().turn(
            if ready || idle {
                crate::Timeout::Now
            } else {
                timeout
            },
            &mut self.out,
        )?;
        for completion in self.out.drain() {
            self.shared.dispatch(completion);
        }
        self.run_ready();
        Ok(info)
    }
    /// Whether any task or referenced loop work remains.
    pub fn alive(&self) -> bool {
        self.shared.occupied.borrow().iter().any(|&v| v) || self.shared.driver.borrow().alive()
    }
}
impl<B: Backend> Drop for LocalExecutor<B> {
    fn drop(&mut self) {
        self.shared.closed.set(true);
        // Drop futures outside table borrows: their destructors cancel owned I/O.
        let count = self.shared.tasks.borrow().len();
        for i in 0..count {
            let task = self.shared.tasks.borrow_mut()[i].take();
            drop(task);
        }
    }
}
/// Cloneable local access to executor resource wrappers and task spawning.
pub struct ExecutorHandle<B: Backend> {
    shared: Rc<Shared<B>>,
}
impl<B: Backend> Clone for ExecutorHandle<B> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}
impl<B: Backend> ExecutorHandle<B> {
    /// Adopt a loop-owned TCP/local/stdio stream. Drop cancels I/O and closes it.
    pub fn io(&self, handle: Handle) -> AsyncIo<B> {
        AsyncIo {
            shared: self.shared.clone(),
            handle,
            peer: None,
            read: None,
            write: None,
            closed: false,
        }
    }
    /// Adopt a UDP socket. AsyncWrite sends one datagram to `peer`; AsyncRead
    /// receives one datagram and discards its source address. Large writes are
    /// rejected rather than split into several datagrams.
    pub fn udp(&self, handle: Handle, peer: SocketAddr) -> AsyncIo<B> {
        let mut io = self.io(handle);
        io.peer = Some(peer);
        io
    }
    /// Sleep until an absolute deadline in the backend clock domain.
    pub fn sleep_until(&self, deadline: Instant) -> Sleep<B> {
        Sleep {
            shared: self.shared.clone(),
            deadline,
            key: None,
            done: false,
        }
    }
    /// Sleep for a duration starting now.
    pub fn sleep(&self, duration: Duration) -> Sleep<B> {
        self.sleep_until(self.shared.driver.borrow().now() + duration)
    }
    /// Fail with TimedOut if the future does not finish before the duration.
    pub fn timeout<F: Future>(&self, duration: Duration, future: F) -> Timeout<B, F> {
        Timeout {
            future,
            sleep: self.sleep(duration),
        }
    }
    /// Await an incoming TCP or local connection.
    pub fn accept(&self, listener: Handle) -> Accept<B> {
        Accept {
            shared: self.shared.clone(),
            listener,
            key: None,
        }
    }
    /// Spawn a !Send future with a cancellation-on-drop join handle.
    pub fn spawn_local<F: Future + 'static>(&self, future: F) -> Result<JoinHandle<F::Output>> {
        if self.shared.closed.get() {
            return Err(Error::new(ErrorKind::NotFound));
        }
        let index = self
            .shared
            .occupied
            .borrow()
            .iter()
            .position(|&v| !v)
            .ok_or(Error::new(ErrorKind::ResourceLimit))?;
        let wake = Arc::new(TaskWake {
            ready: AtomicBool::new(true),
            cancelled: AtomicBool::new(false),
            notifier: self.shared.driver.borrow().notifier(),
        });
        let result = Rc::new(RefCell::new(JoinState {
            value: None,
            waker: None,
            finished: false,
            cancelled: false,
        }));
        let output = JoinGuard(result.clone());
        let wrapped = async move {
            let value = future.await;
            let waker = {
                let mut result = output.0.borrow_mut();
                result.value = Some(value);
                result.finished = true;
                result.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        };
        self.shared.occupied.borrow_mut()[index] = true;
        self.shared.tasks.borrow_mut()[index] = Some(Task {
            future: Box::pin(wrapped),
            wake: wake.clone(),
        });
        Ok(JoinHandle { result, wake })
    }
}
struct JoinState<T> {
    value: Option<T>,
    waker: Option<Waker>,
    finished: bool,
    cancelled: bool,
}
struct JoinGuard<T>(Rc<RefCell<JoinState<T>>>);
impl<T> Drop for JoinGuard<T> {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.0.borrow_mut();
            if state.finished {
                return;
            }
            state.cancelled = true;
            state.finished = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}
/// A task ended before producing its result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JoinError {
    /// The join handle requested cancellation, or its executor was dropped.
    Cancelled,
}
/// Future yielding a task's result. Dropping it cancels the task on the next pass;
/// that pass drops the task's futures and cancels their pending I/O.
pub struct JoinHandle<T> {
    result: Rc<RefCell<JoinState<T>>>,
    wake: Arc<TaskWake>,
}
impl<T> JoinHandle<T> {
    /// Whether the task completed and stored its result.
    pub fn is_finished(&self) -> bool {
        self.result.borrow().finished
    }
    /// Request cancellation and wake the executor.
    pub fn cancel(&self) {
        self.wake.cancelled.store(true, Ordering::Release);
        self.wake.wake_by_ref();
    }
}
impl<T> Future for JoinHandle<T> {
    type Output = std::result::Result<T, JoinError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.result.borrow_mut();
        if let Some(value) = state.value.take() {
            Poll::Ready(Ok(value))
        } else if state.cancelled {
            Poll::Ready(Err(JoinError::Cancelled))
        } else {
            if state
                .waker
                .as_ref()
                .is_none_or(|w| !w.will_wake(cx.waker()))
            {
                state.waker = Some(cx.waker().clone());
            }
            Poll::Pending
        }
    }
}
impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if !self.result.borrow().finished {
            self.cancel();
        }
    }
}

/// TCP, UDP, local-pipe or stdio adapter with stable owned staging buffers.
///
/// Writes are buffered: `poll_write` reports bytes accepted into owned storage;
/// `poll_flush`, `poll_close`, or the next write waits for their native completion
/// and reports delayed errors. Flush before dropping to preserve buffered output.
/// Dropping cancels pending reads and writes and closes the owned loop handle.
/// A Pending poll never consumes the current write slice, so it may be replaced.
/// UDP writes accept one complete datagram or reject it if staging is too small;
/// UDP reads expose payload bytes without the sender address.
pub struct AsyncIo<B: Backend> {
    shared: Rc<Shared<B>>,
    handle: Handle,
    peer: Option<SocketAddr>,
    read: Option<Key>,
    write: Option<Key>,
    closed: bool,
}
impl<B: Backend> Unpin for AsyncIo<B> {}
impl<B: Backend> AsyncIo<B> {
    /// Borrow the underlying loop handle for configuration or address queries.
    pub fn handle(&self) -> Handle {
        self.handle
    }
    fn start_read(&mut self, cx: &Context<'_>, length: usize) -> Result<Key> {
        let key = self.shared.reserve(cx)?;
        let buf = {
            let mut slots = self.shared.slots.borrow_mut();
            let bytes = &mut slots[key.index].bytes;
            let len = length.min(bytes.len());
            // SAFETY: preallocated slot bytes remain alive until terminal native
            // acknowledgement, including when this adapter is dropped while Pending.
            ReadBuf::Provided(unsafe { IoBufMut::from_raw_parts(bytes.as_mut_ptr(), len) })
        };
        let result = if self.peer.is_some() {
            self.shared
                .driver
                .borrow_mut()
                .recv(self.handle, buf, key.token())
        } else {
            self.shared
                .driver
                .borrow_mut()
                .read(self.handle, buf, key.token())
        };
        match result {
            Ok(op) => {
                self.shared.slots.borrow_mut()[key.index].op = Some(op);
                Ok(key)
            }
            Err(e) => {
                self.shared.free(key);
                Err(e)
            }
        }
    }
}
fn io_error(error: Error) -> io::Error {
    if let Some(os) = error.os {
        io::Error::from_raw_os_error(os)
    } else {
        io::Error::other(error)
    }
}
impl<B: Backend> AsyncRead for AsyncIo<B> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let key = match this.read {
            Some(key) => key,
            None => match this.start_read(cx, buf.len()) {
                Ok(key) => {
                    this.read = Some(key);
                    key
                }
                Err(e) => return Poll::Ready(Err(io_error(e))),
            },
        };
        let Some(result) = this.shared.result(key, cx) else {
            return Poll::Pending;
        };
        match result {
            OpResult::Read { n, .. } | OpResult::RecvFrom { n, .. } => {
                let remaining = {
                    let mut slots = this.shared.slots.borrow_mut();
                    let slot = &mut slots[key.index];
                    let take = buf.len().min(n - slot.offset);
                    buf[..take].copy_from_slice(&slot.bytes[slot.offset..slot.offset + take]);
                    slot.offset += take;
                    if slot.offset < n {
                        slot.result = Some(OpResult::Read { n, lease: None });
                    }
                    (take, slot.offset == n)
                };
                if remaining.1 {
                    this.shared.free(key);
                    this.read = None;
                }
                Poll::Ready(Ok(remaining.0))
            }
            other => {
                this.shared.free(key);
                this.read = None;
                Poll::Ready(match other {
                    OpResult::Eof => Ok(0),
                    OpResult::Err(e) => Err(io_error(e)),
                    OpResult::Cancelled => Err(io_error(Error::new(ErrorKind::Cancelled))),
                    _ => Err(io::Error::other("unexpected read completion")),
                })
            }
        }
    }
}
impl<B: Backend> AsyncWrite for AsyncIo<B> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.closed {
            return Poll::Ready(Err(io_error(Error::new(ErrorKind::BrokenPipe))));
        }
        // A Pending call must not consume the current caller's bytes. Drain the
        // previously accepted buffer before accepting this one, even if the
        // caller changed its slice after a Pending poll_write.
        match self.as_mut().poll_flush(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let this = self.get_mut();
        let key = match this.shared.reserve(cx) {
            Ok(key) => key,
            Err(e) => return Poll::Ready(Err(io_error(e))),
        };
        let (ptr, n, too_big) = {
            let mut slots = this.shared.slots.borrow_mut();
            let bytes = &mut slots[key.index].bytes;
            let n = buf.len().min(bytes.len());
            bytes[..n].copy_from_slice(&buf[..n]);
            (bytes.as_ptr(), n, n < buf.len() && this.peer.is_some())
        };
        if too_big {
            this.shared.free(key);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "datagram exceeds executor buffer",
            )));
        }
        // SAFETY: staging bytes are immutable from submit until terminal
        // acknowledgement; dropping the adapter abandons but retains the slot.
        let buf = WriteBuf::Provided(unsafe { IoBuf::from_raw_parts(ptr, n) });
        let result = if let Some(peer) = this.peer {
            this.shared
                .driver
                .borrow_mut()
                .send_to(this.handle, buf, peer, key.token())
        } else {
            this.shared
                .driver
                .borrow_mut()
                .write(this.handle, buf, key.token())
        };
        match result {
            Ok(op) => {
                this.shared.slots.borrow_mut()[key.index].op = Some(op);
                this.write = Some(key);
                // The adapter now owns these bytes. Native errors are surfaced
                // by flush, close, or the next write, as for a buffered writer.
                Poll::Ready(Ok(n))
            }
            Err(e) => {
                this.shared.free(key);
                Poll::Ready(Err(io_error(e)))
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let Some(key) = this.write else {
            return Poll::Ready(Ok(()));
        };
        let Some(result) = this.shared.result(key, cx) else {
            return Poll::Pending;
        };
        this.shared.free(key);
        this.write = None;
        Poll::Ready(match result {
            OpResult::Wrote(_) => Ok(()),
            OpResult::Err(e) => Err(io_error(e)),
            _ => Err(io::Error::other("write cancelled")),
        })
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.as_mut().poll_flush(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        let this = self.get_mut();
        if !this.closed {
            this.shared
                .driver
                .borrow_mut()
                .close(this.handle, Token(0))
                .map_err(io_error)?;
            this.closed = true;
        }
        Poll::Ready(Ok(()))
    }
}
impl<B: Backend> Drop for AsyncIo<B> {
    fn drop(&mut self) {
        if let Some(key) = self.read.take() {
            self.shared.abandon(key);
        }
        if let Some(key) = self.write.take() {
            self.shared.abandon(key);
        }
        if !self.closed {
            let _ = self.shared.driver.borrow_mut().close(self.handle, Token(0));
        }
    }
}

/// A cancellable timer future using the driver's exact monotonic deadline.
pub struct Sleep<B: Backend> {
    shared: Rc<Shared<B>>,
    deadline: Instant,
    key: Option<Key>,
    done: bool,
}
impl<B: Backend> Unpin for Sleep<B> {}
impl<B: Backend> Future for Sleep<B> {
    type Output = Result<()>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(Ok(()));
        }
        let key = match this.key {
            Some(key) => key,
            None => {
                let key = this.shared.reserve(cx)?;
                let timer = this
                    .shared
                    .driver
                    .borrow_mut()
                    .timer(this.deadline, None, key.token());
                match timer {
                    Ok(timer) => {
                        let op = this.shared.driver.borrow().timer_op(timer);
                        let mut slots = this.shared.slots.borrow_mut();
                        slots[key.index].op = op;
                        slots[key.index].timer = Some(timer);
                        this.key = Some(key);
                        key
                    }
                    Err(e) => {
                        this.shared.free(key);
                        return Poll::Ready(Err(e));
                    }
                }
            }
        };
        let Some(result) = this.shared.result(key, cx) else {
            return Poll::Pending;
        };
        this.shared.free(key);
        this.key = None;
        this.done = true;
        Poll::Ready(match result {
            OpResult::Timer => Ok(()),
            OpResult::Err(e) => Err(e),
            _ => Err(Error::new(ErrorKind::Cancelled)),
        })
    }
}
impl<B: Backend> Drop for Sleep<B> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.shared.abandon(key);
        }
    }
}
/// Deadline wrapper. The inner future is dropped when this wrapper is dropped.
pub struct Timeout<B: Backend, F> {
    future: F,
    sleep: Sleep<B>,
}
impl<B: Backend, F: Future> Future for Timeout<B, F> {
    type Output = Result<F::Output>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: neither field is moved out; both projections remain pinned as
        // long as the wrapper. There is no custom Drop that could move the future.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: structural pin projection; future is never moved after pinning.
        if let Poll::Ready(value) = unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx) {
            return Poll::Ready(Ok(value));
        }
        match Pin::new(&mut this.sleep).poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Err(Error::new(ErrorKind::TimedOut))),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}
/// Cancellable accept future returning a stream adapter for either native transport.
pub struct Accept<B: Backend> {
    shared: Rc<Shared<B>>,
    listener: Handle,
    key: Option<Key>,
}
impl<B: Backend> Unpin for Accept<B> {}
impl<B: Backend> Future for Accept<B> {
    type Output = Result<AsyncIo<B>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let key = match this.key {
            Some(key) => key,
            None => {
                let key = this.shared.reserve(cx)?;
                let result = this
                    .shared
                    .driver
                    .borrow_mut()
                    .accept(this.listener, key.token());
                match result {
                    Ok(op) => {
                        this.shared.slots.borrow_mut()[key.index].op = Some(op);
                        this.key = Some(key);
                        key
                    }
                    Err(e) => {
                        this.shared.free(key);
                        return Poll::Ready(Err(e));
                    }
                }
            }
        };
        let Some(result) = this.shared.result(key, cx) else {
            return Poll::Pending;
        };
        this.shared.free(key);
        this.key = None;
        Poll::Ready(match result {
            OpResult::Accepted { conn, .. } | OpResult::PipeAccepted { conn } => {
                Ok(ExecutorHandle {
                    shared: this.shared.clone(),
                }
                .io(conn))
            }
            OpResult::Err(e) => Err(e),
            _ => Err(Error::new(ErrorKind::Cancelled)),
        })
    }
}
impl<B: Backend> Drop for Accept<B> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.shared.abandon(key);
        }
    }
}
