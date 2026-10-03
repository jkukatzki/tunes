//! Persistent shared effects after voice envelopes/panning and before the output limiter.
use super::command_queue::CommandSender;
use super::{commands::AudioCommand, AudioEngine};
use crate::{
    error::{Result, TunesError},
    synthesis::effects::{Delay, EffectChain, Reverb},
};

/// Linear time-based effects shared by an instrument's voices.
#[derive(Default, Clone)]
pub struct BusEffects {
    pub delay: Option<Delay>,
    pub reverb: Option<Reverb>,
}

/// Owned route for a group of voices. Dropping it removes the route and its tails.
/// Keep this alongside the instrument, rather than constructing one for every note.
pub struct EffectBus {
    pub(crate) id: u64,
    pub(crate) slots: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) sender: CommandSender,
    pub(crate) frames: usize,
}

impl EffectBus {
    pub fn belongs_to(&self, engine: &AudioEngine) -> bool {
        self.sender.same_channel(&engine.command_tx)
    }

    /// Replace the shared effect configuration. Existing tails are reset.
    /// Preparing/cloning processors happens on the caller, not the audio callback.
    pub fn set_effects(&self, effects: BusEffects) -> Result<()> {
        self.sender
            .send(AudioCommand::SetEffectBus {
                id: self.id,
                bus: Box::new(EffectBusState::new(effects, self.frames)),
            })
            .map_err(|_| {
                TunesError::AudioEngineError("Audio command queue full or unavailable".into())
            })
    }

    /// Change wet/dry controls without rebuilding delay lines or losing their tails.
    pub fn set_mix(&self, delay: f32, reverb: f32) -> Result<()> {
        self.sender
            .send(AudioCommand::SetEffectBusMix {
                id: self.id,
                delay,
                reverb,
            })
            .map_err(|_| {
                TunesError::AudioEngineError("Audio command queue full or unavailable".into())
            })
    }
}

impl Drop for EffectBus {
    fn drop(&mut self) {
        let _ = self
            .sender
            .send(AudioCommand::RemoveEffectBus { id: self.id });
        self.slots.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

pub(crate) struct EffectBusState {
    pub(crate) input: Vec<f32>,
    pub(super) effects: [EffectChain; 2],
    mono: [Vec<f32>; 2],
    sample_count: u64,
    pub(crate) paused: bool,
}

impl EffectBusState {
    pub(crate) fn new(config: BusEffects, frames: usize) -> Self {
        let mut effects = EffectChain::new();
        effects.delay = config.delay;
        effects.reverb = config.reverb;
        effects.compute_effect_order();
        Self {
            input: vec![0.0; frames * 2],
            effects: [effects.clone(), effects],
            mono: [vec![0.0; frames], vec![0.0; frames]],
            sample_count: 0,
            paused: false,
        }
    }

    pub(crate) fn set_mix(&mut self, delay: f32, reverb: f32) {
        for effects in &mut self.effects {
            if let Some(d) = &mut effects.delay {
                d.mix = delay.clamp(0.0, 1.0);
            }
            if let Some(r) = &mut effects.reverb {
                r.mix = reverb.clamp(0.0, 1.0);
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        for effects in &mut self.effects {
            if let Some(d) = &mut effects.delay {
                d.reset();
            }
            if let Some(r) = &mut effects.reverb {
                r.reset();
            }
        }
        self.input.fill(0.0);
    }

    pub(crate) fn process_add(&mut self, output: &mut [f32], channels: usize, rate: f32) {
        if self.paused {
            return;
        }
        let frames = output.len() / channels;
        if self.effects[0].effect_order.is_empty() {
            for (out, input) in output.iter_mut().zip(&self.input) {
                *out += input;
            }
            self.sample_count = self.sample_count.wrapping_add(frames as u64);
            return;
        }
        // Independent channel histories avoid feeding the right channel through
        // the left channel's delay/reverb state. Zero input still renders tails.
        for channel in 0..channels.min(2) {
            let mono = &mut self.mono[channel];
            mono.resize(frames, 0.0);
            for (i, sample) in mono.iter_mut().enumerate() {
                *sample = self.input[i * channels + channel];
            }
            self.effects[channel].process_mono_block(
                mono,
                rate,
                self.sample_count as f32 / rate,
                self.sample_count,
            );
            for (i, sample) in mono.iter().enumerate() {
                output[i * channels + channel] += sample;
            }
        }
        self.sample_count = self.sample_count.wrapping_add(frames as u64);
    }
}
