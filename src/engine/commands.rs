//! Audio commands for thread-safe communication with the audio thread.
//!
//! Commands use a bounded FIFO with producer-side coalescing.

use crate::synthesis::spatial::{SoundCone, SpatialParams, SpatialPosition};

/// Unique identifier for playing sounds
pub type SoundId = u64;

/// Commands sent from main thread to audio thread
pub(crate) enum AudioCommand {
    PlaySource {
        id: SoundId,
        source: Box<super::source::SoundSource>,
        options: super::voice_budget::VoiceOptions,
        bus: Option<u64>,
    },
    SetEffectBus {
        id: u64,
        bus: Box<super::effect_bus::EffectBusState>,
    },
    SetEffectBusMix {
        id: u64,
        delay: f32,
        reverb: f32,
    },
    RemoveEffectBus {
        id: u64,
    },
    Play {
        id: SoundId,
        mixer: Box<crate::track::Mixer>,
        options: super::voice_budget::VoiceOptions,
        looping: bool,
    },
    Stop {
        id: SoundId,
    },
    SetVolume {
        id: SoundId,
        volume: f32,
    },
    SetPan {
        id: SoundId,
        pan: f32, // -1.0 (left) to 1.0 (right)
    },
    SetPlaybackRate {
        id: SoundId,
        rate: f32, // 1.0 = normal, 2.0 = double speed/pitch
    },
    Pause {
        id: SoundId,
    },
    Resume {
        id: SoundId,
    },
    PauseAll,
    ResumeAll,
    StopAll,
    FadeOut {
        id: SoundId,
        duration: f32, // Duration in seconds
    },
    FadeIn {
        id: SoundId,
        duration: f32,      // Duration in seconds
        target_volume: f32, // Target volume (0.0-1.0)
    },
    TweenPan {
        id: SoundId,
        target_pan: f32, // Target pan (-1.0 to 1.0)
        duration: f32,   // Duration in seconds
    },
    TweenPlaybackRate {
        id: SoundId,
        target_rate: f32, // Target playback rate
        duration: f32,    // Duration in seconds
    },
    SetSoundPosition {
        id: SoundId,
        position: SpatialPosition,
    },
    SetSoundVelocity {
        id: SoundId,
        vx: f32,
        vy: f32,
        vz: f32,
    },
    SetListenerPosition {
        x: f32,
        y: f32,
        z: f32,
    },
    SetListenerVelocity {
        vx: f32,
        vy: f32,
        vz: f32,
    },
    SetListenerForward {
        x: f32,
        y: f32,
        z: f32,
    },
    SetSpatialParams {
        params: SpatialParams,
    },
    SetSoundCone {
        id: SoundId,
        cone: Option<SoundCone>,
    },
    SetSoundOcclusion {
        id: SoundId,
        occlusion: f32,
    },
    // Streaming commands (only available on native platforms - no FS on web)
    #[cfg(not(target_arch = "wasm32"))]
    StreamFile {
        id: SoundId,
        stream: Box<super::streaming::StreamingSound>,
    },
    #[cfg(not(target_arch = "wasm32"))]
    StopStream {
        id: SoundId,
    },
    #[cfg(not(target_arch = "wasm32"))]
    PauseStream {
        id: SoundId,
    },
    #[cfg(not(target_arch = "wasm32"))]
    ResumeStream {
        id: SoundId,
    },
    #[cfg(not(target_arch = "wasm32"))]
    SetStreamVolume {
        id: SoundId,
        volume: f32,
    },
    #[cfg(not(target_arch = "wasm32"))]
    SetStreamPan {
        id: SoundId,
        pan: f32,
    },
}
