//! Final, channel-linked peak protection after all sounds and streams are summed.

pub(crate) struct OutputLimiter {
    gain: f32,
}

impl OutputLimiter {
    pub fn new() -> Self {
        Self { gain: 1.0 }
    }

    pub fn process(&mut self, output: &mut [f32], channels: usize, sample_rate: f32) {
        // About -0.3 dB of headroom, instant attack, 50 ms release.
        const CEILING: f32 = 0.966_050_86;
        let release = (-1.0 / (0.05 * sample_rate)).exp();
        for frame in output.chunks_mut(channels) {
            let mut peak = 0.0_f32;
            for sample in frame.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
                peak = peak.max(sample.abs());
            }
            let target = if peak > CEILING { CEILING / peak } else { 1.0 };
            self.gain = if target < self.gain {
                target
            } else {
                target + release * (self.gain - target)
            };
            for sample in frame {
                *sample *= self.gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summed_peaks_are_limited_without_changing_stereo_balance() {
        let mut limiter = OutputLimiter::new();
        let mut samples = [4.0, 2.0, 0.5, 0.25];
        limiter.process(&mut samples, 2, 48_000.0);
        assert!(samples.iter().all(|s| s.abs() < 0.967));
        assert_eq!(samples[0], samples[1] * 2.0);
        assert_eq!(samples[2], samples[3] * 2.0);
        assert!(samples[2] < 0.13); // Gain recovers gradually after the peak.
    }

    #[test]
    fn invalid_samples_do_not_poison_following_audio() {
        let mut limiter = OutputLimiter::new();
        let mut samples = [f32::NAN, f32::INFINITY, 0.1, -0.1];
        limiter.process(&mut samples, 2, 48_000.0);
        assert_eq!(samples, [0.0, 0.0, 0.1, -0.1]);
    }
}
