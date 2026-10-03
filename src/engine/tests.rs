//! Offline callback regressions: no audio device or game window required.

use super::active_sound::ActiveSound;
use super::callback::{handle_command, mix_sounds, AudioCallbackState};
use super::commands::AudioCommand;
use crate::composition::{Composition, Tempo};
use crate::synthesis::spatial::{ListenerConfig, SpatialParams};
use crate::synthesis::Sample;

const RATE: f32 = 1024.0;

fn assert_audio_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (&a, &b)) in actual.iter().zip(expected).enumerate() {
        assert!((a - b).abs() < 2e-5, "frame {index}: {a} != {b}");
    }
}

#[test]
fn direct_sample_matches_composition_at_fractional_speed_and_pan() {
    use super::source::SoundSource;
    let sample = Sample::from_mono(
        (0..4096).map(|i| (i as f32 * 0.03).sin() * 0.3).collect(),
        RATE as u32,
    );
    for speed in [0.7, 1.0, 1.9] {
        for pan in [-0.6, 0.0, 0.8] {
            let mut comp = Composition::new(Tempo::new(120.0));
            comp.track("sample")
                .volume(0.65)
                .pan(pan)
                .play_sample(&sample, speed);
            let mut reference = comp.into_mixer();
            reference.prepare_realtime(127);
            let mut direct = SoundSource::sample(sample.clone(), speed, 0.65, pan);
            for block in 0..12 {
                let time = block as f32 * 127.0 / RATE;
                let mut a = [0.0; 254];
                let mut b = [0.0; 254];
                direct.process_block(&mut a, RATE, time, None, None);
                reference.process_block(&mut b, RATE, time, None, None);
                assert_audio_close(&a, &b);
            }
        }
    }
}

#[test]
fn sample_blocks_retain_single_and_final_frames_at_any_simd_alignment() {
    for length in [1, 2, 3, 4, 5, 7, 8, 9, 16] {
        let input: Vec<f32> = (0..length).map(|i| (i + 1) as f32 / 32.0).collect();
        let sample = Sample::from_mono(input.clone(), RATE as u32);
        let mut actual = vec![0.0; length + 8];
        sample.fill_buffer_simd_mono(&mut actual, 0.0, 0.0, 1.0 / RATE, 1.0, 1.0);
        assert_audio_close(&actual[..length], &input);
        assert!(actual[length..].iter().all(|&value| value == 0.0));
    }
}

#[test]
fn changing_bus_mix_preserves_an_existing_delay_tail() {
    use super::effect_bus::{BusEffects, EffectBusState};
    use crate::synthesis::effects::Delay;
    let mut bus = EffectBusState::new(
        BusEffects {
            delay: Some(Delay::with_sample_rate(0.125, 0.5, 1.0, RATE)),
            reverb: None,
        },
        64,
    );
    bus.input[0] = 1.0;
    bus.process_add(&mut [0.0; 128], 2, RATE);
    bus.input.fill(0.0);
    bus.set_mix(0.5, 0.0);
    bus.process_add(&mut [0.0; 128], 2, RATE);
    let mut output = [0.0; 128];
    bus.process_add(&mut output, 2, RATE);
    assert!((output[0] - 0.5).abs() < 1e-6);
}

#[test]
fn direct_track_matches_composition_with_voice_filter_and_distortion() {
    use super::source::SoundSource;
    use crate::{
        synthesis::{effects::Distortion, Envelope, Filter, Waveform},
        track::Track,
    };
    let mut track = Track::new();
    track.volume = 0.45;
    track.pan = 0.3;
    track.filter = Filter::low_pass(200.0, 0.5);
    track.effects.distortion = Some(Distortion::new(0.4, 0.3));
    track.add_note_with_waveform_and_envelope(
        &[80.0],
        0.0,
        1.0,
        Waveform::Sine,
        Envelope::default(),
    );
    let mut comp = Composition::new(Tempo::new(120.0));
    comp.track("note")
        .volume(0.45)
        .pan(0.3)
        .filter(Filter::low_pass(200.0, 0.5))
        .distortion(Distortion::new(0.4, 0.3))
        .waveform(Waveform::Sine)
        .envelope(Envelope::default())
        .note(&[80.0], 1.0);
    let mut reference = comp.into_mixer();
    reference.prepare_realtime(64);
    let mut direct = SoundSource::track(track, 64);
    for block in 0..20 {
        let mut a = [0.0; 128];
        let mut b = [0.0; 128];
        let time = block as f32 * 64.0 / RATE;
        direct.process_block(&mut a, RATE, time, None, None);
        reference.process_block(&mut b, RATE, time, None, None);
        assert_audio_close(&a, &b);
    }
}

#[test]
fn shared_bus_keeps_tails_after_voice_end_and_obeys_global_controls() {
    use super::{
        effect_bus::{BusEffects, EffectBusState},
        source::SoundSource,
    };
    use crate::synthesis::effects::Delay;
    let mut callback = Callback::new(false);
    callback.command(AudioCommand::StopAll);
    callback.command(AudioCommand::SetEffectBus {
        id: 20,
        bus: Box::new(EffectBusState::new(
            BusEffects {
                delay: Some(Delay::with_sample_rate(0.125, 0.5, 1.0, RATE)),
                reverb: None,
            },
            64,
        )),
    });
    let sample = Sample::from_mono(vec![0.5; 16], RATE as u32);
    for id in [2, 3] {
        callback.command(AudioCommand::PlaySource {
            options: Default::default(),
            id,
            source: Box::new(SoundSource::sample(sample.clone(), 1.0, 1.0, 0.0)),
            bus: Some(20),
        });
    }
    assert!(callback.render(64).iter().all(|&v| v == 0.0));
    assert!(callback.render(64).iter().all(|&v| v == 0.0));
    assert!(callback.state.active_sounds.get_mut(2).is_none());
    callback.command(AudioCommand::PauseAll);
    assert!(callback.render(64).iter().all(|&v| v == 0.0));
    callback.command(AudioCommand::ResumeAll);
    assert!(callback.render(64).iter().any(|v| v.abs() > 0.1));
    callback.command(AudioCommand::StopAll);
    for _ in 0..8 {
        assert!(callback.render(64).iter().all(|&v| v == 0.0));
    }
}

#[test]
fn shared_effect_channels_have_independent_histories() {
    use super::effect_bus::{BusEffects, EffectBusState};
    use crate::synthesis::effects::Delay;
    let mut bus = EffectBusState::new(
        BusEffects {
            delay: Some(Delay::with_sample_rate(0.125, 0.5, 1.0, RATE)),
            reverb: None,
        },
        64,
    );
    bus.input[0] = 1.0; // left-only impulse
    let mut heard_left = false;
    for _ in 0..8 {
        let mut out = [0.0; 128];
        bus.process_add(&mut out, 2, RATE);
        assert!(out.chunks_exact(2).all(|frame| frame[1] == 0.0));
        heard_left |= out.iter().any(|v| v.abs() > 0.1);
        bus.input.fill(0.0);
    }
    assert!(heard_left);
}

#[test]
fn direct_voice_keeps_fade_pause_and_playback_rate_controls() {
    use super::source::SoundSource;
    let mut callback = Callback::new(false);
    callback.command(AudioCommand::StopAll);
    callback.command(AudioCommand::PlaySource {
        options: Default::default(),
        id: 2,
        source: Box::new(SoundSource::sample(
            Sample::from_mono(vec![0.5; 4096], RATE as u32),
            1.0,
            1.0,
            0.0,
        )),
        bus: None,
    });
    callback.command(AudioCommand::SetPlaybackRate { id: 2, rate: 2.0 });
    assert!(callback.render(128).iter().any(|v| v.abs() > 0.1));
    callback.command(AudioCommand::Pause { id: 2 });
    assert!(callback.render(128).iter().all(|&v| v == 0.0));
    callback.command(AudioCommand::Resume { id: 2 });
    callback.command(AudioCommand::FadeOut {
        id: 2,
        duration: 0.125,
    });
    let fade = callback.render(128);
    assert!(fade[0] > fade[254]);
    assert!(callback.render(128).iter().all(|&v| v == 0.0));
    assert!(callback.state.active_sounds.get_mut(2).is_none());
}

struct Callback {
    state: AudioCallbackState,
    playing: super::playing_states::PlayingStates,
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
            playing: super::playing_states::PlayingStates::new(),
        }
    }

    fn command(&mut self, cmd: AudioCommand) {
        handle_command(
            cmd,
            &mut self.state.effect_buses,
            &mut self.state.active_sounds,
            #[cfg(not(target_arch = "wasm32"))]
            &mut self.state.streaming_sounds,
            &mut self.state.listener,
            &mut self.state.spatial,
            RATE,
            &self.playing,
        );
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames * 2];
        mix_sounds(
            &mut output,
            &mut self.state.effect_buses,
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
