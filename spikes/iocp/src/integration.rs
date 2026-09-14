//! Opt-in, sole IOCP consumer. The host drains a bounded preallocated queue.
use crate::port::{Entry, Port, STOP, Wait, bool_result, owned};
use std::{
    collections::VecDeque,
    io,
    os::windows::io::{AsRawHandle, OwnedHandle},
    ptr,
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::Threading::{CreateEventW, SetEvent},
};

struct Queue {
    entries: VecDeque<Entry>,
    high_water: usize,
    stopping: bool,
    error: Option<io::Error>,
}
struct Shared {
    queue: Mutex<Queue>,
    room: Condvar,
    event: OwnedHandle,
}

pub struct EventIntegration {
    shared: Arc<Shared>,
    port: Arc<Port>,
    worker: Option<JoinHandle<()>>,
}

impl EventIntegration {
    /// The caller must stop calling Port::wait while this helper is active.
    pub fn new(port: Arc<Port>) -> io::Result<Self> {
        // SAFETY: auto-reset event, initially nonsignaled, owned until helper joined.
        let event = unsafe { owned(CreateEventW(ptr::null(), 0, 0, ptr::null())) }?;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                entries: VecDeque::with_capacity(128),
                high_water: 0,
                stopping: false,
                error: None,
            }),
            room: Condvar::new(),
            event,
        });
        let worker_shared = Arc::clone(&shared);
        let worker_port = Arc::clone(&port);
        let worker = std::thread::Builder::new()
            .name("windlass-iocp-event".into())
            .spawn(move || {
                if let Err(error) = pump(&worker_shared, &worker_port) {
                    let mut queue = worker_shared
                        .queue
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    queue.error = Some(error);
                    // SAFETY: event owned by worker_shared; report pump failure to host.
                    unsafe {
                        SetEvent(worker_shared.event.as_raw_handle());
                    }
                }
            })?;
        Ok(Self {
            shared,
            port,
            worker: Some(worker),
        })
    }
    pub fn event(&self) -> HANDLE {
        self.shared.event.as_raw_handle()
    }
    pub fn high_water(&self) -> io::Result<usize> {
        Ok(self
            .shared
            .queue
            .lock()
            .map_err(|_| io::Error::other("helper queue poisoned"))?
            .high_water)
    }
    pub fn drain(&mut self, out: &mut [Entry]) -> io::Result<usize> {
        let mut queue = self
            .shared
            .queue
            .lock()
            .map_err(|_| io::Error::other("helper queue poisoned"))?;
        if let Some(error) = queue.error.take() {
            return Err(error);
        }
        let mut n = 0;
        for slot in out {
            let Some(entry) = queue.entries.pop_front() else {
                break;
            };
            *slot = entry;
            n += 1;
        }
        if !queue.entries.is_empty() {
            // SAFETY: live auto-reset event; partial drains must re-signal while
            // holding the queue lock, or the external waiter can miss remaining work.
            // https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setevent
            bool_result(unsafe { SetEvent(self.event()) })?;
        }
        self.shared.room.notify_one();
        Ok(n)
    }
    pub fn shutdown(&mut self) -> io::Result<()> {
        if self.worker.is_none() {
            return Ok(());
        }
        {
            let mut queue = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            queue.stopping = true;
            self.shared.room.notify_all();
        }
        self.port.post(STOP, 0)?;
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("helper panicked"))?;
        }
        Ok(())
    }
}
impl Drop for EventIntegration {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn pump(shared: &Shared, port: &Port) -> io::Result<()> {
    let mut batch = [Entry::default(); 64];
    loop {
        {
            let queue = shared
                .queue
                .lock()
                .map_err(|_| io::Error::other("helper queue poisoned"))?;
            if queue.stopping {
                return Ok(());
            }
        }
        let Wait::Entries(n) = port.wait(None, false, &mut batch)? else {
            continue;
        };
        for entry in &batch[..n] {
            let mut queue = shared
                .queue
                .lock()
                .map_err(|_| io::Error::other("helper queue poisoned"))?;
            while queue.entries.len() == 128 && !queue.stopping {
                queue = shared
                    .room
                    .wait(queue)
                    .map_err(|_| io::Error::other("helper queue poisoned"))?;
            }
            if queue.stopping || entry.key == STOP {
                return Ok(());
            }
            queue.entries.push_back(*entry); // capacity checked: never grows
            queue.high_water = queue.high_water.max(queue.entries.len());
            // SAFETY: shared owns live event; queue published before wake.
            bool_result(unsafe { SetEvent(shared.event.as_raw_handle()) })?;
        }
    }
}
