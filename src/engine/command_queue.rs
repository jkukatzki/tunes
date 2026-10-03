//! Bounded FIFO with producer-side coalescing and reserved release capacity.
use super::commands::AudioCommand;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub(crate) const COMMANDS_PER_CALLBACK: usize = 64;
const NORMAL_CAPACITY: usize = 256;
const CAPACITY: usize = 384;

struct PendingCommands {
    commands: VecDeque<AudioCommand>,
    // Only the trailing run of parameter updates is indexed. Ordering barriers
    // clear the index; draining shifts surviving indices without allocation.
    parameters: ahash::AHashMap<(u8, u64), usize>,
}

#[derive(Clone)]
pub(crate) struct CommandSender(
    Arc<Mutex<PendingCommands>>,
    Arc<super::playing_states::PlayingStates>,
    Arc<AtomicBool>,
    Arc<AtomicBool>,
    #[cfg(all(feature = "worker", target_arch = "wasm32"))] pub(crate) Option<u32>,
);

impl CommandSender {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_states(Arc::new(super::playing_states::PlayingStates::new()))
    }
    pub fn with_states(states: Arc<super::playing_states::PlayingStates>) -> Self {
        Self(
            Arc::new(Mutex::new(PendingCommands {
                commands: VecDeque::with_capacity(CAPACITY),
                parameters: ahash::AHashMap::with_capacity(CAPACITY),
            })),
            states,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(true)),
            #[cfg(all(feature = "worker", target_arch = "wasm32"))]
            None,
        )
    }
    #[cfg(all(feature = "worker", target_arch = "wasm32"))]
    pub fn for_worker(session: u32, states: Arc<super::playing_states::PlayingStates>) -> Self {
        let mut sender = Self::with_states(states);
        sender.4 = Some(session);
        sender
    }
    pub fn close(&self) {
        #[cfg(all(feature = "worker", target_arch = "wasm32"))]
        if let Some(session) = self.4 {
            super::worker_transport::close(session);
        }

        if let Ok(mut queue) = self.0.lock() {
            self.3.store(false, Ordering::Release);
            queue.commands.clear();
            queue.parameters.clear();
        }
    }
    pub fn same_channel(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub fn send(&self, command: AudioCommand) -> Result<(), ()> {
        #[cfg(all(feature = "worker", target_arch = "wasm32"))]
        if let Some(session) = self.4 {
            return super::worker_transport::send(session, &command);
        }

        let mut queue = self.0.lock().map_err(|_| ())?;
        if !self.3.load(Ordering::Acquire) {
            return Err(());
        }
        if let AudioCommand::RemoveEffectBus { id } = &command {
            if queue
                .commands
                .iter()
                .any(|c| matches!(c, AudioCommand::RemoveEffectBus {id: other} if id == other))
            {
                return Ok(());
            }
        }
        // Coalesce only within the trailing parameter-update run. A play, fade,
        // pause, or stop is an ordering barrier, so fade starting gains are intact.
        let parameter_key = command.parameter_key();
        if let Some(key) = parameter_key {
            if let Some(&index) = queue.parameters.get(&key) {
                queue.commands[index] = command;
                return Ok(());
            }
        }
        let limit = if command.is_release() {
            CAPACITY
        } else {
            NORMAL_CAPACITY
        };
        if queue.commands.len() >= limit {
            if !command.is_release() {
                return Err(());
            }
            // Catastrophic control overload: cancel pending attacks and stop all
            // voices instead of losing a note-off. Destruction happens HERE on
            // the producer. Bus lifecycle commands retain their FIFO ordering.
            queue.commands.retain(|pending| {
                match pending {
                    AudioCommand::Play { id, .. } | AudioCommand::PlaySource { id, .. } => {
                        self.1.remove(id);
                    }
                    _ => {}
                }
                matches!(
                    pending,
                    AudioCommand::SetEffectBus { .. } | AudioCommand::RemoveEffectBus { .. }
                )
            });
            queue.parameters.clear();
            self.2.store(true, Ordering::Release);
            if !matches!(command, AudioCommand::RemoveEffectBus { .. }) {
                return Ok(());
            }
            if queue.commands.len() == CAPACITY {
                return Err(());
            }
        }
        if let Some(key) = parameter_key {
            let index = queue.commands.len();
            queue.parameters.insert(key, index);
        } else {
            queue.parameters.clear();
        }
        queue.commands.push_back(command);
        Ok(())
    }
    pub fn take_batch(&self, batch: &mut Vec<AudioCommand>) {
        // Never wait on a producer. Keep rendering and retry next callback.
        let Ok(mut queue) = self.0.try_lock() else {
            return;
        };
        if self.2.swap(false, Ordering::AcqRel) {
            batch.push(AudioCommand::StopAll);
        }
        let mut drained = 0;
        for _ in batch.len()..COMMANDS_PER_CALLBACK {
            let Some(command) = queue.commands.pop_front() else {
                break;
            };
            batch.push(command);
            drained += 1;
        }
        queue.parameters.retain(|_, index| {
            if *index < drained {
                false
            } else {
                *index -= drained;
                true
            }
        });
    }
}

impl AudioCommand {
    fn is_release(&self) -> bool {
        matches!(
            self,
            Self::Stop { .. }
                | Self::FadeOut { .. }
                | Self::StopAll
                | Self::PauseAll
                | Self::RemoveEffectBus { .. }
        ) || self.is_stream_release()
    }
    fn is_stream_release(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            matches!(self, Self::StopStream { .. })
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }
    fn parameter_key(&self) -> Option<(u8, u64)> {
        Some(match self {
            Self::SetVolume { id, .. } => (0, *id),
            Self::SetPan { id, .. } => (1, *id),
            Self::SetPlaybackRate { id, .. } => (2, *id),
            Self::SetSoundPosition { id, .. } => (3, *id),
            Self::SetSoundVelocity { id, .. } => (4, *id),
            Self::SetSoundOcclusion { id, .. } => (5, *id),
            Self::SetSoundCone { id, .. } => (6, *id),
            Self::SetEffectBusMix { id, .. } => (7, *id),
            Self::SetListenerPosition { .. } => (8, 0),
            Self::SetListenerVelocity { .. } => (9, 0),
            Self::SetListenerForward { .. } => (10, 0),
            Self::SetSpatialParams { .. } => (11, 0),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn critical_overload_stops_instead_of_losing_note_offs() {
        let tx = CommandSender::new();
        for id in 0..CAPACITY {
            tx.send(AudioCommand::FadeOut {
                id: id as u64,
                duration: 0.1,
            })
            .unwrap();
        }
        tx.send(AudioCommand::Stop { id: 999 }).unwrap();
        assert!(tx.0.lock().unwrap().commands.len() <= CAPACITY);
        let mut batch = Vec::with_capacity(COMMANDS_PER_CALLBACK);
        tx.take_batch(&mut batch);
        assert!(matches!(batch[0], AudioCommand::StopAll));
    }
    #[test]
    fn contended_producer_does_not_block_audio() {
        let tx = CommandSender::new();
        let _guard = tx.0.lock().unwrap();
        let mut batch = Vec::with_capacity(COMMANDS_PER_CALLBACK);
        tx.take_batch(&mut batch);
        assert!(batch.is_empty());
    }
    #[test]
    fn updates_coalesce_but_do_not_cross_fades() {
        let tx = CommandSender::new();
        for volume in 0..1000 {
            tx.send(AudioCommand::SetVolume {
                id: 1,
                volume: volume as f32,
            })
            .unwrap();
        }
        tx.send(AudioCommand::FadeOut {
            id: 1,
            duration: 0.1,
        })
        .unwrap();
        tx.send(AudioCommand::SetVolume { id: 1, volume: 0.5 })
            .unwrap();
        let mut batch = Vec::with_capacity(COMMANDS_PER_CALLBACK);
        tx.take_batch(&mut batch);
        assert_eq!(batch.len(), 3);
        assert!(matches!(
            batch[0],
            AudioCommand::SetVolume { volume: 999.0, .. }
        ));
        assert!(matches!(batch[1], AudioCommand::FadeOut { .. }));
    }
    #[test]
    fn indexed_updates_match_fifo_after_partial_drains_and_wraparound() {
        let tx = CommandSender::new();
        let mut reference: VecDeque<(u64, f32)> = VecDeque::new();
        for round in 0..20 {
            for id in 1..=150 {
                let pan = (round as f32 + id as f32) / 200.0;
                tx.send(AudioCommand::SetPan { id, pan }).unwrap();
                if let Some(entry) = reference.iter_mut().find(|entry| entry.0 == id) {
                    entry.1 = pan;
                } else {
                    reference.push_back((id, pan));
                }
            }
            let mut batch = Vec::new();
            tx.take_batch(&mut batch);
            assert_eq!(batch.len(), COMMANDS_PER_CALLBACK);
            for actual in batch {
                let expected = reference.pop_front().unwrap();
                match actual {
                    AudioCommand::SetPan { id, pan } => assert_eq!((id, pan), expected),
                    _ => panic!("unexpected command"),
                }
            }
        }
    }

    #[test]
    fn release_capacity_is_reserved_and_drain_is_bounded() {
        let tx = CommandSender::new();
        for id in 0..NORMAL_CAPACITY {
            tx.send(AudioCommand::Resume { id: id as u64 }).unwrap();
        }
        assert!(tx.send(AudioCommand::Resume { id: 999 }).is_err());
        tx.send(AudioCommand::FadeOut {
            id: 1,
            duration: 0.1,
        })
        .unwrap();
        tx.send(AudioCommand::StopAll).unwrap();
        let mut batch = Vec::with_capacity(COMMANDS_PER_CALLBACK);
        tx.take_batch(&mut batch);
        assert_eq!(batch.len(), COMMANDS_PER_CALLBACK);
        assert_eq!(
            tx.0.lock().unwrap().commands.len(),
            NORMAL_CAPACITY + 2 - COMMANDS_PER_CALLBACK
        );
    }
}
