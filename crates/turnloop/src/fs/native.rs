use super::*;
use crate::{
    Config, OpId,
    blocking::{ReusableWork, WorkOutput, WorkPort, WorkResult},
};
use std::{
    fs::File,
    sync::{
        Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};
#[cfg(unix)]
#[path = "unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "windows.rs"]
mod sys;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
#[derive(Default)]
pub(super) struct FileState {
    pub file: Option<File>,
    pub append: bool,
    #[cfg(windows)]
    pub cursor: u64,
}
enum Request {
    Open(FsPath, FileOptions),
    Io(FsRequest),
}
struct Job {
    op: OpId,
    request: Request,
    file: Option<Arc<Mutex<FileState>>>,
}
// SAFETY: only File, prepared paths and owned/provided bytes move to the worker.
// The core retains exclusive buffer ownership through publication; Drop joins all workers.
unsafe impl Send for Job {}
struct Port {
    work: Arc<WorkPort>,
    running: Mutex<usize>,
    done: Condvar,
}
struct Slot {
    job: Mutex<Option<Job>>,
    cancel: AtomicBool,
    port: Arc<Port>,
}
impl ReusableWork for Slot {
    fn run(&self) {
        let job = lock(&self.job).take().expect("accepted filesystem job");
        let op = job.op;
        let result = if self.cancel.load(Ordering::Acquire) {
            Err(Error::new(ErrorKind::Cancelled))
        } else {
            execute(job)
        };
        // Keep teardown and slot reuse ordered with result publication. No worker
        // accesses the request/buffer or cancel flag after publishing its result.
        let mut running = lock(&self.port.running);
        self.port.work.complete(WorkResult {
            op,
            result: result.map(WorkOutput::Fs),
        });
        *running -= 1;
        self.port.done.notify_all();
    }
}
struct Active {
    op: OpId,
    handle: Option<Handle>,
    next: Option<usize>,
    started: bool,
}
pub(crate) struct Service {
    slots: Vec<Arc<Slot>>,
    files: Vec<Arc<Mutex<FileState>>>,
    active: Vec<Option<Active>>,
    heads: Vec<Option<usize>>,
    tails: Vec<Option<usize>>,
    port: Arc<Port>,
    config: crate::PoolConfig,
}
impl Service {
    pub fn new(config: &Config, work: Arc<WorkPort>) -> Self {
        let port = Arc::new(Port {
            work,
            running: Mutex::new(0),
            done: Condvar::new(),
        });
        drop(lock(&port.running));
        let slots = (0..config.max_operations)
            .map(|_| {
                let slot = Arc::new(Slot {
                    job: Mutex::new(None),
                    cancel: AtomicBool::new(false),
                    port: port.clone(),
                });
                drop(lock(&slot.job));
                slot
            })
            .collect();
        let files = (0..config.max_handles)
            .map(|_| {
                let file = Arc::new(Mutex::new(FileState::default()));
                drop(lock(&file));
                file
            })
            .collect();
        Self {
            slots,
            files,
            active: (0..config.max_operations).map(|_| None).collect(),
            heads: vec![None; config.max_handles],
            tails: vec![None; config.max_handles],
            port,
            config: config.blocking_pool,
        }
    }
    pub fn open(&mut self, op: OpId, handle: Handle, path: FsPath, options: FileOptions) {
        self.enqueue(op, Some(handle), Request::Open(path, options));
    }
    pub fn submit(&mut self, op: OpId, request: FsRequest) {
        self.enqueue(op, request.handle(), Request::Io(request));
    }
    fn enqueue(&mut self, op: OpId, handle: Option<Handle>, request: Request) {
        let i = op.index();
        self.slots[i].cancel.store(false, Ordering::Release);
        *lock(&self.slots[i].job) = Some(Job {
            op,
            request,
            file: handle.map(|h| self.files[h.index()].clone()),
        });
        self.active[i] = Some(Active {
            op,
            handle,
            next: None,
            started: false,
        });
        let mut start = true;
        if let Some(h) = handle {
            if let Some(tail) = self.tails[h.index()] {
                self.active[tail].as_mut().expect("file tail").next = Some(i);
                start = false;
            } else {
                self.heads[h.index()] = Some(i);
            }
            self.tails[h.index()] = Some(i);
        }
        if start {
            self.start(i);
        }
    }
    fn start(&mut self, i: usize) {
        let active = self.active[i].as_mut().expect("file head");
        active.started = true;
        let mut running = lock(&self.port.running);
        *running += 1;
        if let Err(error) = crate::blocking::reusable(self.config, self.slots[i].clone()) {
            lock(&self.slots[i].job).take();
            *running -= 1;
            self.port.work.complete(WorkResult {
                op: active.op,
                result: Err(error),
            });
        }
    }
    pub fn contains(&self, op: OpId) -> bool {
        self.active
            .get(op.index())
            .and_then(Option::as_ref)
            .is_some_and(|a| a.op == op)
    }
    pub fn cancel(&self, op: OpId) {
        self.slots[op.index()].cancel.store(true, Ordering::Release);
    }
    pub fn complete(&mut self, op: OpId) {
        let active = self.active[op.index()]
            .take()
            .expect("filesystem completion");
        if let Some(h) = active.handle {
            self.heads[h.index()] = active.next;
            if let Some(next) = active.next {
                self.start(next);
            } else {
                self.tails[h.index()] = None;
            }
        }
    }
    pub fn release(&mut self, h: Handle) {
        *lock(&self.files[h.index()]) = FileState::default();
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        for (i, a) in self.active.iter().enumerate() {
            if a.is_some() {
                self.slots[i].cancel.store(true, Ordering::Release);
            }
        }
        let mut running = lock(&self.port.running);
        while *running != 0 {
            running = self
                .port
                .done
                .wait(running)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}
fn execute(job: Job) -> Result<FsResult> {
    match job.request {
        Request::Open(path, options) => {
            let mut state = lock(job.file.as_ref().expect("open handle"));
            state.file = Some(sys::open(&path, options)?);
            state.append = options.append;
            Ok(FsResult::Opened)
        }
        Request::Io(request) => {
            let mut state = job.file.as_ref().map(|f| lock(f));
            sys::execute(request, state.as_deref_mut())
        }
    }
}
pub(super) fn file(state: Option<&mut FileState>) -> Result<&mut FileState> {
    state
        .filter(|s| s.file.is_some())
        .ok_or(Error::new(ErrorKind::NotFound))
}
pub(super) fn output(buffer: &mut IoBufMut) -> &mut [u8] {
    // SAFETY: worker exclusively owns this accepted request's output memory until publication.
    unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr(), buffer.len()) }
}
pub(super) fn record(buffer: &mut [u8], used: &mut usize, name: &[u8], kind: u8) -> bool {
    if name.len() > u16::MAX as usize || buffer.len() - *used < name.len() + 3 {
        return false;
    }
    let len = (name.len() as u16).to_le_bytes();
    buffer[*used..*used + 2].copy_from_slice(&len);
    buffer[*used + 2] = kind;
    buffer[*used + 3..*used + 3 + name.len()].copy_from_slice(name);
    *used += name.len() + 3;
    true
}
