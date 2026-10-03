//! Device-independent renderer for dedicated workers and offline verification.
use super::{
    callback::{handle_command, mix_sounds, AudioCallbackState},
    command_queue::{CommandSender, COMMANDS_PER_CALLBACK},
    commands::{AudioCommand, SoundId},
    output_limiter::OutputLimiter,
    playing_states::PlayingStates,
    source::SoundSource,
    voice_budget::VoiceOptions,
    BusEffects, VoicePriority,
};
use crate::{synthesis::Sample, track::Track};
use std::sync::Arc;

/// Runs the same mixer, voice budget and controls without opening an audio device.
/// The owner must call `render` regularly and `collect_garbage` between render jobs.
/// Direct AudioWorklet hosts should use bounded maintenance; command preparation
/// and individual destructors can still allocate or take unbounded time.
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

    /// Capture transport state for selected sample voices, excluding effect history.
    #[cfg(feature = "worker")]
    pub fn playback_snapshot(&mut self, ids: &[String]) -> Result<String, String> {
        let states: Vec<_> = self
            .state
            .active_sounds
            .iter_mut()
            .filter(|(id, _)| ids.iter().any(|v| v == &id.to_string()))
            .map(|(id, v)| {
                (
                    id.to_string(),
                    PlaybackState {
                        elapsed_time: v.elapsed_time,
                        sample_clock: v.sample_clock,
                        control_time: v.control_time,
                        volume: v.volume,
                        pan: v.pan,
                        playback_rate: v.playback_rate,
                        paused: v.paused,
                        fade_start_time: v.fade_start_time,
                        fade_duration: v.fade_duration,
                        fade_start_volume: v.fade_start_volume,
                        fade_target_volume: v.fade_target_volume,
                        stop_after_fade: v.stop_after_fade,
                        pan_tween_start_time: v.pan_tween_start_time,
                        pan_tween_duration: v.pan_tween_duration,
                        pan_tween_start_value: v.pan_tween_start_value,
                        pan_tween_target_value: v.pan_tween_target_value,
                        rate_tween_start_time: v.rate_tween_start_time,
                        rate_tween_duration: v.rate_tween_duration,
                        rate_tween_start_value: v.rate_tween_start_value,
                        rate_tween_target_value: v.rate_tween_target_value,
                    },
                )
            })
            .collect();
        serde_json::to_string(&states).map_err(|e| e.to_string())
    }
    /// Restore transport state on already prepared sample voices.
    #[cfg(feature = "worker")]
    pub fn restore_playback(&mut self, json: &str) -> Result<(), String> {
        if json.len() > 1024 * 1024 {
            return Err("playback snapshot too large".into());
        }
        let states: Vec<(String, PlaybackState)> =
            serde_json::from_str(json).map_err(|e| e.to_string())?;
        for (id, state) in states {
            let id = id.parse::<u64>().map_err(|e| e.to_string())?;
            if let Some(v) = self.state.active_sounds.get_mut(id) {
                v.elapsed_time = state.elapsed_time;
                v.sample_clock = state.sample_clock;
                v.control_time = state.control_time;
                v.volume = state.volume;
                v.pan = state.pan;
                v.playback_rate = state.playback_rate;
                v.paused = state.paused;
                v.fade_start_time = state.fade_start_time;
                v.fade_duration = state.fade_duration;
                v.fade_start_volume = state.fade_start_volume;
                v.fade_target_volume = state.fade_target_volume;
                v.stop_after_fade = state.stop_after_fade;
                v.pan_tween_start_time = state.pan_tween_start_time;
                v.pan_tween_duration = state.pan_tween_duration;
                v.pan_tween_start_value = state.pan_tween_start_value;
                v.pan_tween_target_value = state.pan_tween_target_value;
                v.rate_tween_start_time = state.rate_tween_start_time;
                v.rate_tween_duration = state.rate_tween_duration;
                v.rate_tween_start_value = state.rate_tween_start_value;
                v.rate_tween_target_value = state.rate_tween_target_value;
            }
        }
        Ok(())
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
        self.flush_commands();
        let AudioCallbackState {
            active_sounds,
            effect_buses,
            listener,
            spatial,
            temp_buffer,
            finished_sounds,
            ..
        } = &mut self.state;
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
    /// Apply one bounded command batch without advancing playback (context handoff).
    pub fn flush_commands(&mut self) {
        if !self.state.active_sounds.retirement_available()
            || !self.state.effect_buses.retirement_available()
        {
            return;
        }
        self.sender.take_batch(&mut self.batch);
        let AudioCallbackState {
            active_sounds,
            effect_buses,
            listener,
            spatial,
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
    }
    /// Destroy at most `per_pool` retired voices and `per_pool` retired buses.
    /// Separate budgets prevent a busy voice queue from starving effect cleanup.
    /// Returns the number destroyed. This bounds object count, not destructor time:
    /// dropping one composition or last sample reference can still be expensive.
    pub fn collect_garbage_budget(&self, per_pool: usize) -> usize {
        let mut collected = 0;
        for _ in 0..per_pool {
            let voice = self.state.active_sounds.retired.pop();
            let bus = self.state.effect_buses.retired.pop();
            if voice.is_none() && bus.is_none() {
                break;
            }
            collected += usize::from(voice.is_some()) + usize::from(bus.is_some());
            drop(voice);
            drop(bus);
        }
        collected
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
    fn bounded_cleanup_preserves_audio_and_eventually_drains_both_pools() {
        let mut renderer = AudioRenderer::new(48000.0, 128);
        let mut output = vec![0.0; 256];
        let sample = Sample::from_mono(vec![0.1; 48000], 48000);
        for id in 1..=24 {
            assert!(renderer.play_sample(id, sample.clone(), 1.0, 1.0, 0.0));
        }
        renderer.render(&mut output);
        assert!(renderer.stop_all());
        renderer.render(&mut output);
        // Queue effect replacements independently of voice retirement.
        for _ in 0..12 {
            assert!(renderer.set_bus(100, BusEffects::default()));
            renderer.render(&mut output);
        }
        let voices = renderer.state.active_sounds.retired.len();
        let buses = renderer.state.effect_buses.retired.len();
        assert!(voices >= 24 && buses >= 11);
        assert_eq!(renderer.collect_garbage_budget(0), 0);
        assert_eq!(renderer.collect_garbage_budget(4), 8);
        assert_eq!(renderer.state.active_sounds.retired.len(), voices - 4);
        assert_eq!(renderer.state.effect_buses.retired.len(), buses - 4);
        let mut total = 8;
        for _ in 0..32 {
            let collected = renderer.collect_garbage_budget(4);
            assert!(collected <= 8);
            total += collected;
        }
        assert_eq!(total, voices + buses);
        assert_eq!(renderer.collect_garbage_budget(4), 0);
        renderer.render(&mut output);
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[cfg(feature = "worker")]
    #[test]
    fn handoff_preserves_sample_cursor_pause_loop_and_control_timeline() {
        let sample = Sample::from_mono(
            (0..4800).map(|i| (i as f32 * 0.01).sin() * 0.1).collect(),
            48000,
        );
        for wrapped in [0, 1, 2] {
            let mut old = AudioRenderer::new(48000.0, 128);
            let mut next = AudioRenderer::new(48000.0, 128);
            for renderer in [&mut old, &mut next] {
                if wrapped != 0 {
                    let mut track = Track::new();
                    track.events.push(crate::track::AudioEvent::Sample(
                        crate::track::SampleEvent::new(sample.clone(), 0.0),
                    ));
                    if wrapped == 1 {
                        assert!(renderer.play_track(1, track, None, VoicePriority::Normal));
                    } else {
                        let mut mixer =
                            crate::track::Mixer::new(crate::composition::timing::Tempo::new(120.0));
                        mixer.add_track(track);
                        mixer.prepare_realtime(2048);
                        assert!(renderer.play(
                            1,
                            SoundSource::Mixer(Box::new(mixer)),
                            None,
                            VoicePriority::Normal
                        ));
                    }
                } else {
                    assert!(renderer.play_sample(1, sample.clone(), 1.0, 1.0, 0.0));
                }
                renderer.flush_commands();
                let voice = renderer.state.active_sounds.get_mut(1).unwrap();
                voice.looping = true;
                voice.playback_rate = 1.5;
            }
            let mut expected = vec![0.0; 256];
            let mut actual = vec![0.0; 256];
            for _ in 0..60 {
                old.render(&mut expected);
            }
            let voice = old.state.active_sounds.get_mut(1).unwrap();
            voice.start_fade(1.0, 0.3, false);
            voice.paused = true;
            let snapshot = old.playback_snapshot(&["1".into()]).unwrap();
            next.restore_playback(&snapshot).unwrap();
            old.render(&mut expected);
            next.render(&mut actual);
            assert_eq!(actual, expected);
            assert!(actual.iter().all(|v| *v == 0.0));
            old.state.active_sounds.get_mut(1).unwrap().paused = false;
            next.state.active_sounds.get_mut(1).unwrap().paused = false;
            for _ in 0..40 {
                old.render(&mut expected);
                next.render(&mut actual);
                assert_eq!(
                    actual, expected,
                    "sample transport diverged (wrapped={wrapped})"
                );
            }
        }
    }

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

#[cfg(feature = "worker")]
#[derive(serde::Serialize, serde::Deserialize)]
struct PlaybackState {
    elapsed_time: f32,
    sample_clock: f32,
    control_time: f64,
    volume: f32,
    pan: f32,
    playback_rate: f32,
    paused: bool,
    fade_start_time: Option<f64>,
    fade_duration: f32,
    fade_start_volume: f32,
    fade_target_volume: f32,
    stop_after_fade: bool,
    pan_tween_start_time: Option<f64>,
    pan_tween_duration: f32,
    pan_tween_start_value: f32,
    pan_tween_target_value: f32,
    rate_tween_start_time: Option<f64>,
    rate_tween_duration: f32,
    rate_tween_start_value: f32,
    rate_tween_target_value: f32,
}
