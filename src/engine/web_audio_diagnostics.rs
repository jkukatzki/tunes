//! Measure CPAL's main-thread scheduling separately from Rust callback work.

pub(super) fn now_ms() -> Option<f64> {
    web_sys::window()?.performance().map(|clock| clock.now())
}

#[derive(Default)]
pub(super) struct CallbackDiagnostics {
    last_report_ms: f64,
    callbacks: u32,
    late_callbacks: u32,
    missed_deadlines: u32,
    max_lateness_ms: f64,
    max_render_ms: f64,
}

impl CallbackDiagnostics {
    pub(super) fn record(
        &mut self,
        started: Option<f64>,
        info: &cpal::OutputCallbackInfo,
        frames: usize,
        sample_rate: f32,
    ) {
        let (Some(started), Some(now)) = (started, now_ms()) else {
            return;
        };
        let render_ms = now - started;
        let timestamp = info.timestamp();
        let headroom_ms = match timestamp.playback.duration_since(&timestamp.callback) {
            Some(duration) => duration.as_secs_f64() * 1000.0,
            None => {
                -timestamp
                    .callback
                    .duration_since(&timestamp.playback)
                    .unwrap_or_default()
                    .as_secs_f64()
                    * 1000.0
            }
        };
        self.callbacks += 1;
        self.late_callbacks += u32::from(headroom_ms < 0.0);
        self.missed_deadlines += u32::from(render_ms > headroom_ms);
        self.max_lateness_ms = self.max_lateness_ms.max(-headroom_ms);
        self.max_render_ms = self.max_render_ms.max(render_ms);

        // Avoid per-buffer console traffic, which itself can cause dropouts.
        if now - self.last_report_ms < 5000.0 {
            return;
        }
        if self.missed_deadlines > 0 {
            web_sys::console::warn_1(&format!(
                "[tunes] Audio timing: callbacks={}, late_at_entry={}, missed_deadlines={}, max_lateness={:.1}ms, max_render={:.1}ms, buffer={:.1}ms. Late entry indicates browser scheduling delay; render time measures callback work. Background/resume can also cause late entry.",
                self.callbacks, self.late_callbacks, self.missed_deadlines,
                self.max_lateness_ms, self.max_render_ms,
                frames as f64 / sample_rate as f64 * 1000.0,
            ).into());
        }
        *self = Self {
            last_report_ms: now,
            ..Self::default()
        };
    }
}
