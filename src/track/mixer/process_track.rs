//! Track processing methods for the Mixer.
//!
//! Contains methods for processing individual tracks in both sample-by-sample
//! and block-based modes.

use super::Mixer;
use crate::cache::{CacheKey, CachedSample, SampleCache};
use crate::instruments::drums::DrumType;
use crate::synthesis::simd::{SimdLanes, SIMD};
use crate::track::events::{AudioEvent, NoteEvent};
use crate::track::Track;
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(feature = "gpu")]
use crate::gpu::GpuSynthesizer;

impl Mixer {
    /// Render a complete note into a buffer
    ///
    /// This synthesizes an entire note from start to finish, used for caching.
    /// If GPU synthesizer is provided, uses GPU, otherwise falls back to CPU.
    pub(crate) fn render_note_to_buffer(
        note: &NoteEvent,
        sample_rate: f32,
        #[cfg(feature = "gpu")] gpu_synthesizer: Option<&Arc<GpuSynthesizer>>,
    ) -> Vec<f32> {
        // Try GPU first if available
        #[cfg(feature = "gpu")]
        if let Some(gpu) = gpu_synthesizer {
            if let Ok(samples) = gpu.synthesize_note(note, sample_rate) {
                return samples;
            }
            // GPU failed, fall through to CPU
        }

        // CPU synthesis fallback
        let total_duration = note.envelope.total_duration(note.duration);
        let num_samples = (total_duration * sample_rate) as usize;
        let mut buffer = vec![0.0f32; num_samples];

        let time_delta = 1.0 / sample_rate;

        for (i, sample_out) in buffer.iter_mut().enumerate() {
            let time_in_note = i as f32 * time_delta;
            let envelope_amp = note.envelope.amplitude_at(time_in_note, note.duration);

            let mut note_value = 0.0;

            // Synthesize for the first frequency only (monophonic cache)
            // Polyphonic notes will be handled separately
            if note.num_freqs > 0 {
                let base_freq = note.frequencies[0];

                let freq = if note.pitch_bend_semitones != 0.0 {
                    let bend_progress = (time_in_note / note.duration).min(1.0);
                    let bend_multiplier =
                        2.0f32.powf((note.pitch_bend_semitones * bend_progress) / 12.0);
                    base_freq * bend_multiplier
                } else {
                    base_freq
                };

                let sample = if note.fm_params.mod_index > 0.0 {
                    note.fm_params.sample(freq, time_in_note, note.duration)
                } else if let Some(ref wavetable) = note.custom_wavetable {
                    let phase = (time_in_note * freq) % 1.0;
                    wavetable.sample(phase)
                } else {
                    let phase = (time_in_note * freq) % 1.0;
                    note.waveform.sample(phase)
                };

                note_value += sample * envelope_amp;
            }

            *sample_out = note_value;
        }

        buffer
    }

    /// Process a single track into a mono buffer (block-processing version)
    ///
    /// This is the high-performance version that generates multiple samples at once,
    /// reducing function call overhead and enabling better cache locality.
    ///
    /// # Arguments
    /// * `track` - The track to process
    /// * `buffer` - Output mono buffer to fill
    /// * `sample_rate` - Sample rate in Hz
    /// * `start_time` - Starting time for the block
    /// * `start_sample_count` - Starting sample counter
    /// * `cache` - Optional sample cache for pre-rendered synthesis
    /// * `gpu_synthesizer` - Optional GPU synthesizer for 500-1000x faster rendering
    /// * `prerendered` - If true, skip cache-miss detection (already pre-rendered)
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn process_track_block(
        track: &mut Track,
        buffer: &mut [f32],
        sample_rate: f32,
        start_time: f32,
        start_sample_count: u64,
        cache: Option<&Arc<SampleCache>>,
        #[cfg(feature = "gpu")] gpu_synthesizer: Option<&Arc<GpuSynthesizer>>,
        prerendered: bool,
        allow_cache_render: bool,
    ) {
        // Clear output buffer
        buffer.fill(0.0);

        // Ensure events are sorted by start_time for binary search
        track.ensure_sorted();

        let track_start = track.start_time();
        let track_end = track.end_time();
        let time_delta = 1.0 / sample_rate;
        let block_duration = buffer.len() as f32 * time_delta;
        let block_end_time = start_time + block_duration;

        // Skip track entirely if we're completely outside its active range
        if (block_end_time < track_start || start_time > track_end)
            && track.effects.delay.is_none()
            && track.effects.reverb.is_none()
        {
            return;
        }

        // Binary search ONCE to find events that might be active during this block
        // We need to search at the start of the block
        let (start_idx, end_idx) = track.find_active_range_until(start_time, block_end_time);

        // Look up cached notes once per block; reuse flags during rendering.
        // No mutex overhead - concurrent cache access from Rayon threads!
        track.cached_note_flags.resize(end_idx - start_idx, false);
        track.cached_note_flags.fill(false);

        if let Some(cache_ref) = cache {
            // Special case: if pre-rendered, all notes are cached
            if prerendered {
                for (idx, event) in track.events[start_idx..end_idx].iter().enumerate() {
                    if let AudioEvent::Note(_) = event {
                        track.cached_note_flags[idx] = true;
                    }
                }
            }

            // Single pass through events: check misses, build indices, copy samples
            for (idx, event) in track.events[start_idx..end_idx].iter().enumerate() {
                if let AudioEvent::Note(note_event) = event {
                    let cache_key = CacheKey::from_note_event(note_event, sample_rate);

                    // Step 1: Handle cache miss (render if needed)
                    if allow_cache_render && !prerendered && cache_ref.get(&cache_key).is_none() {
                        let total_duration =
                            note_event.envelope.total_duration(note_event.duration);

                        // Only cache notes with reasonable duration
                        if total_duration > 0.0 && total_duration < 10.0 {
                            let rendered_samples = Self::render_note_to_buffer(
                                note_event,
                                sample_rate,
                                #[cfg(feature = "gpu")]
                                gpu_synthesizer,
                            );

                            let cached_sample = CachedSample::new(
                                rendered_samples,
                                sample_rate,
                                total_duration,
                                note_event.frequencies[0],
                            );
                            cache_ref.insert(cache_key.clone(), cached_sample);
                        }
                    }

                    // Step 2 & 3: If cached, build index AND copy to buffer
                    if let Some(cached_sample) = cache_ref.get(&cache_key) {
                        // Add to cached indices (for synthesis loop to skip)
                        if !prerendered {
                            track.cached_note_flags[idx] = true;
                        }

                        // Copy cached sample DIRECTLY to output buffer (SIMD-optimized)
                        let note_start = note_event.start_time;
                        let note_end = note_start + cached_sample.duration;

                        // Skip if note doesn't overlap with current block
                        if note_end >= start_time
                            && note_start < start_time + (buffer.len() as f32 / sample_rate)
                        {
                            // Compute sample ranges
                            let time_offset_in_note = (start_time - note_start).max(0.0);
                            let cache_start_sample = (time_offset_in_note * sample_rate) as usize;

                            let buffer_start_sample = if note_start > start_time {
                                ((note_start - start_time) * sample_rate) as usize
                            } else {
                                0
                            };

                            // Calculate how many samples to copy
                            let samples_remaining_in_cache = cached_sample
                                .samples
                                .len()
                                .saturating_sub(cache_start_sample);
                            let samples_remaining_in_buffer =
                                buffer.len().saturating_sub(buffer_start_sample);
                            let num_samples_to_copy =
                                samples_remaining_in_cache.min(samples_remaining_in_buffer);

                            // SIMD-optimized bulk copy
                            if num_samples_to_copy > 0
                                && cache_start_sample < cached_sample.samples.len()
                            {
                                use wide::f32x8;

                                let src = &cached_sample.samples[cache_start_sample..];
                                let dst = &mut buffer[buffer_start_sample..];

                                // SIMD-optimized bulk copy
                                let simd_chunks = num_samples_to_copy / 8;
                                let remainder = num_samples_to_copy % 8;

                                for chunk_idx in 0..simd_chunks {
                                    let i = chunk_idx * 8;
                                    let src_simd = f32x8::new(src[i..i + 8].try_into().unwrap());
                                    let dst_simd = f32x8::new(dst[i..i + 8].try_into().unwrap());
                                    let result = dst_simd + src_simd;
                                    dst[i..i + 8].copy_from_slice(&result.to_array());
                                }

                                // Handle remaining samples
                                let simd_end = simd_chunks * 8;
                                for i in simd_end..simd_end + remainder {
                                    dst[i] += src[i];
                                }
                            }
                        }
                    }
                }
            }
        }

        // Pre-render sample events with SIMD for better performance
        // This processes whole blocks instead of per-sample, enabling vectorization
        let has_samples = track.events[start_idx..end_idx]
            .iter()
            .any(|event| matches!(event, AudioEvent::Sample(_)));
        if has_samples {
            track.sample_scratch_buffer.resize(buffer.len(), 0.0);
            track.sample_scratch_buffer.fill(0.0);
        } else {
            track.sample_scratch_buffer.clear();
        }
        for event in &track.events[start_idx..end_idx] {
            if let AudioEvent::Sample(sample_event) = event {
                sample_event.sample.fill_buffer_simd_mono(
                    &mut track.sample_scratch_buffer,
                    sample_event.start_time,
                    start_time,
                    time_delta,
                    sample_event.playback_rate,
                    sample_event.volume,
                );
            }
        }

        // Render notes event-first: bounds, cache status and waveform dispatch are
        // determined once per event, rather than scanned for every output sample.
        for (relative_idx, event) in track.events[start_idx..end_idx].iter().enumerate() {
            if let AudioEvent::Note(note_event) = event {
                if track.cached_note_flags[relative_idx] {
                    continue;
                }
                let end =
                    note_event.start_time + note_event.envelope.total_duration(note_event.duration);
                let first =
                    frame_at_or_after(note_event.start_time, start_time, time_delta, buffer.len());
                let last = frame_at_or_after(end, start_time, time_delta, buffer.len());
                if first >= last {
                    continue;
                }
                let num_freqs = note_event.num_freqs;
                let can_vectorize =
                    note_event.fm_params.mod_index == 0.0 && note_event.custom_wavetable.is_none();
                if can_vectorize && num_freqs == 1 && note_event.pitch_bend_semitones == 0.0 {
                    render_simple_note(
                        note_event,
                        &mut buffer[first..last],
                        start_time,
                        first,
                        time_delta,
                    );
                    continue;
                }
                for (offset, sample_out) in buffer[first..last].iter_mut().enumerate() {
                    let time = start_time + (first + offset) as f32 * time_delta;
                    let time_in_note = time - note_event.start_time;
                    let envelope_amp = note_event
                        .envelope
                        .amplitude_at(time_in_note, note_event.duration);

                    // SIMD-optimized polyphonic frequency processing
                    // Process 8 frequencies at once for 3-6x speedup
                    use crate::synthesis::simd::SIMD;
                    use wide::{f32x4, f32x8};

                    // Pre-calculate pitch bend (hoisted out of frequency loop for efficiency)
                    let bend_multiplier = if note_event.pitch_bend_semitones != 0.0 {
                        let bend_progress = (time_in_note / note_event.duration).min(1.0);
                        2.0f32.powf((note_event.pitch_bend_semitones * bend_progress) / 12.0)
                    } else {
                        1.0
                    };

                    // Determine if we can use SIMD path (simple waveforms only)

                    if can_vectorize && SIMD.width() >= 8 && num_freqs >= 8 {
                        // SIMD path: process 8 frequencies at once
                        let chunks = num_freqs / 8;
                        let bend_simd = f32x8::splat(bend_multiplier);
                        let time_simd = f32x8::splat(time_in_note);

                        for chunk_idx in 0..chunks {
                            let base_idx = chunk_idx * 8;

                            // Load 8 base frequencies
                            let mut freq_array = [0.0f32; 8];
                            freq_array
                                .copy_from_slice(&note_event.frequencies[base_idx..base_idx + 8]);
                            let base_freqs = f32x8::from(freq_array);

                            // Apply pitch bend to all 8 frequencies at once (SIMD)
                            let freqs = base_freqs * bend_simd;

                            // Calculate 8 phases at once (SIMD multiplication - this is the win!)
                            let phases_raw = time_simd * freqs;
                            let phases = phases_raw.to_array();

                            // Sample waveform for each phase (wavetable lookups remain scalar)
                            for &phase in &phases {
                                let phase_wrapped = phase.fract(); // Wrap to [0, 1)
                                *sample_out +=
                                    note_event.waveform.sample(phase_wrapped) * envelope_amp;
                            }
                        }

                        // Scalar remainder
                        for freq_idx in (chunks * 8)..num_freqs {
                            let freq = note_event.frequencies[freq_idx] * bend_multiplier;
                            let phase = (time_in_note * freq) % 1.0;
                            *sample_out += note_event.waveform.sample(phase) * envelope_amp;
                        }
                    } else if can_vectorize && SIMD.width() >= 4 && num_freqs >= 4 {
                        // SSE path: process 4 frequencies at once
                        let chunks = num_freqs / 4;
                        let bend_simd = f32x4::splat(bend_multiplier);
                        let time_simd = f32x4::splat(time_in_note);

                        for chunk_idx in 0..chunks {
                            let base_idx = chunk_idx * 4;

                            // Load 4 base frequencies
                            let mut freq_array = [0.0f32; 4];
                            freq_array
                                .copy_from_slice(&note_event.frequencies[base_idx..base_idx + 4]);
                            let base_freqs = f32x4::from(freq_array);

                            // Apply pitch bend to all 4 frequencies at once (SIMD)
                            let freqs = base_freqs * bend_simd;

                            // Calculate 4 phases at once (SIMD multiplication - this is the win!)
                            let phases_raw = time_simd * freqs;
                            let phases = phases_raw.to_array();

                            // Sample waveform for each phase (wavetable lookups remain scalar)
                            for &phase in &phases {
                                let phase_wrapped = phase.fract(); // Wrap to [0, 1)
                                *sample_out +=
                                    note_event.waveform.sample(phase_wrapped) * envelope_amp;
                            }
                        }

                        // Scalar remainder
                        for freq_idx in (chunks * 4)..num_freqs {
                            let freq = note_event.frequencies[freq_idx] * bend_multiplier;
                            let phase = (time_in_note * freq) % 1.0;
                            *sample_out += note_event.waveform.sample(phase) * envelope_amp;
                        }
                    } else {
                        // Scalar fallback (FM synthesis, custom wavetables, or low polyphony)
                        for freq_idx in 0..num_freqs {
                            let freq = note_event.frequencies[freq_idx] * bend_multiplier;

                            let sample = if note_event.fm_params.mod_index > 0.0 {
                                note_event
                                    .fm_params
                                    .sample(freq, time_in_note, note_event.duration)
                            } else if let Some(ref wavetable) = note_event.custom_wavetable {
                                let phase = (time_in_note * freq) % 1.0;
                                wavetable.sample(phase)
                            } else {
                                let phase = (time_in_note * freq) % 1.0;
                                note_event.waveform.sample(phase)
                            };

                            *sample_out += sample * envelope_amp;
                        }
                    }
                }
            }
        }

        // Drum stealing only changes at a hit's start/end. Prepare these spans
        // once per block, retaining the old rule that an older long hit resumes
        // after a newer shorter hit ends. Equal-time hits still sum together.
        track.prepared_drums.clear();
        track.drum_boundaries.clear();

        for (index, event) in track.events[start_idx..end_idx].iter().enumerate() {
            if let AudioEvent::Drum(drum) = event {
                let ratio = 2.0_f32.powf(drum.pitch_offset / 12.0);
                let end = drum.start_time + drum.drum_type.duration() / ratio;
                let first =
                    frame_at_or_after(drum.start_time, start_time, time_delta, buffer.len());
                let last = frame_at_or_after(end, start_time, time_delta, buffer.len());
                if first < last {
                    track
                        .prepared_drums
                        .push((start_idx + index, first, last, ratio));
                    track.drum_boundaries.extend([first, last]);
                }
            }
        }
        if !track.prepared_drums.is_empty() {
            track.drum_boundaries.extend([0, buffer.len()]);
        }
        track.drum_boundaries.sort_unstable();
        track.drum_boundaries.dedup();
        for span in track.drum_boundaries.windows(2) {
            let (first, last) = (span[0], span[1]);
            track.drum_starts.clear();
            for &(index, begin, end, _) in &track.prepared_drums {
                if begin <= first && first < end {
                    let AudioEvent::Drum(drum) = &track.events[index] else {
                        unreachable!()
                    };
                    let latest = track.drum_starts.entry(drum.drum_type).or_insert(f32::MIN);
                    *latest = latest.max(drum.start_time);
                }
            }
            for &(index, begin, end, ratio) in &track.prepared_drums {
                if begin > first || first >= end {
                    continue;
                }
                let AudioEvent::Drum(drum) = &track.events[index] else {
                    unreachable!()
                };
                if track.drum_starts.get(&drum.drum_type) != Some(&drum.start_time) {
                    continue;
                }
                for (offset, sample_out) in buffer[first..last].iter_mut().enumerate() {
                    let time = start_time + (first + offset) as f32 * time_delta;
                    let sample_index = ((time - drum.start_time) * sample_rate * ratio) as usize;
                    *sample_out += drum.drum_type.sample(sample_index, sample_rate) * drum.velocity;
                }
            }
        }
        for (out, sample) in buffer.iter_mut().zip(&track.sample_scratch_buffer) {
            *out += sample;
        }

        // Apply track volume to entire buffer (SIMD-optimized, ~8x faster than per-sample!)
        if track.volume != 1.0 {
            SIMD.multiply_const(buffer, track.volume);
        }

        // Apply filter to entire buffer (optimized block processing!)
        track.filter.process_buffer(buffer, sample_rate);

        // Apply effects to entire buffer (block processing!)
        track
            .effects
            .process_mono_block(buffer, sample_rate, start_time, start_sample_count);
    }

    /// Process a single track and return its stereo output (static version)
    ///
    /// This is a helper method extracted from the main mixing loop.
    /// It handles event synthesis, filtering, effects, and panning for one track.
    pub(crate) fn process_track_static(
        track: &mut Track,
        time: f32,
        sample_rate: f32,
        sample_count: u64,
    ) -> (f32, f32) {
        // Ensure events are sorted by start_time for binary search
        track.ensure_sorted();

        // Quick time-bounds check: skip entire track if current time is outside its active range
        let track_start = track.start_time();
        let track_end = track.end_time();

        // Skip track entirely if we're before it starts or after it ends
        if (time < track_start || time > track_end)
            && track.effects.delay.is_none()
            && track.effects.reverb.is_none()
        {
            return (0.0, 0.0);
        }

        let mut track_value = 0.0;
        let mut has_active_event = false;

        // Binary search to find potentially active events
        let (start_idx, end_idx) = track.find_active_range(time);

        // Voice stealing: find the latest active drum for each type at this time
        // This prevents overlapping drums of the same type from stacking
        let mut latest_drum_starts: HashMap<DrumType, f32> = HashMap::new();
        for event in track.events[start_idx..end_idx].iter() {
            if let AudioEvent::Drum(drum_event) = event {
                let pitch_ratio = 2.0_f32.powf(drum_event.pitch_offset / 12.0);
                let drum_duration = drum_event.drum_type.duration() / pitch_ratio;
                if time >= drum_event.start_time && time < drum_event.start_time + drum_duration {
                    let entry = latest_drum_starts
                        .entry(drum_event.drum_type)
                        .or_insert(f32::MIN);
                    if drum_event.start_time > *entry {
                        *entry = drum_event.start_time;
                    }
                }
            }
        }

        // Process events
        for event in &track.events[start_idx..end_idx] {
            match event {
                AudioEvent::Note(note_event) => {
                    let total_duration = note_event.envelope.total_duration(note_event.duration);
                    let note_end_with_release = note_event.start_time + total_duration;

                    if time >= note_event.start_time && time < note_end_with_release {
                        has_active_event = true;
                        let time_in_note = time - note_event.start_time;
                        let envelope_amp = note_event
                            .envelope
                            .amplitude_at(time_in_note, note_event.duration);

                        for i in 0..note_event.num_freqs {
                            let base_freq = note_event.frequencies[i];

                            let freq = if note_event.pitch_bend_semitones != 0.0 {
                                let bend_progress = (time_in_note / note_event.duration).min(1.0);
                                let bend_multiplier = 2.0f32
                                    .powf((note_event.pitch_bend_semitones * bend_progress) / 12.0);
                                base_freq * bend_multiplier
                            } else {
                                base_freq
                            };

                            let sample = if note_event.fm_params.mod_index > 0.0 {
                                note_event
                                    .fm_params
                                    .sample(freq, time_in_note, note_event.duration)
                            } else if let Some(ref wavetable) = note_event.custom_wavetable {
                                let phase = (time_in_note * freq) % 1.0;
                                wavetable.sample(phase)
                            } else {
                                let phase = (time_in_note * freq) % 1.0;
                                note_event.waveform.sample(phase)
                            };

                            track_value += sample * envelope_amp;
                        }
                    }
                }
                AudioEvent::Drum(drum_event) => {
                    // Apply pitch offset: higher pitch = faster playback
                    let pitch_ratio = 2.0_f32.powf(drum_event.pitch_offset / 12.0);
                    let drum_duration = drum_event.drum_type.duration() / pitch_ratio;
                    if time >= drum_event.start_time && time < drum_event.start_time + drum_duration
                    {
                        // Voice stealing: only render if this is the most recent trigger
                        // This prevents overlapping drums of the same type from stacking
                        if latest_drum_starts.get(&drum_event.drum_type)
                            == Some(&drum_event.start_time)
                        {
                            has_active_event = true;
                            let time_in_drum = time - drum_event.start_time;
                            let sample_index = (time_in_drum * sample_rate * pitch_ratio) as usize;
                            track_value += drum_event.drum_type.sample(sample_index, sample_rate)
                                * drum_event.velocity;
                        }
                    }
                }
                AudioEvent::Sample(sample_event) => {
                    let time_in_sample = time - sample_event.start_time;
                    let sample_duration = sample_event.sample.duration / sample_event.playback_rate;

                    if time_in_sample >= 0.0 && time_in_sample < sample_duration {
                        has_active_event = true;
                        let (sample_left, sample_right) = sample_event
                            .sample
                            .sample_at_interpolated(time_in_sample, sample_event.playback_rate);
                        track_value += (sample_left + sample_right) * 0.5 * sample_event.volume;
                    }
                }
                _ => {} // Tempo/time/key signatures don't generate audio
            }
        }

        // Skip effect processing if track has no active events and no tail effects
        if !has_active_event && track.effects.delay.is_none() && track.effects.reverb.is_none() {
            return (0.0, 0.0);
        }

        // Apply track volume
        track_value *= track.volume;

        // Apply filter
        track_value = track.filter.process(track_value, sample_rate);

        // Apply effects through the unified effect chain
        track_value = track
            .effects
            .process_mono(track_value, sample_rate, time, sample_count);

        // Apply stereo panning using constant power panning (fast trig)
        let pan_angle = (track.pan + 1.0) * 0.25 * std::f32::consts::PI;
        let left_gain = pan_angle.fast_cos();
        let right_gain = pan_angle.fast_sin();

        (track_value * left_gain, track_value * right_gain)
    }
}

// Preserve the original per-sample clock arithmetic (no accumulating oscillator
// phase drift), but select the wavetable once and let it wrap phase only once.
fn render_simple_note(note: &NoteEvent, output: &mut [f32], start: f32, first: usize, step: f32) {
    let table = note.waveform.table();
    let frequency = note.frequencies[0];
    for (offset, sample) in output.iter_mut().enumerate() {
        let time = start + (first + offset) as f32 * step;
        let time_in_note = time - note.start_time;
        *sample += table.sample(time_in_note * frequency)
            * note.envelope.amplitude_at(time_in_note, note.duration);
    }
}

fn frame_at_or_after(time: f32, block_start: f32, step: f32, frames: usize) -> usize {
    let (mut low, mut high) = (0, frames);
    while low < high {
        let mid = low + (high - low) / 2;
        if block_start + mid as f32 * step < time {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

#[cfg(test)]
mod block_preparation_tests {
    use super::*;
    use crate::synthesis::{Envelope, Waveform};

    fn compare_to_scalar(mut track: Track) {
        const RATE: f32 = 4096.0;
        let mut reference = track.clone();
        track.prepare_realtime(67);
        let pan_gain = (std::f32::consts::PI * 0.25).fast_cos();
        for block in 0..90 {
            let mut actual = [0.0; 67];
            let start = block as f32 * 67.0 / RATE;
            Mixer::process_track_block(
                &mut track,
                &mut actual,
                RATE,
                start,
                block * 67,
                None,
                #[cfg(feature = "gpu")]
                None,
                false,
                false,
            );
            for (i, value) in actual.iter().enumerate() {
                let time = start + i as f32 / RATE;
                let expected =
                    Mixer::process_track_static(&mut reference, time, RATE, block * 67 + i as u64)
                        .0;
                assert!(
                    (value * pan_gain - expected).abs() < 2e-5,
                    "block {block}, frame {i}: {} != {expected}",
                    value * pan_gain
                );
            }
        }
    }

    #[test]
    fn simple_note_fast_path_preserves_waveforms_and_envelope_boundaries() {
        use crate::synthesis::envelope::EnvelopeCurve;
        for waveform in [
            Waveform::Sine,
            Waveform::Square,
            Waveform::Sawtooth,
            Waveform::Triangle,
        ] {
            for curve in [
                EnvelopeCurve::Linear,
                EnvelopeCurve::Exponential,
                EnvelopeCurve::Logarithmic,
            ] {
                let mut track = Track::new();
                track.add_note_with_waveform_and_envelope(
                    &[233.08188],
                    0.013,
                    0.08,
                    waveform,
                    Envelope::with_curve(0.005, 0.02, 0.6, 0.05, curve),
                );
                compare_to_scalar(track);
            }
        }
    }

    #[test]
    fn fractional_note_boundaries_and_overlapping_releases_match_scalar() {
        let mut track = Track::new();
        let envelope = Envelope::new(0.01, 0.02, 0.6, 0.18);
        track.add_note_with_waveform_and_envelope(
            &[80.0, 130.0],
            0.0131,
            0.31,
            Waveform::Sine,
            envelope,
        );
        track.add_note_with_waveform_and_envelope(
            &[190.0],
            0.1537,
            0.023,
            Waveform::Triangle,
            envelope,
        );
        compare_to_scalar(track);
    }

    #[test]
    fn pitched_drum_stealing_ties_and_older_hit_resumption_match_scalar() {
        use crate::track::events::DrumEvent;
        let mut track = Track::new();
        for (kind, start, pitch, velocity) in [
            (DrumType::Kick, 0.0, -12.0, 0.3),
            (DrumType::Kick, 0.1037, 24.0, 0.4),
            (DrumType::Kick, 0.1037, 24.0, 0.2),
            (DrumType::Snare, 0.0713, 0.0, 0.2),
        ] {
            track.events.push(AudioEvent::Drum(DrumEvent {
                drum_type: kind,
                start_time: start,
                pitch_offset: pitch,
                velocity,
                spatial_position: None,
            }));
        }
        track.invalidate_time_cache();
        compare_to_scalar(track);
    }

    #[test]
    fn sample_only_blocks_and_empty_future_ranges_match_scalar() {
        use crate::{synthesis::Sample, track::events::SampleEvent};
        let mut track = Track::new();
        track.events.push(AudioEvent::Sample(SampleEvent {
            sample: Sample::from_mono(vec![0.25; 1024], 4096),
            start_time: 0.0437,
            playback_rate: 0.8,
            volume: 0.7,
            spatial_position: None,
        }));
        track.invalidate_time_cache();
        compare_to_scalar(track);
    }
}
