//! Dense storage whose cost depends on concurrent sounds, not session length.

use super::commands::SoundId;

pub(crate) struct SoundPool<T> {
    entries: Vec<(SoundId, T)>,
    pub(crate) retired: std::sync::Arc<crossbeam::queue::ArrayQueue<T>>,
}

impl<T> SoundPool<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            retired: std::sync::Arc::new(crossbeam::queue::ArrayQueue::new(1024)),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn retirement_available(&self) -> bool {
        self.retired.capacity() - self.retired.len()
            >= self.entries.len() + super::command_queue::COMMANDS_PER_CALLBACK * 2
    }

    pub fn insert(&mut self, id: SoundId, sound: T) {
        if let Some(existing) = self.get_mut(id) {
            let old = std::mem::replace(existing, sound);
            assert!(
                self.retired.push(old).is_ok(),
                "retirement capacity must be checked before rendering"
            );
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
            let (_, sound) = self.entries.swap_remove(index);
            assert!(
                self.retired.push(sound).is_ok(),
                "retirement capacity must be checked before rendering"
            );
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut (SoundId, T)> {
        self.entries.iter_mut()
    }

    pub fn clear(&mut self) {
        while let Some((_, sound)) = self.entries.pop() {
            assert!(
                self.retired.push(sound).is_ok(),
                "retirement capacity must be checked before rendering"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_defers_destructors_until_maintenance() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Count(Arc<AtomicUsize>);
        impl Drop for Count {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let mut pool = SoundPool::with_capacity(2);
        pool.insert(1, Count(drops.clone()));
        pool.remove(1);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(pool.retired.pop());
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn storage_is_reused_and_ids_survive_swap_removal() {
        let mut pool = SoundPool::with_capacity(2);
        for id in 1..10_000 {
            pool.insert(id, id);
            pool.remove(id);
            while pool.retired.pop().is_some() {}
        }
        pool.insert(u64::MAX, 1);
        pool.insert(50_000, 2);
        pool.remove(u64::MAX);
        assert_eq!(pool.get_mut(50_000), Some(&mut 2));
        assert!(pool.get_mut(u64::MAX).is_none());
        assert_eq!(pool.entries.capacity(), 2);
    }
}
