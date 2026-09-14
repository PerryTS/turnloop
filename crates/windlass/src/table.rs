//! Fixed-capacity generational storage. Exhausted generations retire their slot.
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}
pub(crate) struct Table<T> {
    slots: Vec<Slot<T>>,
    free: Vec<usize>,
}
impl<T> Table<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: (0..capacity)
                .map(|_| Slot {
                    generation: 1,
                    value: None,
                })
                .collect(),
            free: (0..capacity).rev().collect(),
        }
    }
    pub fn insert(&mut self, value: T) -> Option<u64> {
        let i = self.free.pop()?;
        self.slots[i].value = Some(value);
        Some((u64::from(self.slots[i].generation) << 32) | i as u64)
    }
    pub fn get(&self, key: u64) -> Option<&T> {
        let s = self.slots.get(key as u32 as usize)?;
        (s.generation == (key >> 32) as u32)
            .then_some(s.value.as_ref())
            .flatten()
    }
    pub fn get_mut(&mut self, key: u64) -> Option<&mut T> {
        let s = self.slots.get_mut(key as u32 as usize)?;
        (s.generation == (key >> 32) as u32)
            .then_some(s.value.as_mut())
            .flatten()
    }
    pub fn remove(&mut self, key: u64) -> Option<T> {
        let i = key as u32 as usize;
        let s = self.slots.get_mut(i)?;
        if s.generation != (key >> 32) as u32 {
            return None;
        }
        let value = s.value.take()?;
        if let Some(generation) = s.generation.checked_add(1) {
            s.generation = generation;
            self.free.push(i);
        }
        Some(value)
    }
    pub fn at(&self, index: usize) -> Option<(u64, &T)> {
        let s = self.slots.get(index)?;
        s.value
            .as_ref()
            .map(|v| (((s.generation as u64) << 32) | index as u64, v))
    }
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_generation_never_reaches_reused_slot() {
        let mut t = Table::new(1);
        let a = t.insert(12).expect("capacity");
        assert_eq!(t.remove(a), Some(12));
        let b = t.insert(34).expect("capacity");
        assert_ne!(a, b);
        assert_eq!(t.get(a), None);
        assert_eq!(t.remove(a), None);
        assert_eq!(t.get(b), Some(&34));
    }
}
