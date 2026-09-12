//! Dense storage whose cost depends on concurrent sounds, not session length.

use super::commands::SoundId;

pub(crate) struct SoundPool<T> {
    entries: Vec<(SoundId, T)>,
}

impl<T> SoundPool<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    pub fn insert(&mut self, id: SoundId, sound: T) {
        if let Some(existing) = self.get_mut(id) {
            *existing = sound;
        } else {
            self.entries.push((id, sound));
        }
    }

    pub fn get_mut(&mut self, id: SoundId) -> Option<&mut T> {
        self.entries
            .iter_mut()
            .find(|(key, _)| *key == id)
            .map(|(_, sound)| sound)
    }

    pub fn remove(&mut self, id: SoundId) {
        if let Some(index) = self.entries.iter().position(|(key, _)| *key == id) {
            self.entries.swap_remove(index);
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut (SoundId, T)> {
        self.entries.iter_mut()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_is_reused_and_ids_survive_swap_removal() {
        let mut pool = SoundPool::with_capacity(2);
        for id in 1..10_000 {
            pool.insert(id, id);
            pool.remove(id);
        }
        pool.insert(u64::MAX, 1);
        pool.insert(50_000, 2);
        pool.remove(u64::MAX);
        assert_eq!(pool.get_mut(50_000), Some(&mut 2));
        assert!(pool.get_mut(u64::MAX).is_none());
        assert_eq!(pool.entries.capacity(), 2);
    }
}
