//! Device-independent renderer for dedicated workers and offline verification.
use super::{
    BusEffects, VoicePriority,
    callback::{AudioCallbackState, handle_command, mix_sounds},
    command_queue::{COMMANDS_PER_CALLBACK, CommandSender},
    commands::{AudioCommand, SoundId},
    output_limiter::OutputLimiter,
    playing_states::PlayingStates,
    source::SoundSource,
    voice_budget::VoiceOptions,
};
use crate::{synthesis::Sample, track::Track};
use std::sync::Arc;

/// Runs the same mixer, voice budget and controls without opening an audio device.
/// The owner must call `render` regularly and `collect_garbage` between render jobs.
/// Intended for a dedicated DSP worker, not an AudioWorklet's process callback.
pub struct AudioRenderer {
    state: AudioCallbackState,
    sender: CommandSender,
    playing: Arc<PlayingStates>,
    batch: Vec<AudioCommand>,
    limiter: OutputLimiter,
    rate: f32,
    frames: usize,
}
impl AudioRenderer {
    pub fn new(sample_rate: f32, frames: usize) -> Self {
        assert!(sample_rate.is_finite() && (8000.0..=192000.0).contains(&sample_rate));
        assert!((1..=8192).contains(&frames));
        let playing = Arc::new(PlayingStates::new());
        let mut state = AudioCallbackState::new();
        state.temp_buffer.resize(frames * 32, 0.0);
        Self {
            state,
            sender: CommandSender::with_states(playing.clone()),
            playing,
            batch: Vec::with_capacity(COMMANDS_PER_CALLBACK),
            limiter: OutputLimiter::new(),
            rate: sample_rate,
            frames,
        }
    }
    /// Change the render quantum between worker jobs without losing voices or effects.
    pub fn set_block_frames(&mut self, frames: usize) {
        assert!((1..=8192).contains(&frames));
        self.state.temp_buffer.resize(frames * 32, 0.0);
        self.frames = frames;
    }
    fn play(
        &self,
        id: SoundId,
        source: SoundSource,
        bus: Option<u64>,
        priority: VoicePriority,
    ) -> bool {
        if id == 0 || !self.playing.insert(id, ()) {
            return false;
        }
        if self
            .sender
            .send(AudioCommand::PlaySource {
                id,
                source: Box::new(source),
                bus,
                options: VoiceOptions {
                    priority,
                    ..Default::default()
                },
            })
            .is_err()
        {
            self.playing.remove(&id);
            return false;
        }
        true
    }
    pub fn play_track(
        &self,
        id: SoundId,
        track: Track,
        bus: Option<u64>,
        priority: VoicePriority,
    ) -> bool {
        self.play(
            id,
            SoundSource::track(track, self.frames * 16),
            bus,
            priority,
        )
    }
    pub fn play_sample(
        &self,
        id: SoundId,
        sample: Sample,
        speed: f32,
        volume: f32,
        pan: f32,
    ) -> bool {
        if !speed.is_finite() || !(0.1..=4.0).contains(&speed) {
            return false;
        }
        self.play(
            id,
            SoundSource::sample(sample, speed, volume.clamp(0.0, 2.0), pan.clamp(-1.0, 1.0)),
            None,
            VoicePriority::Normal,
        )
    }
    #[cfg(feature = "worker")]
    pub fn submit_wire(
        &self,
        json: &str,
        samples: &std::collections::HashMap<String, Arc<Vec<f32>>>,
    ) -> Result<(), String> {
        if json.len() > 16 * 1024 * 1024 {
            return Err("audio command too large".into());
        }
        let wire: super::worker_protocol::WireCommand =
            serde_json::from_str(json).map_err(|e| e.to_string())?;
        let command = wire.decode(samples, self.frames)?;
        let id = match &command {
            AudioCommand::Play { id, .. } | AudioCommand::PlaySource { id, .. } => Some(*id),
            _ => None,
        };
        if let Some(id) = id {
            if id == 0 || !self.playing.insert(id, ()) {
                return Err("audio voice ID unavailable".into());
            }
        }
        if self.sender.send(command).is_err() {
            if let Some(id) = id {
                self.playing.remove(&id);
            }
            return Err("audio command queue full".into());
        }
        Ok(())
    }
    #[cfg(feature = "worker")]
    pub fn playing_ids(&self) -> Vec<String> {
        self.playing.ids()
    }

    pub fn set_bus(&self, id: u64, effects: BusEffects) -> bool {
        self.sender
            .send(AudioCommand::SetEffectBus {
                id,
                bus: Box::new(super::effect_bus::EffectBusState::new(effects, self.frames)),
            })
            .is_ok()
    }
    pub fn release(&self, id: SoundId, seconds: f32) -> bool {
        self.sender
            .send(AudioCommand::FadeOut {
                id,
                duration: seconds,
            })
            .is_ok()
    }
    pub fn stop_all(&self) -> bool {
        self.sender.send(AudioCommand::StopAll).is_ok()
    }
    pub fn is_playing(&self, id: SoundId) -> bool {
        self.playing.contains_key(&id)
    }
    pub fn render(&mut self, output: &mut [f32]) {
        assert_eq!(output.len(), self.frames * 2);
        if !self.state.active_sounds.retirement_available()
            || !self.state.effect_buses.retirement_available()
        {
            output.fill(0.0);
            return;
        }
        self.sender.take_batch(&mut self.batch);
        let AudioCallbackState {
            active_sounds,
            effect_buses,
            listener,
            spatial,
            temp_buffer,
            finished_sounds,
            #[cfg(not(target_arch = "wasm32"))]
            streaming_sounds,
            ..
        } = &mut self.state;
        for command in self.batch.drain(..) {
            handle_command(
                command,
                effect_buses,
                active_sounds,
                #[cfg(not(target_arch = "wasm32"))]
                streaming_sounds,
                listener,
                spatial,
                self.rate,
                &self.playing,
            );
        }
        mix_sounds(
            output,
            effect_buses,
            active_sounds,
            temp_buffer,
            finished_sounds,
            listener,
            spatial,
            self.rate,
            2,
        );
        for id in finished_sounds {
            self.playing.remove(id);
        }
        self.limiter.process(output, 2, self.rate);
    }
    /// Call on the worker between render jobs, never in AudioWorklet.process.
    pub fn collect_garbage(&self) {
        while self.state.active_sounds.retired.pop().is_some() {}
        while self.state.effect_buses.retired.pop().is_some() {}
    }
}

impl Drop for AudioRenderer {
    fn drop(&mut self) {
        self.sender.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resizing_preserves_sample_position_and_output() {
        let sample = Sample::from_mono(
            (0..48000).map(|i| (i as f32 * 0.01).sin() * 0.1).collect(),
            48000,
        );
        let mut changing = AudioRenderer::new(48000.0, 512);
        let mut reference = AudioRenderer::new(48000.0, 512);
        assert!(changing.play_sample(1, sample.clone(), 1.0, 1.0, 0.0));
        assert!(reference.play_sample(1, sample, 1.0, 1.0, 0.0));
        for frames in [512, 8192, 1024, 4096, 2048, 512] {
            changing.set_block_frames(frames);
            let mut output = vec![0.0; frames * 2];
            changing.render(&mut output);
            let mut expected = vec![0.0; frames * 2];
            for chunk in expected.chunks_mut(1024) {
                reference.render(chunk);
            }
            let max_error = output
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                max_error < 0.0001,
                "sample mismatch at {frames} frames: {max_error}"
            );
            assert!(changing.is_playing(1));
        }
    }
    #[test]
    fn resizing_keeps_synth_and_effect_bus_alive() {
        use crate::synthesis::{Delay, Envelope, Waveform};
        let mut renderer = AudioRenderer::new(48000.0, 512);
        let mut track = Track::new();
        track.add_note_with_waveform_and_envelope(
            &[220.0],
            0.0,
            10.0,
            Waveform::Sine,
            Envelope::new(0.01, 0.1, 0.5, 0.1),
        );
        assert!(renderer.set_bus(
            1,
            BusEffects {
                delay: Some(Delay::with_sample_rate(0.01, 0.5, 0.3, 48000.0)),
                reverb: None
            }
        ));
        assert!(renderer.play_track(1, track, Some(1), VoicePriority::Normal));
        for frames in [512, 8192, 1024, 4096, 2048, 512] {
            renderer.set_block_frames(frames);
            let mut output = vec![0.0; frames * 2];
            renderer.render(&mut output);
            assert!(output.iter().all(|v| v.is_finite()));
            assert!(output.iter().any(|v| v.abs() > 0.001));
            assert!(renderer.is_playing(1));
        }
    }
    #[test]
    fn worker_renderer_plays_and_retires_without_opening_a_device() {
        let mut renderer = AudioRenderer::new(44100.0, 512);
        let sample = Sample::from_mono(vec![0.25; 44100], 44100);
        assert!(renderer.play_sample(1, sample, 1.0, 1.0, 0.0));
        let mut output = vec![0.0; 1024];
        renderer.render(&mut output);
        assert!(output.iter().any(|v| *v > 0.0));
        assert!(renderer.release(1, 0.001));
        renderer.render(&mut output);
        assert!(!renderer.is_playing(1));
        renderer.collect_garbage();
        renderer.render(&mut output);
        assert!(output.iter().all(|v| *v == 0.0));
    }
}
