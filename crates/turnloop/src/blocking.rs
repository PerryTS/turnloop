//! Lazily started process-wide bounded pool, with a completion port per loop.
use crate::{BlockingResult, Error, ErrorKind, Notifier, OpId, Payload, Result, queue::Queue};
use std::{
    net::{SocketAddr, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Configuration fixed by the first process-wide blocking-pool submission.
pub struct PoolConfig {
    /// Number of lazily started process-wide blocking workers.
    pub threads: usize,
    /// Maximum queued blocking jobs before ResourceLimit is returned.
    pub queue_capacity: usize,
}
impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            threads: 4,
            queue_capacity: 1024,
        }
    }
}
#[derive(Debug)]
/// Owned hostname and port for a pool-backed native address lookup.
pub struct DnsRequest {
    /// Hostname resolved by the native resolver.
    pub host: String,
    /// Port included in each resolved socket address.
    pub port: u16,
}
pub(crate) enum WorkOutput {
    ExternalWait(crate::WaitResult),
    Blocking(Payload),
    Resolved(Vec<SocketAddr>),
}
pub(crate) struct WorkResult {
    pub op: OpId,
    pub result: Result<WorkOutput>,
}
pub(crate) struct WorkPort {
    queue: Queue<WorkResult>,
    #[cfg(not(target_arch = "wasm32"))]
    notifier: Notifier,
    closed: AtomicBool,
}
impl WorkPort {
    pub fn new(capacity: usize, notifier: Notifier) -> Arc<Self> {
        #[cfg(target_arch = "wasm32")]
        let _ = notifier;
        Arc::new(Self {
            queue: Queue::new(capacity.max(2).next_power_of_two()),
            #[cfg(not(target_arch = "wasm32"))]
            notifier,
            closed: AtomicBool::new(false),
        })
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub fn pop(&self) -> Option<WorkResult> {
        self.queue.pop()
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn complete(&self, result: WorkResult) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        // One reserved core operation credit per job remains held until delivery.
        // Thus this separate queue (>= max_operations) cannot overflow.
        assert!(
            self.queue.push(result).is_ok(),
            "blocking completion credit invariant"
        );
        let _ = self.notifier.notify();
    }
}
#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub(crate) trait ReusableWork: Send + Sync {
    fn run(&self);
}
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::{
        collections::VecDeque,
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{Condvar, Mutex, OnceLock},
        thread,
    };
    struct Job {
        op: OpId,
        cancel: Arc<AtomicBool>,
        port: Arc<WorkPort>,
        f: Box<dyn FnOnce() -> Result<WorkOutput> + Send>,
    }
    enum Task {
        Boxed(Job),
        #[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
        Reusable(Arc<dyn ReusableWork>),
    }
    struct State {
        jobs: Mutex<VecDeque<Task>>,
        ready: Condvar,
        stopping: AtomicBool,
    }
    struct Pool {
        state: Arc<State>,
        config: PoolConfig,
    }
    static POOL: OnceLock<Result<Pool>> = OnceLock::new();
    fn start(config: PoolConfig) -> Result<Pool> {
        let state = Arc::new(State {
            jobs: Mutex::new(VecDeque::with_capacity(config.queue_capacity)),
            ready: Condvar::new(),
            stopping: AtomicBool::new(false),
        });
        let mut handles = Vec::with_capacity(config.threads);
        for i in 0..config.threads {
            let s = state.clone();
            let worker = thread::Builder::new()
                .name(format!("turnloop-blocking-{i}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let mut jobs = s
                                .jobs
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            while jobs.is_empty() && !s.stopping.load(Ordering::Acquire) {
                                jobs = s
                                    .ready
                                    .wait(jobs)
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                            }
                            if s.stopping.load(Ordering::Acquire) {
                                return;
                            }
                            jobs.pop_front().expect("nonempty job queue")
                        };
                        let job = match job {
                            Task::Boxed(job) => job,
                            #[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
                            Task::Reusable(work) => {
                                work.run();
                                continue;
                            }
                        };
                        let result = if job.cancel.load(Ordering::Acquire) {
                            Err(Error::new(ErrorKind::Cancelled))
                        } else {
                            catch_unwind(AssertUnwindSafe(job.f))
                                .unwrap_or_else(|_| Err(Error::new(ErrorKind::Other)))
                        };
                        job.port.complete(WorkResult { op: job.op, result });
                    }
                });
            match worker {
                Ok(handle) => handles.push(handle),
                Err(e) => {
                    state.stopping.store(true, Ordering::Release);
                    state.ready.notify_all();
                    for h in handles {
                        let _ = h.join();
                    }
                    return Err(e.into());
                }
            }
        }
        // Workers live for the process lifetime, shared by every submitting loop.
        Ok(Pool { state, config })
    }
    pub(super) fn submit(
        config: PoolConfig,
        op: OpId,
        cancel: Arc<AtomicBool>,
        port: Arc<WorkPort>,
        f: Box<dyn FnOnce() -> Result<WorkOutput> + Send>,
    ) -> Result<()> {
        if config.threads == 0 || config.queue_capacity == 0 {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let pool = POOL
            .get_or_init(|| start(config))
            .as_ref()
            .map_err(|&e| e)?;
        if pool.config != config {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let mut jobs = pool
            .state
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if jobs.len() == pool.config.queue_capacity {
            return Err(Error::new(ErrorKind::ResourceLimit));
        }
        jobs.push_back(Task::Boxed(Job {
            op,
            cancel,
            port,
            f,
        }));
        drop(jobs);
        pool.state.ready.notify_one();
        Ok(())
    }
    #[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
    pub(crate) fn reusable(config: PoolConfig, work: Arc<dyn ReusableWork>) -> Result<()> {
        if config.threads == 0 || config.queue_capacity == 0 {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let pool = POOL
            .get_or_init(|| start(config))
            .as_ref()
            .map_err(|&e| e)?;
        if pool.config != config {
            return Err(Error::new(ErrorKind::InvalidInput));
        }
        let mut jobs = pool
            .state
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if jobs.len() == pool.config.queue_capacity {
            return Err(Error::new(ErrorKind::ResourceLimit));
        }
        jobs.push_back(Task::Reusable(work));
        pool.state.ready.notify_one();
        Ok(())
    }
}
pub(crate) fn submit(
    config: PoolConfig,
    op: OpId,
    cancel: Arc<AtomicBool>,
    port: Arc<WorkPort>,
    f: Box<dyn FnOnce() -> Result<WorkOutput> + Send>,
) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::submit(config, op, cancel, port, f)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (config, op, cancel, port, f);
        Err(Error::new(ErrorKind::Unsupported))
    }
}
pub(crate) fn blocking(
    f: impl FnOnce() -> BlockingResult + Send + 'static,
) -> Box<dyn FnOnce() -> Result<WorkOutput> + Send> {
    Box::new(move || f().map(WorkOutput::Blocking))
}
pub(crate) fn resolve(request: DnsRequest) -> Box<dyn FnOnce() -> Result<WorkOutput> + Send> {
    Box::new(move || {
        // std's native ToSocketAddrs implementation uses getaddrinfo (Unix) or
        // GetAddrInfoW (Windows). Only owned Rust data runs on the pool thread.
        let addresses: Vec<_> = (request.host.as_str(), request.port)
            .to_socket_addrs()
            .map_err(Error::from)?
            .collect();
        if addresses.is_empty() {
            return Err(Error::new(ErrorKind::NotFound));
        }
        Ok(WorkOutput::Resolved(addresses))
    })
}

#[cfg(all(test, loom))]
mod models {
    use super::*;
    use crate::{
        backend::Wake,
        sync::{AtomicUsize, Ordering as ModelOrdering},
    };
    struct WakeCounter(AtomicUsize);
    impl Wake for WakeCounter {
        fn wake(&self) -> Result<()> {
            self.0.fetch_add(1, ModelOrdering::Relaxed);
            Ok(())
        }
        fn syscall_count(&self) -> u64 {
            self.0.load(ModelOrdering::Relaxed) as u64
        }
    }
    #[test]
    fn pool_result_publication_races_loop_parking() {
        loom::model(|| {
            let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
            let notifier = Notifier::new(wake.clone());
            let port = WorkPort::new(2, notifier.clone());
            let worker = port.clone();
            let op = OpId {
                owner: 7,
                key: 1 << 32,
            };
            let t = loom::thread::spawn(move || {
                worker.complete(WorkResult {
                    op,
                    result: Ok(WorkOutput::Blocking(Payload::U64(42))),
                })
            });
            let parked = notifier.park();
            t.join().expect("pool worker");
            let completion = port.pop().expect("published completion");
            assert_eq!(completion.op, op);
            assert!(matches!(
                completion.result,
                Ok(WorkOutput::Blocking(Payload::U64(42)))
            ));
            assert!(port.pop().is_none());
            if parked {
                assert_eq!(wake.syscall_count(), 1);
            } else {
                assert!(notifier.begin());
            }
        });
    }
}

#[cfg(any(turnloop_backend = "kqueue", turnloop_backend = "epoll"))]
pub(crate) use native::reusable;
