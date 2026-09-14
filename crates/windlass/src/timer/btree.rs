use super::{Entry, Instant};
use std::collections::BTreeMap;
/// Benchmark comparison only. Repeated large batches allocate BTreeMap nodes;
/// therefore the production Driver uses Heap even with the comparison enabled.
pub struct Tree {
    tree: BTreeMap<Entry, ()>,
    keys: Vec<Option<Entry>>,
}
impl Tree {
    pub fn new(capacity: usize) -> Self {
        Self {
            tree: BTreeMap::new(),
            keys: vec![None; capacity],
        }
    }
    pub fn len(&self) -> usize {
        self.tree.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.tree.first_key_value().map(|(e, _)| e.at)
    }
    pub fn insert(&mut self, id: u64, at: Instant) {
        let i = id as u32 as usize;
        assert!(self.keys[i].is_none(), "one timer per slot");
        let e = Entry { id, at };
        self.keys[i] = Some(e);
        self.tree.insert(e, ());
    }
    pub fn cancel(&mut self, id: u64) -> bool {
        let i = id as u32 as usize;
        let Some(Some(e)) = self.keys.get(i).copied() else {
            return false;
        };
        if e.id != id {
            return false;
        }
        self.keys[i] = None;
        self.tree.remove(&e).is_some()
    }
    pub fn pop_expired(&mut self, now: Instant) -> Option<(u64, Instant)> {
        let e = *self.tree.first_key_value()?.0;
        if e.at > now {
            return None;
        }
        self.cancel(e.id);
        Some((e.id, e.at))
    }
}
