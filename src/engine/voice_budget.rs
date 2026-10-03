//! Engine-wide admission policy. Lower priority releases are stolen first.
use super::{active_sound::ActiveSound, commands::SoundId, sound_pool::SoundPool};

pub(crate) const MAX_VOICES: usize = 96;
pub(crate) const RETIRING_VOICES: usize = 8;

/// Relative importance when the engine reaches its voice budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "worker", derive(serde::Serialize, serde::Deserialize))]
pub enum VoicePriority {
    Incidental,
    #[default]
    Normal,
    Important,
}

#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(feature = "worker", derive(serde::Serialize, serde::Deserialize))]
pub(crate) struct VoiceOptions {
    pub priority: VoicePriority,
    pub group: Option<u64>,
    pub max_instances: usize,
}

pub(crate) fn admit(
    sounds: &mut SoundPool<ActiveSound>,
    options: VoiceOptions,
    streams: usize,
) -> bool {
    let mut playing = 0;
    let mut group_count = 0;
    let mut candidate: Option<(SoundId, VoicePriority, bool, f32)> = None;
    for (id, sound) in sounds.iter_mut() {
        if !sound.stolen {
            playing += 1;
        }
        if options.group.is_some() && sound.options.group == options.group && !sound.stolen {
            group_count += 1;
        }
        if sound.stolen || sound.options.priority > options.priority {
            continue;
        }
        let gain = if sound.paused {
            0.0
        } else {
            sound.volume_at(sound.control_time).abs()
                * sound.cached_spatial_volume.abs()
                * sound.source.gain_hint()
        };
        let key = (sound.options.priority, !sound.stop_after_fade, gain);
        if candidate
            .as_ref()
            .is_none_or(|(_, p, held, g)| key < (*p, *held, *g))
        {
            candidate = Some((*id, key.0, key.1, key.2));
        }
    }
    if options.max_instances > 0 && group_count >= options.max_instances {
        return false;
    }
    if playing + streams < MAX_VOICES && sounds.len() + streams < MAX_VOICES + RETIRING_VOICES {
        return true;
    }
    if sounds.len() + streams >= MAX_VOICES + RETIRING_VOICES {
        return false;
    }
    if let Some((id, _, _, _)) = candidate {
        let victim = sounds.get_mut(id).unwrap();
        victim.stolen = true;
        victim.paused = false;
        victim.start_fade(0.005, 0.0, true);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{composition::Tempo, track::Mixer};
    fn voice(priority: VoicePriority, gain: f32) -> ActiveSound {
        let mut v = ActiveSound::new(Mixer::new(Tempo::new(120.0)), true);
        v.options.priority = priority;
        v.volume = gain;
        v
    }
    #[test]
    fn important_attack_steals_quiet_release_with_a_fade() {
        let mut sounds = SoundPool::with_capacity(128);
        for id in 1..=MAX_VOICES as u64 {
            sounds.insert(id, voice(VoicePriority::Normal, 1.0));
        }
        let quiet = sounds.get_mut(2).unwrap();
        quiet.start_fade(1.0, 0.0, true);
        quiet.control_time = 0.9;
        assert!(admit(
            &mut sounds,
            VoiceOptions {
                priority: VoicePriority::Important,
                ..Default::default()
            },
            0
        ));
        let quiet = sounds.get_mut(2).unwrap();
        assert!(quiet.stolen && quiet.stop_after_fade);
        assert_eq!(quiet.fade_duration, 0.005);
        assert!((quiet.fade_start_volume - 0.1).abs() < 1e-6);
    }
    #[test]
    fn incidental_cannot_steal_important_and_retiring_pool_is_bounded() {
        let mut sounds = SoundPool::with_capacity(128);
        for id in 1..=MAX_VOICES as u64 {
            sounds.insert(id, voice(VoicePriority::Important, 1.0));
        }
        assert!(!admit(
            &mut sounds,
            VoiceOptions {
                priority: VoicePriority::Incidental,
                ..Default::default()
            },
            0
        ));
        for id in MAX_VOICES..MAX_VOICES + RETIRING_VOICES {
            assert!(admit(
                &mut sounds,
                VoiceOptions {
                    priority: VoicePriority::Important,
                    ..Default::default()
                },
                0
            ));
            sounds.insert(id as u64 + 1, voice(VoicePriority::Important, 1.0));
        }
        assert!(!admit(
            &mut sounds,
            VoiceOptions {
                priority: VoicePriority::Important,
                ..Default::default()
            },
            0
        ));
    }
    #[test]
    fn repeated_groups_and_native_streams_share_the_budget() {
        let mut sounds = SoundPool::with_capacity(128);
        let options = VoiceOptions {
            group: Some(7),
            max_instances: 4,
            ..Default::default()
        };
        for id in 1..=4 {
            let mut v = voice(VoicePriority::Normal, 1.0);
            v.options = options;
            sounds.insert(id, v);
        }
        assert!(!admit(&mut sounds, options, 0));
        assert!(admit(&mut sounds, VoiceOptions::default(), MAX_VOICES - 4));
        assert!(sounds.iter_mut().any(|(_, v)| v.stolen));
    }
}
