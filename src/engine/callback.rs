//! Audio callback state and mixing functions.
//!
//! Contains the pre-allocated buffers and mixing logic for the real-time audio callback.

use super::active_sound::ActiveSound;
use super::commands::{AudioCommand, SoundId};
use super::sound_pool::SoundPool;
#[cfg(not(target_arch = "wasm32"))]
use super::streaming::StreamingSound;
use crate::synthesis::spatial::{calculate_spatial_with_cone, ListenerConfig, SpatialParams, Vec3};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;
use wide::f32x4;

/// Audio callback state with reusable mixing buffers
///
/// Holds pre-allocated buffers to avoid allocations in the real-time audio thread.
/// All buffers are reused across callback invocations.
pub(crate) struct AudioCallbackState {
    /// Active sounds packed together, independent of monotonically increasing SoundIds
    pub listener: ListenerConfig,
    pub spatial: SpatialParams,
    pub active_sounds: SoundPool<ActiveSound>,
    pub effect_buses: SoundPool<super::effect_bus::EffectBusState>,
    /// Streaming sounds (separate from pre-rendered sounds, native only)
    #[cfg(not(target_arch = "wasm32"))]
    pub streaming_sounds: SoundPool<StreamingSound>,
    /// Pre-allocated temp buffer for mixing (stereo interleaved)
    /// Size is determined by the maximum buffer size we expect
    pub temp_buffer: Vec<f32>,
    /// Pre-allocated list for tracking finished sounds (avoids allocation during cleanup)
    pub finished_sounds: Vec<SoundId>,
    /// Pre-allocated list for tracking finished streams (native only)
    #[cfg(not(target_arch = "wasm32"))]
    pub finished_streams: Vec<SoundId>,
}

impl AudioCallbackState {
    pub fn new() -> Self {
        Self {
            listener: ListenerConfig::default(),
            spatial: SpatialParams::default(),
            // Pre-allocate space for 128 concurrent sounds (typical max for games)
            active_sounds: SoundPool::with_capacity(128),
            effect_buses: SoundPool::with_capacity(16),
            #[cfg(not(target_arch = "wasm32"))]
            streaming_sounds: SoundPool::with_capacity(16),
            // Pre-allocate for a reasonably large buffer (2048 frames stereo = 4096 samples)
            temp_buffer: vec![0.0; 4096 * 16],
            finished_sounds: Vec::with_capacity(128),
            #[cfg(not(target_arch = "wasm32"))]
            finished_streams: Vec::with_capacity(16),
        }
    }

    /// Ensure temp buffer is large enough for the given size
    #[allow(dead_code)]
    pub fn ensure_temp_buffer_size(&mut self, required_size: usize) {
        if self.temp_buffer.len() < required_size {
            self.temp_buffer.resize(required_size, 0.0);
        }
    }
}

/// Handle commands from the main thread (called from audio thread)
pub(crate) fn handle_command(
    cmd: AudioCommand,
    effect_buses: &mut SoundPool<super::effect_bus::EffectBusState>,
    active_sounds: &mut SoundPool<ActiveSound>,
    #[cfg(not(target_arch = "wasm32"))] streaming_sounds: &mut SoundPool<StreamingSound>,
    listener: &mut ListenerConfig,
    spatial: &mut SpatialParams,
    _sample_rate: f32,
    playing_states: &super::playing_states::PlayingStates,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let stream_count = streaming_sounds.len();
    #[cfg(target_arch = "wasm32")]
    let stream_count = 0;
    match cmd {
        AudioCommand::PlaySource {
            id,
            source,
            bus,
            options,
        } => {
            if !super::voice_budget::admit(active_sounds, options, stream_count) {
                let sound = ActiveSound::from_source(*source, false, bus);
                assert!(active_sounds.retired.push(sound).is_ok());
                playing_states.remove(&id);
                return;
            }
            if let Some(route) = bus.and_then(|id| effect_buses.get_mut(id)) {
                route.paused = false;
            }
            let mut sound = ActiveSound::from_source(*source, false, bus);
            sound.options = options;
            active_sounds.insert(id, sound);
        }
        AudioCommand::SetEffectBus { id, bus } => {
            if effect_buses.get_mut(id).is_some() || effect_buses.len() < 16 {
                effect_buses.insert(id, *bus);
            } else {
                assert!(effect_buses.retired.push(*bus).is_ok());
            }
        }
        AudioCommand::RemoveEffectBus { id } => {
            effect_buses.remove(id);
        }
        AudioCommand::SetEffectBusMix { id, delay, reverb } => {
            if let Some(bus) = effect_buses.get_mut(id) {
                bus.set_mix(delay, reverb);
            }
        }

        AudioCommand::Play {
            id,
            mixer,
            looping,
            options,
        } => {
            let accepted = super::voice_budget::admit(active_sounds, options, stream_count);
            let mut sound =
                ActiveSound::from_source(super::source::SoundSource::Mixer(mixer), looping, None);
            sound.options = options;
            if accepted {
                active_sounds.insert(id, sound);
            } else {
                assert!(active_sounds.retired.push(sound).is_ok());
                playing_states.remove(&id);
            }
        }
        AudioCommand::Stop { id } => {
            active_sounds.remove(id);
            playing_states.remove(&id);
        }
        AudioCommand::SetVolume { id, volume } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.volume = volume.clamp(0.0, 1.0);
            }
        }
        AudioCommand::SetPan { id, pan } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.pan = pan.clamp(-1.0, 1.0);
            }
        }
        AudioCommand::SetPlaybackRate { id, rate } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                // Clamp to reasonable range (0.1x to 4.0x speed)
                sound.playback_rate = rate.clamp(0.1, 4.0);
            }
        }
        AudioCommand::Pause { id } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.paused = true;
            }
        }
        AudioCommand::Resume { id } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.paused = false;
            }
        }
        AudioCommand::SetSoundPosition { id, position } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.spatial_position = Some(position);
                sound.spatial_dirty = true; // Mark for recalculation
            }
        }
        AudioCommand::SetSoundVelocity { id, vx, vy, vz } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                if let Some(pos) = &mut sound.spatial_position {
                    pos.set_velocity(vx, vy, vz);
                    sound.spatial_dirty = true;
                }
            }
        }
        AudioCommand::SetListenerPosition { x, y, z } => {
            listener.position = Vec3::new(x, y, z);
            for (_, sound) in active_sounds.iter_mut() {
                sound.spatial_dirty = true;
            }
        }
        AudioCommand::SetListenerVelocity { vx, vy, vz } => {
            listener.velocity = Vec3::new(vx, vy, vz);
            for (_, sound) in active_sounds.iter_mut() {
                sound.spatial_dirty = true;
            }
        }
        AudioCommand::SetListenerForward { x, y, z } => {
            listener.forward = Vec3::new(x, y, z).normalize();
            for (_, sound) in active_sounds.iter_mut() {
                sound.spatial_dirty = true;
            }
        }
        AudioCommand::SetSpatialParams { params } => {
            *spatial = params;
            for (_, sound) in active_sounds.iter_mut() {
                sound.spatial_dirty = true;
            }
        }
        AudioCommand::SetSoundCone { id, cone } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.spatial_cone = cone;
                sound.spatial_dirty = true; // Mark for recalculation
            }
        }
        AudioCommand::SetSoundOcclusion { id, occlusion } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.occlusion = occlusion.clamp(0.0, 1.0);
                sound.spatial_dirty = true; // Mark for recalculation
            }
        }
        AudioCommand::PauseAll => {
            for (_, bus) in effect_buses.iter_mut() {
                bus.paused = true;
            }
            for (_, sound) in active_sounds.iter_mut() {
                sound.paused = true;
            }
        }
        AudioCommand::ResumeAll => {
            for (_, bus) in effect_buses.iter_mut() {
                bus.paused = false;
            }
            for (_, sound) in active_sounds.iter_mut() {
                sound.paused = false;
            }
        }
        AudioCommand::StopAll => {
            for (_, bus) in effect_buses.iter_mut() {
                bus.reset();
            }
            for (id, _) in active_sounds.iter_mut() {
                playing_states.remove(id);
            }
            active_sounds.clear();
            #[cfg(not(target_arch = "wasm32"))]
            streaming_sounds.clear();
        }
        AudioCommand::FadeOut { id, duration } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.start_fade(duration, 0.0, true);
            }
        }
        AudioCommand::FadeIn {
            id,
            duration,
            target_volume,
        } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.start_fade(duration, target_volume.clamp(0.0, 1.0), false);
            }
        }
        AudioCommand::TweenPan {
            id,
            target_pan,
            duration,
        } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.pan_tween_start_time = Some(sound.control_time);
                sound.pan_tween_duration = duration;
                sound.pan_tween_start_value = sound.pan;
                sound.pan_tween_target_value = target_pan.clamp(-1.0, 1.0);
            }
        }
        AudioCommand::TweenPlaybackRate {
            id,
            target_rate,
            duration,
        } => {
            if let Some(sound) = active_sounds.get_mut(id) {
                sound.rate_tween_start_time = Some(sound.control_time);
                sound.rate_tween_duration = duration;
                sound.rate_tween_start_value = sound.playback_rate;
                sound.rate_tween_target_value = target_rate.max(0.1); // Prevent division by zero
            }
        }
        // Streaming commands (native only)
        #[cfg(not(target_arch = "wasm32"))]
        AudioCommand::StreamFile { id, stream } => {
            if streaming_sounds.len() < 16
                && active_sounds.len() + streaming_sounds.len() < super::voice_budget::MAX_VOICES
            {
                streaming_sounds.insert(id, *stream);
            } else {
                assert!(streaming_sounds.retired.push(*stream).is_ok());
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        AudioCommand::StopStream { id } => {
            // Setting to None will trigger Drop, which signals thread to stop
            streaming_sounds.remove(id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        AudioCommand::PauseStream { id } => {
            if let Some(stream) = streaming_sounds.get_mut(id) {
                stream.pause_signal.store(true, Ordering::Relaxed);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        AudioCommand::ResumeStream { id } => {
            if let Some(stream) = streaming_sounds.get_mut(id) {
                stream.pause_signal.store(false, Ordering::Relaxed);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        AudioCommand::SetStreamVolume { id, volume } => {
            if let Some(stream) = streaming_sounds.get_mut(id) {
                stream.volume = volume.clamp(0.0, 1.0);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        AudioCommand::SetStreamPan { id, pan } => {
            if let Some(stream) = streaming_sounds.get_mut(id) {
                stream.pan = pan.clamp(-1.0, 1.0);
            }
        }
    }
}

/// Mix all active sounds into the output buffer (called from audio thread)
///
/// Buffers are reused; larger device blocks or higher concurrency can grow them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mix_sounds(
    output: &mut [f32],
    effect_buses: &mut SoundPool<super::effect_bus::EffectBusState>,
    active_sounds: &mut SoundPool<ActiveSound>,
    temp_buffer: &mut Vec<f32>,
    finished_sounds: &mut Vec<SoundId>,
    listener: &ListenerConfig,
    spatial_params: &SpatialParams,
    sample_rate: f32,
    channels: usize,
) {
    // Clear output buffer
    output.fill(0.0);
    for (_, bus) in effect_buses.iter_mut() {
        bus.input.resize(output.len(), 0.0);
        bus.input.fill(0.0);
    }

    // Clear finished sounds list (reuse allocation)
    finished_sounds.clear();

    // Ensure temp buffer is large enough (may resize on first call, then reuses)
    let num_frames = output.len() / channels;
    let required_size = num_frames * 2;
    if temp_buffer.len() < required_size {
        temp_buffer.resize(required_size, 0.0);
    }

    // Mix each active sound using block processing (cache-friendly sequential iteration)
    for (id, sound) in active_sounds.iter_mut() {
        if sound.paused {
            continue;
        }

        if sound.update_fade() {
            finished_sounds.push(*id);
            continue;
        }

        let duration = sound.duration;

        // Check if sound will finish during this block
        let time_delta = 1.0 / sample_rate;

        if sound.elapsed_time >= duration {
            if sound.looping {
                sound.elapsed_time = 0.0;
                sound.sample_clock = 0.0;
            } else {
                finished_sounds.push(*id);
                continue;
            }
        }

        // Apply pan tween if active
        if let Some(tween_start) = sound.pan_tween_start_time {
            let tween_elapsed = (sound.control_time - tween_start) as f32;
            if tween_elapsed >= sound.pan_tween_duration {
                // Tween complete
                sound.pan = sound.pan_tween_target_value;
                sound.pan_tween_start_time = None;
            } else {
                // Interpolate
                let t = (tween_elapsed / sound.pan_tween_duration).clamp(0.0, 1.0);
                sound.pan = sound.pan_tween_start_value
                    + (sound.pan_tween_target_value - sound.pan_tween_start_value) * t;
            }
        }

        // Apply playback rate tween if active
        if let Some(tween_start) = sound.rate_tween_start_time {
            let tween_elapsed = (sound.control_time - tween_start) as f32;
            if tween_elapsed >= sound.rate_tween_duration {
                // Tween complete
                sound.playback_rate = sound.rate_tween_target_value;
                sound.rate_tween_start_time = None;
            } else {
                // Interpolate
                let t = (tween_elapsed / sound.rate_tween_duration).clamp(0.0, 1.0);
                sound.playback_rate = sound.rate_tween_start_value
                    + (sound.rate_tween_target_value - sound.rate_tween_start_value) * t;
            }
        }

        // Calculate spatial audio if runtime position is set
        // Use cached values if nothing changed, otherwise recalculate
        let (mut spatial_volume, spatial_pan, spatial_pitch, spatial_occlusion) =
            if let Some(pos) = &sound.spatial_position {
                if sound.spatial_dirty {
                    // Recalculate spatial audio
                    let result = calculate_spatial_with_cone(
                        pos,
                        listener,
                        spatial_params,
                        sound.spatial_cone.as_ref(),
                        sound.occlusion,
                    );
                    // Cache the results
                    sound.cached_spatial_volume = result.volume;
                    sound.cached_spatial_pan = result.pan;
                    sound.cached_spatial_pitch = result.pitch;
                    sound.spatial_dirty = false; // Mark as clean
                    (result.volume, result.pan, result.pitch, result.occlusion)
                } else {
                    // Use cached values
                    (
                        sound.cached_spatial_volume,
                        sound.cached_spatial_pan,
                        sound.cached_spatial_pitch,
                        sound.occlusion, // Occlusion is just read directly, not cached
                    )
                }
            } else {
                (1.0, sound.pan, 1.0, 0.0)
            };

        // Apply occlusion as volume reduction
        // 0.0 = no occlusion (full volume), 1.0 = fully occluded (silent)
        spatial_volume *= 1.0 - spatial_occlusion;

        // Apply doppler pitch shift to playback rate
        let effective_playback_rate = (sound.playback_rate * spatial_pitch).clamp(0.1, 16.0);
        let base_block_duration = num_frames as f32 * time_delta;

        // Compute source frames needed for resampling.
        // rate > 1.0 (faster/higher pitch): render more source frames, compress into output.
        // rate < 1.0 (slower/lower pitch):  render fewer source frames, stretch into output.
        // At rate == 1.0 we skip resampling entirely and use the SIMD fast path.
        let needs_resample = (effective_playback_rate - 1.0).abs() > 1e-4;
        let source_frames = if needs_resample {
            ((num_frames as f32 * effective_playback_rate).ceil() as usize).max(1)
        } else {
            num_frames
        };
        let source_size = source_frames * 2;

        // Grow temp buffer if this sound needs more source frames than current capacity
        if temp_buffer.len() < source_size {
            temp_buffer.resize(source_size, 0.0);
        }

        // Only apply composition-time spatial audio if NO runtime position is set
        let (listener_for_mixer, params_for_mixer) = if sound.spatial_position.is_some() {
            (None, None) // Runtime position will handle spatial audio
        } else {
            (Some(listener), Some(spatial_params)) // Use composition-time position
        };

        // Only dry direct samples can be virtualized safely. Stateful synth
        // filters and effect tails must still advance through their DSP.
        if spatial_volume.abs() * sound.volume_at(sound.control_time).abs() < 0.00001
            && matches!(sound.source, super::source::SoundSource::Sample(_))
            && sound.bus.is_none()
        {
            sound.control_time += num_frames as f64 / sample_rate as f64;
            sound.elapsed_time += base_block_duration * effective_playback_rate;
            if sound.update_fade() {
                finished_sounds.push(*id);
            }
            continue;
        }

        // Render source_frames of source material into temp_buffer
        sound.source.process_block(
            &mut temp_buffer[..source_size],
            sample_rate,
            sound.elapsed_time,
            listener_for_mixer,
            params_for_mixer,
        );

        let pan_angle = (spatial_pan + 1.0) * 0.25 * std::f32::consts::PI;
        let left_pan = pan_angle.cos();
        let right_pan = pan_angle.sin();

        let output = match sound.bus.and_then(|id| effect_buses.get_mut(id)) {
            Some(bus) => bus.input.as_mut_slice(),
            None => &mut *output,
        };

        // Mix temp buffer into output with volume/pan/fade applied.
        // SIMD fast path requires rate == 1.0 (no resampling) and no active fade.
        if sound.fade_start_time.is_none() && channels == 2 && !needs_resample {
            // SIMD fast path: no fade, stereo output, playback_rate == 1.0
            let combined_volume = sound.volume * spatial_volume;
            let simd_num_frames = source_frames; // == num_frames when !needs_resample

            mix_stereo_add(
                &mut output[..simd_num_frames * 2],
                &temp_buffer[..simd_num_frames * 2],
                combined_volume,
                left_pan,
                right_pan,
            );
        } else {
            // Scalar path: fade active, mono output, or resampling needed.
            // When needs_resample, reads source frames with linear interpolation.
            for frame_idx in 0..num_frames {
                let (left, right) = if needs_resample {
                    // Fractional source position for this output frame
                    let src_pos = frame_idx as f32 * effective_playback_rate;
                    let src_a = src_pos as usize;
                    let frac = src_pos - src_a as f32;

                    let a_idx = src_a * 2;
                    let b_idx = (src_a + 1) * 2;

                    let (a_l, a_r) = if a_idx + 1 < source_size {
                        (temp_buffer[a_idx], temp_buffer[a_idx + 1])
                    } else {
                        (0.0, 0.0)
                    };
                    let (b_l, b_r) = if b_idx + 1 < source_size {
                        (temp_buffer[b_idx], temp_buffer[b_idx + 1])
                    } else {
                        (a_l, a_r) // Clamp at end of source
                    };

                    (a_l + frac * (b_l - a_l), a_r + frac * (b_r - a_r))
                } else {
                    let idx = frame_idx * 2;
                    (temp_buffer[idx], temp_buffer[idx + 1])
                };

                let effective_volume =
                    sound.volume_at(sound.control_time + frame_idx as f64 / sample_rate as f64);

                let out_left = left * effective_volume * spatial_volume * left_pan;
                let out_right = right * effective_volume * spatial_volume * right_pan;

                // Mix into output
                let out_idx = frame_idx * channels;
                if out_idx < output.len() {
                    if channels == 1 {
                        output[out_idx] += (out_left + out_right) * 0.5;
                    } else if out_idx + 1 < output.len() {
                        output[out_idx] += out_left;
                        output[out_idx + 1] += out_right;
                    }
                }
            }
        }

        sound.control_time += num_frames as f64 / sample_rate as f64;
        if sound.update_fade() {
            finished_sounds.push(*id);
        }

        // Advance elapsed time by the amount of source material consumed this block
        sound.elapsed_time += base_block_duration * effective_playback_rate;
        sound.sample_clock =
            (sound.sample_clock + (num_frames as f32 * effective_playback_rate)) % sample_rate;
    }

    for (_, bus) in effect_buses.iter_mut() {
        bus.process_add(output, channels, sample_rate);
    }

    // Remove finished sounds from the dense pool.
    for id in finished_sounds {
        active_sounds.remove(*id);
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Mix streaming sounds into the output buffer (called from audio thread)
///
/// Reads decoded samples from ring buffers and mixes them into the output.
/// This is ALLOCATION-FREE and lock-free (uses lockless ring buffer).
pub(crate) fn mix_streaming_sounds(
    output: &mut [f32],
    streaming_sounds: &mut SoundPool<StreamingSound>,
    finished_streams: &mut Vec<SoundId>,
    channels: usize,
) {
    use ringbuf::traits::{Consumer, Observer};

    // Clear finished streams list
    finished_streams.clear();

    // Mix each streaming sound (cache-friendly sequential iteration)
    for (id, stream) in streaming_sounds.iter_mut() {
        // Check if the decoder thread has finished
        if let Some(handle) = &stream.decoder_thread {
            if handle.is_finished() {
                // Thread finished - mark for removal
                finished_streams.push(*id);
                continue;
            }
        }

        // Read available samples from ring buffer
        let available = stream.ring_consumer.occupied_len();
        if available == 0 {
            // Buffer underrun - could happen at start or if decoding is slow
            continue;
        }

        // Calculate how many samples we need (limited by output buffer size)
        let samples_needed = output.len().min(available);

        // Mix samples into output
        for i in (0..samples_needed).step_by(channels) {
            // Pop samples from ring buffer
            let left = stream.ring_consumer.try_pop().unwrap_or(0.0);
            let right = if channels == 2 {
                stream.ring_consumer.try_pop().unwrap_or(0.0)
            } else {
                left // Mono - use same sample for both channels
            };

            // Apply volume and pan (constant-power, matches process_block.rs)
            let pan = stream.pan;
            let pan_angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
            let left_gain = pan_angle.cos() * stream.volume;
            let right_gain = pan_angle.sin() * stream.volume;

            // Mix into output (additively)
            if i < output.len() {
                output[i] += left * left_gain;
            }
            if i + 1 < output.len() {
                output[i + 1] += right * right_gain;
            }
        }
    }

    // Remove finished streams from the dense pool.
    for id in finished_streams.iter() {
        streaming_sounds.remove(*id);
    }
}

/// Mix directly in interleaved layout: [L, R, L, R]. Avoid channel shuffles and
/// runtime ISA dispatch inside every voice. `wide` selects the target's SIMD
/// implementation (including wasm simd128), with a portable scalar fallback.
fn mix_stereo_add(output: &mut [f32], input: &[f32], volume: f32, left: f32, right: f32) {
    debug_assert_eq!(output.len(), input.len());
    debug_assert_eq!(output.len() % 2, 0);
    let volume_vec = f32x4::splat(volume);
    let pan = f32x4::from([left, right, left, right]);
    let mut out_chunks = output.chunks_exact_mut(4);
    let mut in_chunks = input.chunks_exact(4);
    for (out, samples) in out_chunks.by_ref().zip(in_chunks.by_ref()) {
        let source = f32x4::from(<[f32; 4]>::try_from(samples).unwrap());
        let previous = f32x4::from(<[f32; 4]>::try_from(&*out).unwrap());
        // Keep the original multiplication order for PCM compatibility.
        out.copy_from_slice(&(previous + source * volume_vec * pan).to_array());
    }
    for (channel, (out, sample)) in out_chunks
        .into_remainder()
        .iter_mut()
        .zip(in_chunks.remainder())
        .enumerate()
    {
        *out += sample * volume * if channel == 0 { left } else { right };
    }
}

#[cfg(test)]
mod interleaved_tests {
    use super::mix_stereo_add;

    #[test]
    fn interleaved_mix_matches_scalar_bits_including_partial_vectors() {
        for frames in [0, 1, 2, 3, 127, 128, 129, 512] {
            let source: Vec<_> = (0..frames * 2).map(|i| (i as f32 * 1.79).sin()).collect();
            for (volume, left, right) in [(0.0, 1.0, 0.0), (0.71, 0.3, 0.9), (-0.4, 1.0, 1.0)] {
                let mut actual = vec![0.125; source.len()];
                let mut expected = actual.clone();
                for (i, (out, sample)) in expected.iter_mut().zip(&source).enumerate() {
                    *out += sample * volume * if i % 2 == 0 { left } else { right };
                }
                mix_stereo_add(&mut actual, &source, volume, left, right);
                assert_eq!(
                    actual.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
                );
            }
        }
    }
}
