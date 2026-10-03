//! Versioned, owned commands for the isolated DSP module. No pointers cross workers.
use super::{commands::AudioCommand, source::SoundSource, voice_budget::VoiceOptions, BusEffects};
use crate::synthesis::sample::WorkerSampleMetadata;
#[cfg(any(target_arch = "wasm32", test))]
use crate::synthesis::Sample;
use crate::{
    composition::Tempo,
    synthesis::{effects::*, filter::Filter, lfo::ModRoute, spatial::*},
    track::{AudioEvent, Bus, Mixer, Track},
    track::{DrumEvent, NoteEvent, SampleEvent},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
/// Version of the private browser wire protocol. Package both WASM modules together.
pub const VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
pub(crate) struct WireCommand {
    pub version: u32,
    pub id: String,
    pub op: Op,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
pub(crate) enum Op {
    Play {
        source: Source,
        bus: Option<u64>,
        options: VoiceOptions,
        looping: bool,
    },
    Bus {
        effects: Effects,
    },
    Control {
        name: String,
        args: Vec<f32>,
        position: Option<SpatialPosition>,
        cone: Option<SoundCone>,
        spatial: Option<SpatialParams>,
    },
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Source {
    Track(TrackData),
    Sample {
        key: String,
        sample: WorkerSampleMetadata,
        speed: f32,
        volume: f32,
        pan: f32,
    },
    Mixer {
        buses: Vec<BusData>,
        master: Effects,
    },
}
#[derive(Serialize, Deserialize)]
pub(crate) struct TrackData {
    id: u32,
    name: Option<String>,
    bus_id: u32,
    volume: f32,
    pan: f32,
    filter: Filter,
    modulation: Vec<ModRoute>,
    effects: Effects,
    events: Vec<Event>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct BusData {
    id: u32,
    name: String,
    volume: f32,
    pan: f32,
    muted: bool,
    soloed: bool,
    effects: Effects,
    tracks: Vec<TrackData>,
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Event {
    Note(NoteEvent),
    Drum(DrumEvent),
    Sample {
        key: String,
        sample: WorkerSampleMetadata,
        start: f32,
        speed: f32,
        volume: f32,
        position: Option<SpatialPosition>,
    },
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Effects {
    eq: Option<EQ>,
    compressor: Option<Compressor>,
    gate: Option<Gate>,
    saturation: Option<Saturation>,
    bitcrusher: Option<BitCrusher>,
    distortion: Option<Distortion>,
    chorus: Option<Chorus>,
    phaser: Option<Phaser>,
    flanger: Option<Flanger>,
    ring_mod: Option<RingModulator>,
    tremolo: Option<Tremolo>,
    autopan: Option<AutoPan>,
    delay: Option<Delay>,
    reverb: Option<Reverb>,
    limiter: Option<Limiter>,
    parametric_eq: Option<ParametricEQ>,
}
impl Effects {
    #[cfg(any(target_arch = "wasm32", test))]
    fn encode(chain: &EffectChain) -> Result<Self, String> {
        if chain.convolution_reverb.is_some()
            || chain.phase_vocoder.is_some()
            || chain.spectral_freeze.is_some()
            || chain.spectral_gate.is_some()
            || chain.spectral_compressor.is_some()
            || chain.spectral_robotize.is_some()
            || chain.spectral_delay.is_some()
            || chain.spectral_filter.is_some()
            || chain.spectral_blur.is_some()
            || chain.spectral_shift.is_some()
            || chain.spectral_exciter.is_some()
            || chain.spectral_invert.is_some()
            || chain.spectral_widen.is_some()
            || chain.spectral_morph.is_some()
            || chain.spectral_dynamics.is_some()
            || chain.spectral_scramble.is_some()
            || chain.formant_shifter.is_some()
            || chain.spectral_harmonizer.is_some()
            || chain.spectral_resonator.is_some()
            || chain.spectral_panner.is_some()
        {
            return Err(
                "Spectral/convolution effects are not supported by worker protocol v2".into(),
            );
        }
        Ok(Self {
            eq: chain.eq.clone(),
            compressor: chain.compressor.clone(),
            gate: chain.gate.clone(),
            saturation: chain.saturation.clone(),
            bitcrusher: chain.bitcrusher.clone(),
            distortion: chain.distortion.clone(),
            chorus: chain.chorus.clone(),
            phaser: chain.phaser.clone(),
            flanger: chain.flanger.clone(),
            ring_mod: chain.ring_mod.clone(),
            tremolo: chain.tremolo.clone(),
            autopan: chain.autopan.clone(),
            delay: chain.delay.clone(),
            reverb: chain.reverb.clone(),
            limiter: chain.limiter.clone(),
            parametric_eq: chain.parametric_eq.clone(),
        })
    }
    fn decode(self) -> EffectChain {
        let mut chain = EffectChain::new();
        chain.eq = self.eq;
        chain.compressor = self.compressor;
        chain.gate = self.gate;
        chain.saturation = self.saturation;
        chain.bitcrusher = self.bitcrusher;
        chain.distortion = self.distortion;
        chain.chorus = self.chorus;
        chain.phaser = self.phaser;
        chain.flanger = self.flanger;
        chain.ring_mod = self.ring_mod;
        chain.tremolo = self.tremolo;
        chain.autopan = self.autopan;
        chain.delay = self.delay;
        chain.reverb = self.reverb;
        chain.limiter = self.limiter;
        chain.parametric_eq = self.parametric_eq;
        chain.compute_effect_order();
        chain
    }
}

impl TrackData {
    #[cfg(any(target_arch = "wasm32", test))]
    fn encode(
        track: &Track,
        upload: &mut impl FnMut(&Sample) -> Result<String, String>,
    ) -> Result<Self, String> {
        let mut events = Vec::with_capacity(track.events.len());
        for event in &track.events {
            events.push(match event {
                AudioEvent::Note(n) => Event::Note(n.clone()),
                AudioEvent::Drum(d) => Event::Drum(*d),
                AudioEvent::Sample(s) => Event::Sample {
                    key: upload(&s.sample)?,
                    sample: (&s.sample).into(),
                    start: s.start_time,
                    speed: s.playback_rate,
                    volume: s.volume,
                    position: s.spatial_position,
                },
                // Tempo/key metadata does not participate in already-timed audio rendering.
                _ => continue,
            });
        }
        Ok(Self {
            id: track.id,
            name: track.name.clone(),
            bus_id: track.bus_id,
            volume: track.volume,
            pan: track.pan,
            filter: track.filter,
            modulation: track.modulation.clone(),
            effects: Effects::encode(&track.effects)?,
            events,
        })
    }
    fn decode(self, samples: &HashMap<String, Arc<Vec<f32>>>) -> Result<Track, String> {
        if self.events.len() > 65536 {
            return Err("too many track events".into());
        }
        let mut track = Track::new();
        track.id = self.id;
        track.name = self.name;
        track.bus_id = self.bus_id;
        track.volume = self.volume;
        track.pan = self.pan;
        track.filter = self.filter;
        track.modulation = self.modulation;
        track.effects = self.effects.decode();
        for event in self.events {
            track.events.push(match event {
                Event::Note(n) => {
                    if n.num_freqs > 8 {
                        return Err("invalid frequency count".into());
                    }
                    AudioEvent::Note(n)
                }
                Event::Drum(d) => AudioEvent::Drum(d),
                Event::Sample {
                    key,
                    sample,
                    start,
                    speed,
                    volume,
                    position,
                } => {
                    let sample = sample
                        .with_pcm(samples.get(&key).ok_or("sample not registered")?.clone())?;
                    AudioEvent::Sample(SampleEvent {
                        sample,
                        start_time: start,
                        playback_rate: speed,
                        volume,
                        spatial_position: position,
                    })
                }
            });
        }
        // Directly inserted events must participate in timing/index preparation.
        track.worker_invalidate_events();
        Ok(track)
    }
}
impl Source {
    #[cfg(any(target_arch = "wasm32", test))]
    fn encode(
        source: &SoundSource,
        upload: &mut impl FnMut(&Sample) -> Result<String, String>,
    ) -> Result<Self, String> {
        Ok(match source {
            SoundSource::Track(v) => Self::Track(TrackData::encode(&v.track, upload)?),
            SoundSource::Sample(v) => Self::Sample {
                key: upload(&v.sample)?,
                sample: (&v.sample).into(),
                speed: v.speed,
                volume: v.volume,
                pan: v.pan,
            },
            SoundSource::Mixer(m) => Self::mixer(m, upload)?,
        })
    }
    #[cfg(any(target_arch = "wasm32", test))]
    fn mixer(
        m: &Mixer,
        upload: &mut impl FnMut(&Sample) -> Result<String, String>,
    ) -> Result<Self, String> {
        let mut buses = vec![];
        for b in m.buses.iter().flatten() {
            buses.push(BusData {
                id: b.id,
                name: b.name.clone(),
                volume: b.volume,
                pan: b.pan,
                muted: b.muted,
                soloed: b.soloed,
                effects: Effects::encode(&b.effects)?,
                tracks: b
                    .tracks
                    .iter()
                    .map(|t| TrackData::encode(t, upload))
                    .collect::<Result<_, _>>()?,
            });
        }
        Ok(Self::Mixer {
            buses,
            master: Effects::encode(&m.master)?,
        })
    }
    fn decode(
        self,
        samples: &HashMap<String, Arc<Vec<f32>>>,
        frames: usize,
    ) -> Result<SoundSource, String> {
        Ok(match self {
            Self::Track(t) => SoundSource::track(t.decode(samples)?, frames * 16),
            Self::Sample {
                key,
                sample,
                speed,
                volume,
                pan,
            } => {
                let sample =
                    sample.with_pcm(samples.get(&key).ok_or("sample not registered")?.clone())?;
                SoundSource::sample(sample, speed, volume, pan)
            }
            Self::Mixer { buses, master } => {
                if buses.len() > 256 {
                    return Err("too many buses".into());
                }
                let mut mixer = Mixer::new(Tempo::new(120.0));
                for b in buses {
                    if b.id > 4096 {
                        return Err("bus ID out of bounds".into());
                    }
                    let mut bus = Bus::new(b.id, b.name);
                    bus.volume = b.volume;
                    bus.pan = b.pan;
                    bus.muted = b.muted;
                    bus.soloed = b.soloed;
                    bus.effects = b.effects.decode();
                    bus.tracks = b
                        .tracks
                        .into_iter()
                        .map(|t| t.decode(samples))
                        .collect::<Result<_, _>>()?;
                    mixer.add_bus(bus);
                }
                mixer.master = master.decode();
                mixer.prepare_realtime(frames * 16);
                SoundSource::Mixer(Box::new(mixer))
            }
        })
    }
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn encode(
    command: &AudioCommand,
    upload: &mut impl FnMut(&Sample) -> Result<String, String>,
) -> Result<WireCommand, String> {
    let (id, op) = match command {
        AudioCommand::PlaySource {
            id,
            source,
            options,
            bus,
        } => (
            *id,
            Op::Play {
                source: Source::encode(source, upload)?,
                bus: *bus,
                options: *options,
                looping: false,
            },
        ),
        AudioCommand::Play {
            id,
            mixer,
            options,
            looping,
        } => (
            *id,
            Op::Play {
                source: Source::mixer(mixer, upload)?,
                bus: None,
                options: *options,
                looping: *looping,
            },
        ),
        AudioCommand::SetEffectBus { id, bus } => (
            *id,
            Op::Bus {
                effects: Effects::encode(&bus.effects[0])?,
            },
        ),
        AudioCommand::Stop { id } => (
            *id,
            Op::Control {
                name: "Stop".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetVolume { id, volume } => (
            *id,
            Op::Control {
                name: "SetVolume".into(),
                args: vec![*volume],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetPan { id, pan } => (
            *id,
            Op::Control {
                name: "SetPan".into(),
                args: vec![*pan],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetPlaybackRate { id, rate } => (
            *id,
            Op::Control {
                name: "SetPlaybackRate".into(),
                args: vec![*rate],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::Pause { id } => (
            *id,
            Op::Control {
                name: "Pause".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::Resume { id } => (
            *id,
            Op::Control {
                name: "Resume".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::PauseAll => (
            0,
            Op::Control {
                name: "PauseAll".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::ResumeAll => (
            0,
            Op::Control {
                name: "ResumeAll".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::StopAll => (
            0,
            Op::Control {
                name: "StopAll".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::FadeOut { id, duration } => (
            *id,
            Op::Control {
                name: "FadeOut".into(),
                args: vec![*duration],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::FadeIn {
            id,
            duration,
            target_volume,
        } => (
            *id,
            Op::Control {
                name: "FadeIn".into(),
                args: vec![*duration, *target_volume],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::TweenPan {
            id,
            target_pan,
            duration,
        } => (
            *id,
            Op::Control {
                name: "TweenPan".into(),
                args: vec![*target_pan, *duration],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::TweenPlaybackRate {
            id,
            target_rate,
            duration,
        } => (
            *id,
            Op::Control {
                name: "TweenPlaybackRate".into(),
                args: vec![*target_rate, *duration],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetSoundVelocity { id, vx, vy, vz } => (
            *id,
            Op::Control {
                name: "SetSoundVelocity".into(),
                args: vec![*vx, *vy, *vz],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetListenerPosition { x, y, z } => (
            0,
            Op::Control {
                name: "SetListenerPosition".into(),
                args: vec![*x, *y, *z],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetListenerVelocity { vx, vy, vz } => (
            0,
            Op::Control {
                name: "SetListenerVelocity".into(),
                args: vec![*vx, *vy, *vz],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetListenerForward { x, y, z } => (
            0,
            Op::Control {
                name: "SetListenerForward".into(),
                args: vec![*x, *y, *z],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetSoundOcclusion { id, occlusion } => (
            *id,
            Op::Control {
                name: "SetSoundOcclusion".into(),
                args: vec![*occlusion],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetEffectBusMix { id, delay, reverb } => (
            *id,
            Op::Control {
                name: "SetEffectBusMix".into(),
                args: vec![*delay, *reverb],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::RemoveEffectBus { id } => (
            *id,
            Op::Control {
                name: "RemoveEffectBus".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetSoundPosition { id, position } => (
            *id,
            Op::Control {
                name: "SetSoundPosition".into(),
                args: vec![],
                position: Some(*position),
                cone: None,
                spatial: None,
            },
        ),
        AudioCommand::SetSoundCone { id, cone } => (
            *id,
            Op::Control {
                name: "SetSoundCone".into(),
                args: vec![],
                position: None,
                cone: *cone,
                spatial: None,
            },
        ),
        AudioCommand::SetSpatialParams { params } => (
            0,
            Op::Control {
                name: "SetSpatialParams".into(),
                args: vec![],
                position: None,
                cone: None,
                spatial: Some(*params),
            },
        ),
        #[cfg(not(target_arch = "wasm32"))]
        _ => return Err("native streaming cannot be sent to a browser worker".into()),
    };
    Ok(WireCommand {
        version: VERSION,
        id: id.to_string(),
        op,
    })
}
impl WireCommand {
    pub(crate) fn decode(
        self,
        samples: &HashMap<String, Arc<Vec<f32>>>,
        frames: usize,
    ) -> Result<AudioCommand, String> {
        if self.version != VERSION {
            return Err(
                "DSP protocol version mismatch: rebuild game and audio module together".into(),
            );
        }
        let id = self.id.parse::<u64>().map_err(|_| "invalid sound ID")?;
        Ok(match self.op {
            Op::Play {
                source,
                bus,
                options,
                looping,
            } => {
                let source = source.decode(samples, frames)?;
                if looping {
                    match source {
                        SoundSource::Mixer(mixer) => AudioCommand::Play {
                            id,
                            mixer,
                            options,
                            looping,
                        },
                        _ => return Err("looping direct source unsupported".into()),
                    }
                } else {
                    AudioCommand::PlaySource {
                        id,
                        source: Box::new(source),
                        options,
                        bus,
                    }
                }
            }
            Op::Bus { effects } => AudioCommand::SetEffectBus {
                id,
                bus: Box::new(super::effect_bus::EffectBusState::new(
                    BusEffects {
                        delay: effects.delay,
                        reverb: effects.reverb,
                    },
                    frames,
                )),
            },
            Op::Control {
                name,
                args,
                position,
                cone,
                spatial,
            } => {
                if args.iter().any(|v| !v.is_finite()) {
                    return Err("invalid control value".into());
                }
                match name.as_str() {
                    "Stop" if args.len() == 0 => AudioCommand::Stop { id },
                    "SetVolume" if args.len() == 1 => AudioCommand::SetVolume {
                        id,
                        volume: args[0],
                    },
                    "SetPan" if args.len() == 1 => AudioCommand::SetPan { id, pan: args[0] },
                    "SetPlaybackRate" if args.len() == 1 => {
                        AudioCommand::SetPlaybackRate { id, rate: args[0] }
                    }
                    "Pause" if args.len() == 0 => AudioCommand::Pause { id },
                    "Resume" if args.len() == 0 => AudioCommand::Resume { id },
                    "PauseAll" if args.len() == 0 => AudioCommand::PauseAll,
                    "ResumeAll" if args.len() == 0 => AudioCommand::ResumeAll,
                    "StopAll" if args.len() == 0 => AudioCommand::StopAll,
                    "FadeOut" if args.len() == 1 => AudioCommand::FadeOut {
                        id,
                        duration: args[0],
                    },
                    "FadeIn" if args.len() == 2 => AudioCommand::FadeIn {
                        id,
                        duration: args[0],
                        target_volume: args[1],
                    },
                    "TweenPan" if args.len() == 2 => AudioCommand::TweenPan {
                        id,
                        target_pan: args[0],
                        duration: args[1],
                    },
                    "TweenPlaybackRate" if args.len() == 2 => AudioCommand::TweenPlaybackRate {
                        id,
                        target_rate: args[0],
                        duration: args[1],
                    },
                    "SetSoundVelocity" if args.len() == 3 => AudioCommand::SetSoundVelocity {
                        id,
                        vx: args[0],
                        vy: args[1],
                        vz: args[2],
                    },
                    "SetListenerPosition" if args.len() == 3 => AudioCommand::SetListenerPosition {
                        x: args[0],
                        y: args[1],
                        z: args[2],
                    },
                    "SetListenerVelocity" if args.len() == 3 => AudioCommand::SetListenerVelocity {
                        vx: args[0],
                        vy: args[1],
                        vz: args[2],
                    },
                    "SetListenerForward" if args.len() == 3 => AudioCommand::SetListenerForward {
                        x: args[0],
                        y: args[1],
                        z: args[2],
                    },
                    "SetSoundOcclusion" if args.len() == 1 => AudioCommand::SetSoundOcclusion {
                        id,
                        occlusion: args[0],
                    },
                    "SetEffectBusMix" if args.len() == 2 => AudioCommand::SetEffectBusMix {
                        id,
                        delay: args[0],
                        reverb: args[1],
                    },
                    "RemoveEffectBus" if args.len() == 0 => AudioCommand::RemoveEffectBus { id },
                    "SetSoundPosition" => AudioCommand::SetSoundPosition {
                        id,
                        position: position.ok_or("missing position")?,
                    },
                    "SetSoundCone" => AudioCommand::SetSoundCone { id, cone },
                    "SetSpatialParams" => AudioCommand::SetSpatialParams {
                        params: spatial.ok_or("missing spatial config")?,
                    },
                    _ => return Err("unknown or malformed audio control".into()),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthesis::{Envelope, Waveform};
    fn roundtrip(source: &SoundSource) -> SoundSource {
        let mut samples = HashMap::new();
        let encoded = Source::encode(source, &mut |sample| {
            samples.insert("pcm".into(), sample.data.clone());
            Ok("pcm".into())
        })
        .unwrap();
        let json = serde_json::to_string(&encoded).unwrap();
        let decoded: Source = serde_json::from_str(&json).unwrap();
        decoded.decode(&samples, 64).unwrap()
    }
    fn compare(mut original: SoundSource, mut remote: SoundSource) {
        for block in 0..30 {
            let mut a = [0.0; 128];
            let mut b = [0.0; 128];
            let time = block as f32 * 64.0 / 44100.0;
            original.process_block(&mut a, 44100.0, time, None, None);
            remote.process_block(&mut b, 44100.0, time, None, None);
            for (a, b) in a.into_iter().zip(b) {
                assert!((a - b).abs() < 1e-6, "{a} != {b}");
            }
        }
    }
    #[test]
    fn filtered_distorted_track_survives_json_transport() {
        let mut track = Track::new();
        track.volume = 0.4;
        track.pan = -0.3;
        track.filter = Filter::low_pass(300.0, 0.5);
        track.effects.distortion = Some(Distortion::new(0.4, 0.3));
        track.add_note_with_waveform_and_envelope(
            &[80.0],
            0.0,
            1.0,
            Waveform::Sine,
            Envelope::default(),
        );
        let source = SoundSource::track(track, 1024);
        let remote = roundtrip(&source);
        compare(source, remote);
    }
    #[test]
    fn sample_pcm_is_separate_and_loop_metadata_survives() {
        let sample = Sample::from_mono(vec![0.1, 0.2, 0.3, 0.4, 0.5], 44100)
            .with_loop_frames(1, 4)
            .unwrap();
        let source = SoundSource::sample(sample, 1.25, 0.4, 0.2);
        let remote = roundtrip(&source);
        compare(source, remote);
    }
    #[test]
    fn mixer_routing_and_effects_survive_transport() {
        let mut comp = crate::composition::Composition::new(Tempo::new(120.0));
        comp.track("trigger").volume(0.3).note(&[60.0], 1.0);
        comp.track("lead")
            .compressor(Compressor::new(-20.0, 4.0, 0.01, 0.1, 0.0).with_sidechain_track("trigger"))
            .volume(0.4)
            .pan(0.3)
            .filter(Filter::low_pass(300.0, 0.5))
            .note(&[100.0], 1.0);
        let mut mixer = comp.into_mixer();
        mixer.prepare_realtime(1024);
        let source = SoundSource::Mixer(Box::new(mixer));
        let remote = roundtrip(&source);
        compare(source, remote);
    }
    #[test]
    fn transport_preserves_large_ids_and_release_completion() {
        let id = (1u64 << 54) + 7;
        let mut renderer = super::super::AudioRenderer::new(44100.0, 64);
        assert!(renderer.play_sample(
            id,
            Sample::from_mono(vec![0.5; 44100], 44100),
            1.0,
            1.0,
            0.0
        ));
        renderer.render(&mut [0.0; 128]);
        let wire = encode(
            &AudioCommand::FadeOut {
                id,
                duration: 0.001,
            },
            &mut |_| unreachable!(),
        )
        .unwrap();
        renderer
            .submit_wire(&serde_json::to_string(&wire).unwrap(), &HashMap::new())
            .unwrap();
        renderer.render(&mut [0.0; 128]);
        assert!(!renderer.is_playing(id));
    }
    #[test]
    fn stereo_metadata_is_validated_before_rendering() {
        let json = serde_json::json!({"channels":2,"sample_rate":44100,"duration":4.0/44100.0,
            "num_frames":4,"loop_start":1,"loop_end":4});
        let pcm = Arc::new(vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
        let metadata: WorkerSampleMetadata = serde_json::from_value(json.clone()).unwrap();
        let sample = metadata.with_pcm(pcm.clone()).unwrap();
        assert_eq!(sample.channels, 2);
        assert_eq!(sample.loop_points(), Some((1, 4)));
        assert_eq!(sample.sample_at(0.0, 1.0), (0.1, 0.2));
        for (field, value) in [("channels", 0), ("num_frames", 99), ("loop_end", 99)] {
            let mut invalid = json.clone();
            invalid[field] = value.into();
            let metadata: WorkerSampleMetadata = serde_json::from_value(invalid).unwrap();
            assert!(metadata.with_pcm(pcm.clone()).is_err());
        }
    }

    #[test]
    fn mismatched_protocol_and_unknown_samples_fail_explicitly() {
        let mut wire = encode(&AudioCommand::StopAll, &mut |_| unreachable!()).unwrap();
        wire.version = 0;
        assert!(wire.decode(&HashMap::new(), 64).is_err());
        let sample = Sample::from_mono(vec![0.1; 10], 44100);
        let source = Source::encode(&SoundSource::sample(sample, 1.0, 1.0, 0.0), &mut |_| {
            Ok("missing".into())
        })
        .unwrap();
        assert!(source.decode(&HashMap::new(), 64).is_err());
    }
}
