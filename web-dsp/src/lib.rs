//! Isolated worker entry points. No AudioContext, game memory, or shared memory.
use std::collections::HashMap;
use tunes::{
    engine::{AudioRenderer, BusEffects, VoicePriority},
    prelude::{Delay, Envelope, Reverb, Waveform},
    synthesis::Sample,
    track::Track,
};
use wasm_bindgen::prelude::*;
const FRAMES: usize = 512;

#[wasm_bindgen]
pub struct DspWorker {
    renderer: AudioRenderer,
    samples: HashMap<u32, Sample>,
    pcm: HashMap<String, std::sync::Arc<Vec<f32>>>,
    pcm_bytes: usize,
    sample_bytes: usize,
    output: Vec<f32>,
    rate: f32,
}
#[wasm_bindgen]
impl DspWorker {
    #[wasm_bindgen(constructor)]
    pub fn new(rate: f32) -> Result<DspWorker, JsValue> {
        if !rate.is_finite() || !(8000.0..=192000.0).contains(&rate) {
            return Err(JsValue::from_str("invalid sample rate"));
        }
        Ok(Self {
            renderer: AudioRenderer::new(rate, FRAMES),
            samples: HashMap::new(),
            pcm: HashMap::new(),
            pcm_bytes: 0,
            sample_bytes: 0,
            output: vec![0.0; FRAMES * 2],
            rate,
        })
    }
    pub fn protocol_version(&self) -> u32 {
        tunes::engine::WORKER_PROTOCOL_VERSION
    }
    pub fn register_pcm(&mut self, key: String, data: Vec<f32>) -> Result<(), JsValue> {
        if data.is_empty() || data.iter().any(|v| !v.is_finite()) {
            return Err(JsValue::from_str("invalid PCM"));
        }
        let old = self.pcm.get(&key).map_or(0, |v| v.len() * 4);
        let bytes = self.pcm_bytes - old + data.len() * 4;
        if bytes > 128 * 1024 * 1024 || (!self.pcm.contains_key(&key) && self.pcm.len() >= 128) {
            return Err(JsValue::from_str("worker PCM cache full"));
        }
        self.pcm.insert(key, std::sync::Arc::new(data));
        self.pcm_bytes = bytes;
        Ok(())
    }
    pub fn remove_pcm(&mut self, key: &str) {
        if let Some(data) = self.pcm.remove(key) {
            self.pcm_bytes -= data.len() * 4;
        }
    }
    pub fn submit(&self, json: &str) -> Result<(), JsValue> {
        self.renderer
            .submit_wire(json, &self.pcm)
            .map_err(|e| JsValue::from_str(&e))
    }
    pub fn playing_ids(&self) -> Vec<String> {
        self.renderer.playing_ids()
    }
    pub fn reset(&mut self) {
        self.renderer = AudioRenderer::new(self.rate, FRAMES);
        self.pcm.clear();
        self.pcm_bytes = 0;
        self.samples.clear();
        self.sample_bytes = 0;
    }

    /// Register decoded PCM once, ahead of its first attack. Bounded at 64 samples/64 MiB.
    pub fn register_sample(&mut self, id: u32, data: Vec<f32>, rate: u32) -> Result<(), JsValue> {
        if rate < 8000 || rate > 192000 || data.is_empty() || data.iter().any(|x| !x.is_finite()) {
            return Err(JsValue::from_str("invalid PCM"));
        }
        let old = self.samples.get(&id).map_or(0, |s| s.data.len() * 4);
        let bytes = self.sample_bytes - old + data.len() * 4;
        if bytes > 64 * 1024 * 1024 || (!self.samples.contains_key(&id) && self.samples.len() >= 64)
        {
            return Err(JsValue::from_str("sample cache full"));
        }
        self.samples.insert(id, Sample::from_mono(data, rate));
        self.sample_bytes = bytes;
        Ok(())
    }
    pub fn remove_sample(&mut self, id: u32) {
        if let Some(s) = self.samples.remove(&id) {
            self.sample_bytes -= s.data.len() * 4;
        }
    }
    pub fn play_sample(&self, id: u32, sample: u32, speed: f32, volume: f32, pan: f32) -> bool {
        self.samples.get(&sample).is_some_and(|s| {
            self.renderer
                .play_sample(id as u64, s.clone(), speed, volume, pan)
        })
    }
    /// Protocol v1 synth voice. ADSR/reverb use the same tunes implementation as the game.
    pub fn note(
        &self,
        id: u32,
        frequency: f32,
        wave: u8,
        volume: f32,
        pan: f32,
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
    ) -> bool {
        if ![frequency, volume, pan, attack, decay, sustain, release]
            .iter()
            .all(|x| x.is_finite())
            || !(20.0..=20000.0).contains(&frequency)
            || wave > 3
        {
            return false;
        }
        let wave = match wave {
            0 => Waveform::Sine,
            1 => Waveform::Square,
            2 => Waveform::Sawtooth,
            _ => Waveform::Triangle,
        };
        let mut track = Track::new();
        track.volume = volume.clamp(0.0, 2.0);
        track.pan = pan.clamp(-1.0, 1.0);
        track.add_note_with_waveform_and_envelope(
            &[frequency],
            0.0,
            30.0,
            wave,
            Envelope::new(attack, decay, sustain, release),
        );
        self.renderer
            .play_track(id as u64, track, Some(1), VoicePriority::Important)
    }
    pub fn effects(&self, delay: f32, reverb: f32) -> bool {
        if !delay.is_finite() || !reverb.is_finite() {
            return false;
        }
        self.renderer.set_bus(
            1,
            BusEffects {
                delay: Some(Delay::with_sample_rate(
                    0.25,
                    0.5,
                    delay.clamp(0.0, 1.0),
                    self.rate,
                )),
                reverb: Some(Reverb::with_sample_rate(
                    0.8,
                    0.5,
                    reverb.clamp(0.0, 1.0),
                    self.rate,
                )),
            },
        )
    }
    pub fn release(&self, id: u32, seconds: f32) -> bool {
        self.renderer.release(id as u64, seconds)
    }
    pub fn stop_all(&self) -> bool {
        self.renderer.stop_all()
    }
    pub fn render(&mut self) -> Vec<f32> {
        self.renderer.render(&mut self.output);
        self.output.clone()
    }
    pub fn collect_garbage(&self) {
        self.renderer.collect_garbage();
    }
}
