use turnloop::Instant;
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Entry {
    at: Instant,
    id: u64,
}
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn cancel_and_expire_match_sorted_reference() {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let base = Instant::now();
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let base = Instant::from_duration(Duration::from_secs(1));
        let mut q = Tree::new(1000);
        let mut reference = Vec::new();
        for i in 0..1000u64 {
            let at = base + Duration::from_nanos((i * 691) % 1000);
            q.insert((1 << 32) | i, at);
            if i % 3 != 0 {
                reference.push((at, (1 << 32) | i));
            }
        }
        for i in (0..1000u64).step_by(3) {
            assert!(q.cancel((1 << 32) | i));
        }
        reference.sort();
        assert_eq!(q.len(), reference.len());
        assert!(q.pop_expired(base - Duration::from_nanos(1)).is_none());
        let mut ran = 0;
        for (at, id) in reference {
            assert_eq!(q.pop_expired(base + Duration::from_secs(1)), Some((id, at)));
            ran += 1;
        }
        assert_eq!(ran, 666);
        assert!(q.is_empty());
    }
}
