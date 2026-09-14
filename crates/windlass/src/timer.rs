//! Benchmarkable timer queue: indexed four-way heap, or `timer-btree` comparison.
use crate::Instant;
#[cfg(feature = "timer-btree")]
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Entry {
    at: Instant,
    id: u64,
}

/// Keys encode a slot index in their low 32 bits and a generation in the high bits.
/// At most one deadline per slot. Cancellation removes storage immediately.
pub struct TimerQueue {
    #[cfg(not(feature = "timer-btree"))]
    heap: Vec<Entry>,
    #[cfg(not(feature = "timer-btree"))]
    positions: Vec<usize>,
    #[cfg(feature = "timer-btree")]
    tree: BTreeMap<Entry, ()>,
    #[cfg(feature = "timer-btree")]
    keys: Vec<Option<Entry>>,
}
impl TimerQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            #[cfg(not(feature = "timer-btree"))]
            heap: Vec::with_capacity(capacity),
            #[cfg(not(feature = "timer-btree"))]
            positions: vec![usize::MAX; capacity],
            #[cfg(feature = "timer-btree")]
            tree: BTreeMap::new(),
            #[cfg(feature = "timer-btree")]
            keys: vec![None; capacity],
        }
    }
    pub fn len(&self) -> usize {
        #[cfg(not(feature = "timer-btree"))]
        {
            self.heap.len()
        }
        #[cfg(feature = "timer-btree")]
        {
            self.tree.len()
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.first().map(|e| e.at)
    }
    fn first(&self) -> Option<Entry> {
        #[cfg(not(feature = "timer-btree"))]
        {
            self.heap.first().copied()
        }
        #[cfg(feature = "timer-btree")]
        {
            self.tree.first_key_value().map(|(&k, _)| k)
        }
    }
    pub fn insert(&mut self, id: u64, at: Instant) {
        let i = id as u32 as usize;
        #[cfg(not(feature = "timer-btree"))]
        {
            assert_eq!(self.positions[i], usize::MAX, "one timer per slot");
            self.positions[i] = self.heap.len();
            self.heap.push(Entry { id, at });
            self.up(self.heap.len() - 1);
        }
        #[cfg(feature = "timer-btree")]
        {
            assert!(self.keys[i].is_none(), "one timer per slot");
            let e = Entry { id, at };
            self.keys[i] = Some(e);
            self.tree.insert(e, ());
        }
    }
    pub fn cancel(&mut self, id: u64) -> bool {
        let i = id as u32 as usize;
        #[cfg(not(feature = "timer-btree"))]
        {
            let Some(&p) = self.positions.get(i) else {
                return false;
            };
            if p == usize::MAX || self.heap[p].id != id {
                return false;
            }
            self.heap.swap_remove(p);
            self.positions[i] = usize::MAX;
            if p < self.heap.len() {
                self.positions[self.heap[p].id as u32 as usize] = p;
                if p > 0 && self.heap[p] < self.heap[(p - 1) / 4] {
                    self.up(p);
                } else {
                    self.down(p);
                }
            }
            true
        }
        #[cfg(feature = "timer-btree")]
        {
            let Some(Some(e)) = self.keys.get(i).copied() else {
                return false;
            };
            if e.id != id {
                return false;
            }
            self.keys[i] = None;
            self.tree.remove(&e).is_some()
        }
    }
    pub fn pop_expired(&mut self, now: Instant) -> Option<(u64, Instant)> {
        let e = self.first()?;
        if e.at > now {
            return None;
        }
        self.cancel(e.id);
        Some((e.id, e.at))
    }
    #[cfg(not(feature = "timer-btree"))]
    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.positions[self.heap[a].id as u32 as usize] = a;
        self.positions[self.heap[b].id as u32 as usize] = b;
    }
    #[cfg(not(feature = "timer-btree"))]
    fn up(&mut self, mut p: usize) {
        while p > 0 {
            let parent = (p - 1) / 4;
            if self.heap[parent] <= self.heap[p] {
                break;
            }
            self.swap(p, parent);
            p = parent;
        }
    }
    #[cfg(not(feature = "timer-btree"))]
    fn down(&mut self, mut p: usize) {
        loop {
            let first = p * 4 + 1;
            if first >= self.heap.len() {
                break;
            }
            let mut best = first;
            for c in first + 1..(first + 4).min(self.heap.len()) {
                if self.heap[c] < self.heap[best] {
                    best = c;
                }
            }
            if self.heap[p] <= self.heap[best] {
                break;
            }
            self.swap(p, best);
            p = best;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn cancel_and_expire_match_sorted_reference() {
        #[cfg(not(windlass_backend = "web"))]
        let base = Instant::now();
        #[cfg(windlass_backend = "web")]
        let base = Instant::from_duration(Duration::from_secs(1));
        let mut q = TimerQueue::new(1000);
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
