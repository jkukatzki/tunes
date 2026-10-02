//! Playback sources share the engine's controls without requiring a composition.
use crate::synthesis::{
    simd::SimdLanes,
    spatial::{ListenerConfig, SpatialParams},
    Sample,
};
use crate::track::{Mixer, Track};

pub(crate) enum SoundSource {
    Mixer(Box<Mixer>),
    Track(Box<TrackVoice>),
    Sample(SampleVoice),
}

pub(crate) struct TrackVoice {
    track: Track,
    sample_count: u64,
}

pub(crate) struct SampleVoice {
    sample: Sample,
    speed: f32,
    volume: f32,
    pan: f32,
}

impl SoundSource {
    pub(crate) fn track(mut track: Track, frames: usize) -> Self {
        track.prepare_voice(frames);
        Self::Track(Box::new(TrackVoice {
            track,
            sample_count: 0,
        }))
    }

    pub(crate) fn sample(sample: Sample, speed: f32, volume: f32, pan: f32) -> Self {
        Self::Sample(SampleVoice {
            sample,
            speed,
            volume,
            pan,
        })
    }

    pub(crate) fn duration(&self, looping: bool) -> f32 {
        match self {
            Self::Mixer(m) => {
                if looping {
                    m.total_duration()
                } else {
                    m.playback_duration()
                }
            }
            Self::Track(v) => {
                if looping {
                    v.track.total_duration()
                } else {
                    v.track.playback_duration()
                }
            }
            Self::Sample(v) => v.sample.duration / v.speed,
        }
    }

    pub(crate) fn process_block(
        &mut self,
        output: &mut [f32],
        rate: f32,
        time: f32,
        listener: Option<&ListenerConfig>,
        spatial: Option<&SpatialParams>,
    ) {
        match self {
            Self::Mixer(m) => m.process_block(output, rate, time, listener, spatial),
            Self::Track(v) => {
                let frames = output.len() / 2;
                Mixer::process_track_block(
                    &mut v.track,
                    &mut output[..frames],
                    rate,
                    time,
                    v.sample_count,
                    None,
                    #[cfg(feature = "gpu")]
                    None,
                    false,
                    false,
                );
                v.sample_count = v.sample_count.wrapping_add(frames as u64);
                expand_mono_in_place(output, v.track.pan);
            }
            Self::Sample(v) => {
                let frames = output.len() / 2;
                output[..frames].fill(0.0);
                v.sample.fill_buffer_simd_mono(
                    &mut output[..frames],
                    0.0,
                    time,
                    1.0 / rate,
                    v.speed,
                    v.volume,
                );
                expand_mono_in_place(output, v.pan);
            }
        }
    }
}

fn expand_mono_in_place(output: &mut [f32], pan: f32) {
    // Use the engine's shared render buffer, expanding backwards so unread mono
    // samples cannot be overwritten. Direct voices need no private audio buffers.
    // Retain track-pan + centred default-bus gain for existing volume semantics.
    let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
    let bus_gain = (std::f32::consts::PI * 0.25).fast_cos();
    let left = angle.fast_cos() * bus_gain;
    let right = angle.fast_sin() * bus_gain;
    for i in (0..output.len() / 2).rev() {
        let mono = output[i];
        output[2 * i] = mono * left;
        output[2 * i + 1] = mono * right;
    }
}
