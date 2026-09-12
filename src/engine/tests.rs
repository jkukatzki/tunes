//! Offline callback regressions: no audio device or game window required.

use super::active_sound::ActiveSound;
use super::callback::{handle_command, mix_sounds, AudioCallbackState};
use super::commands::AudioCommand;
use crate::composition::{Composition, Tempo};
use crate::synthesis::spatial::{ListenerConfig, SpatialParams};
use crate::synthesis::Sample;
use crossbeam::epoch::Atomic;
use dashmap::DashMap;
use std::sync::Arc;

const RATE: f32 = 1024.0;

struct Callback {
    state: AudioCallbackState,
    listener: Arc<Atomic<ListenerConfig>>,
    spatial: Arc<Atomic<SpatialParams>>,
    playing: DashMap<u64, ()>,
}

impl Callback {
    fn new(looping: bool) -> Self {
        let mut comp = Composition::new(Tempo::new(120.0));
        let sample = Sample::from_mono(vec![0.5; 4096], RATE as u32);
        comp.track("constant").play_sample(&sample, 1.0);
        let mut mixer = comp.into_mixer();
        mixer.prepare_realtime(512);
        let mut state = AudioCallbackState::new();
        state
            .active_sounds
            .insert(1, ActiveSound::new(mixer, looping));
        Self {
            state,
            listener: Arc::new(Atomic::new(ListenerConfig::default())),
            spatial: Arc::new(Atomic::new(SpatialParams::default())),
            playing: DashMap::new(),
        }
    }

    fn command(&mut self, cmd: AudioCommand) {
        handle_command(
            cmd,
            &mut self.state.active_sounds,
            #[cfg(not(target_arch = "wasm32"))]
            &mut self.state.streaming_sounds,
            &self.listener,
            &self.spatial,
            RATE,
            &self.playing,
        );
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames * 2];
        mix_sounds(
            &mut output,
            &mut self.state.active_sounds,
            &mut self.state.temp_buffer,
            &mut self.state.finished_sounds,
            &ListenerConfig::default(),
            &SpatialParams::default(),
            RATE,
            2,
        );
        output
    }
}

#[test]
fn shortening_release_starts_at_current_gain_and_retires_voice() {
    let mut callback = Callback::new(false);
    let baseline = callback.render(1)[0];
    callback.command(AudioCommand::FadeOut {
        id: 1,
        duration: 0.5,
    });
    callback.render(256);
    callback.command(AudioCommand::FadeOut {
        id: 1,
        duration: 0.125,
    });
    let tail = callback.render(128);
    assert!((tail[0] / baseline - 0.5).abs() < 1e-5);
    assert!(tail
        .chunks_exact(2)
        .map(|s| s[0])
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[1] <= w[0]));
    assert!(callback.state.active_sounds.get_mut(1).is_none());
    assert_eq!(callback.state.finished_sounds, [1]);
    assert!(callback.render(64).iter().all(|&s| s == 0.0));
}

#[test]
fn fade_duration_uses_output_time_at_different_playback_rates() {
    for rate in [0.5, 1.0, 2.0] {
        let mut callback = Callback::new(true);
        // Exercise a loop boundary while the fade is running.
        callback
            .state
            .active_sounds
            .get_mut(1)
            .unwrap()
            .elapsed_time = 4.0;
        callback.command(AudioCommand::SetPlaybackRate { id: 1, rate });
        callback.command(AudioCommand::FadeOut {
            id: 1,
            duration: 0.125,
        });
        callback.render(64);
        let sound = callback.state.active_sounds.get_mut(1).unwrap();
        assert!((sound.volume - 0.5).abs() < 1e-5);
        callback.render(64);
        assert!(callback.state.active_sounds.get_mut(1).is_none());
    }
}

#[test]
fn pause_freezes_fade_and_repeated_release_does_not_extend_it() {
    let mut callback = Callback::new(false);
    callback.command(AudioCommand::FadeOut {
        id: 1,
        duration: 0.125,
    });
    callback.render(64);
    callback.command(AudioCommand::Pause { id: 1 });
    assert!(callback.render(512).iter().all(|&s| s == 0.0));
    callback.command(AudioCommand::Resume { id: 1 });
    callback.command(AudioCommand::FadeOut {
        id: 1,
        duration: 0.5,
    });
    callback.render(64);
    assert!(callback.state.active_sounds.get_mut(1).is_none());
}

#[test]
fn fade_in_to_zero_does_not_stop_and_zero_duration_fade_out_does() {
    let mut callback = Callback::new(false);
    callback.command(AudioCommand::FadeIn {
        id: 1,
        duration: 0.125,
        target_volume: 0.0,
    });
    callback.render(128);
    assert!(callback.state.active_sounds.get_mut(1).is_some());
    callback.command(AudioCommand::FadeOut {
        id: 1,
        duration: 0.0,
    });
    assert!(callback.render(64).iter().all(|&s| s == 0.0));
    assert!(callback.state.active_sounds.get_mut(1).is_none());
}
