use crate::*;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
#[cfg(target_vendor = "apple")]
#[path = "watch_apple.rs"]
mod sys;
#[cfg(all(unix, not(target_vendor = "apple")))]
#[path = "watch_unix.rs"]
mod sys;
#[cfg(windows)]
#[path = "watch_windows.rs"]
mod sys;
pub(super) const CHANGE: u32 = 1;
pub(super) const RENAME: u32 = 2;
pub(super) const OVERFLOW: u32 = 4;
const STOPPED: u32 = 8;
pub(super) struct State {
    flags: AtomicU32,
    notifier: Notifier,
}
impl State {
    pub fn event(&self, flags: u32) {
        if self.flags.fetch_or(flags, Ordering::AcqRel) & flags != flags {
            let _ = self.notifier.notify();
        }
    }
    pub fn stopped(&self) {
        self.event(STOPPED);
    }
}
struct Active {
    op: OpId,
    state: Arc<State>,
    watch: sys::Watch,
    finished: bool,
}
pub(crate) struct Watches {
    slots: Vec<Option<Active>>,
    cursor: usize,
    notifier: Notifier,
}
impl Watches {
    pub fn new(config: &Config, notifier: Notifier) -> Self {
        Self {
            slots: (0..config.max_handles).map(|_| None).collect(),
            cursor: 0,
            notifier,
        }
    }
    pub fn start(&mut self, h: Handle, op: OpId, path: &FsPath, recursive: bool) -> Result<()> {
        let state = Arc::new(State {
            flags: AtomicU32::new(0),
            notifier: self.notifier.clone(),
        });
        let watch = sys::Watch::new(path, recursive, state.clone())?;
        self.slots[h.index()] = Some(Active {
            op,
            state,
            watch,
            finished: false,
        });
        Ok(())
    }
    pub fn cancel(&mut self, h: Handle) -> Result<()> {
        self.slots[h.index()]
            .as_mut()
            .ok_or(Error::new(ErrorKind::NotFound))?
            .watch
            .cancel()
    }
    pub fn has_work(&self) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|a| !a.finished && a.state.flags.load(Ordering::Acquire) != 0)
    }
    pub fn poll(&mut self) -> Option<(OpId, Option<WatchEvent>)> {
        for _ in 0..self.slots.len() {
            let i = self.cursor;
            self.cursor = (i + 1) % self.slots.len();
            let Some(a) = self.slots[i].as_mut().filter(|a| !a.finished) else {
                continue;
            };
            let flags = a.state.flags.swap(0, Ordering::AcqRel);
            if flags & STOPPED != 0 {
                a.finished = true;
                return Some((a.op, None));
            }
            if flags != 0 {
                return Some((
                    a.op,
                    Some(WatchEvent {
                        changed: flags & CHANGE != 0,
                        renamed: flags & RENAME != 0,
                        overflow: flags & OVERFLOW != 0,
                    }),
                ));
            }
        }
        None
    }
    pub fn release(&mut self, h: Handle) {
        self.slots[h.index()].take();
    }
}
