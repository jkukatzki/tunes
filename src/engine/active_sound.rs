//! Active sound state for playing sounds.
//!
//! Contains state for sounds that are currently playing in the audio engine.

use crate::synthesis::spatial::{SoundCone, SpatialPosition};
use crate::track::Mixer;

/// State for an actively playing sound
pub(crate) struct ActiveSound {
    pub mixer: Mixer,
    pub duration: f32,
    pub sample_clock: f32,
    pub elapsed_time: f32,
    /// Output time, independent of playback speed and source looping.
    pub control_time: f64,
    pub volume: f32,
    pub pan: f32,
    pub playback_rate: f32, // 1.0 = normal, 2.0 = double speed/pitch
    pub paused: bool,
    pub looping: bool,
    pub spatial_position: Option<SpatialPosition>, // 3D position for spatial audio
    pub spatial_cone: Option<SoundCone>,           // Optional directional cone
    pub occlusion: f32, // Occlusion amount (0.0 = none, 1.0 = fully occluded)
    // Spatial audio caching (avoid recalculating every frame)
    pub cached_spatial_volume: f32,
    pub cached_spatial_pan: f32,
    pub cached_spatial_pitch: f32,
    pub spatial_dirty: bool, // True when position/cone/occlusion changed
    // Volume fade state
    pub fade_start_time: Option<f64>,
    pub fade_duration: f32,
    pub fade_start_volume: f32,
    pub fade_target_volume: f32,
    pub stop_after_fade: bool,
    // Pan tween state
    pub pan_tween_start_time: Option<f64>,
    pub pan_tween_duration: f32,
    pub pan_tween_start_value: f32,
    pub pan_tween_target_value: f32,
    // Playback rate tween state
    pub rate_tween_start_time: Option<f64>,
    pub rate_tween_duration: f32,
    pub rate_tween_start_value: f32,
    pub rate_tween_target_value: f32,
}

impl ActiveSound {
    /// Create a new active sound from a mixer
    pub fn new(mixer: Mixer, looping: bool) -> Self {
        let duration = if looping {
            mixer.total_duration()
        } else {
            mixer.playback_duration()
        };
        Self {
            mixer,
            duration,
            sample_clock: 0.0,
            elapsed_time: 0.0,
            control_time: 0.0,
            volume: 1.0,
            pan: 0.0,
            playback_rate: 1.0,
            paused: false,
            looping,
            spatial_position: None,
            spatial_cone: None,
            occlusion: 0.0,
            // Initialize spatial cache (will be calculated on first frame)
            cached_spatial_volume: 1.0,
            cached_spatial_pan: 0.0,
            cached_spatial_pitch: 1.0,
            spatial_dirty: true, // Force calculation on first frame
            fade_start_time: None,
            fade_duration: 0.0,
            fade_start_volume: 1.0,
            fade_target_volume: 1.0,
            stop_after_fade: false,
            pan_tween_start_time: None,
            pan_tween_duration: 0.0,
            pan_tween_start_value: 0.0,
            pan_tween_target_value: 0.0,
            rate_tween_start_time: None,
            rate_tween_duration: 0.0,
            rate_tween_start_value: 1.0,
            rate_tween_target_value: 1.0,
        }
    }

    pub fn volume_at(&self, time: f64) -> f32 {
        let Some(start) = self.fade_start_time else {
            return self.volume;
        };
        let progress = if self.fade_duration > 0.0 {
            ((time - start) / self.fade_duration as f64).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        self.fade_start_volume + (self.fade_target_volume - self.fade_start_volume) * progress
    }

    pub fn start_fade(&mut self, duration: f32, target: f32, stop: bool) {
        let duration = if duration.is_finite() {
            duration.max(0.0)
        } else {
            0.0
        };
        // Repeated note-offs must not prolong a fade already ending sooner.
        if stop && self.stop_after_fade {
            if let Some(start) = self.fade_start_time {
                if start + self.fade_duration as f64 <= self.control_time + duration as f64 {
                    return;
                }
            }
        }
        self.volume = self.volume_at(self.control_time);
        self.fade_start_volume = self.volume;
        self.fade_start_time = Some(self.control_time);
        self.fade_duration = duration;
        self.fade_target_volume = target;
        self.stop_after_fade = stop;
    }

    /// Publish the gain at the next block boundary and report a completed stop.
    pub fn update_fade(&mut self) -> bool {
        self.volume = self.volume_at(self.control_time);
        if let Some(start) = self.fade_start_time {
            if self.control_time - start >= self.fade_duration as f64 {
                self.volume = self.fade_target_volume;
                self.fade_start_time = None;
                return self.stop_after_fade;
            }
        }
        false
    }
}
