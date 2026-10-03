//! Fixed atomic registry: callback completion never locks a concurrent map.
use std::sync::atomic::{AtomicU64, Ordering};
const SLOTS: usize = 1024;
pub(crate) struct PlayingStates {
    slots: [AtomicU64; SLOTS],
}
impl PlayingStates {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
    pub fn insert(&self, id: u64, _: ()) -> bool {
        assert_ne!(id, 0);
        self.slots[id as usize % SLOTS]
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }
    pub fn remove(&self, id: &u64) {
        let _ = self.slots[*id as usize % SLOTS].compare_exchange(
            *id,
            0,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
    }
    #[cfg(feature = "worker")]
    pub fn ids(&self) -> Vec<String> {
        self.slots.iter().map(|slot|slot.load(Ordering::Acquire)).filter(|id|*id!=0).map(|id|id.to_string()).collect()
    }
    pub fn contains_key(&self, id: &u64) -> bool {
        *id != 0 && self.slots[*id as usize % SLOTS].load(Ordering::Acquire) == *id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_completion_cannot_clear_reused_slot() {
        let states = PlayingStates::new();
        assert!(states.insert(1, ()));
        assert!(!states.insert(1025, ()));
        states.remove(&1);
        assert!(states.insert(1025, ()));
        states.remove(&1);
        assert!(states.contains_key(&1025));
        assert!(!states.contains_key(&1));
    }
}
