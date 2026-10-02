//! Bounded foreground recovery policy, independent of browser APIs.

#[derive(Default)]
pub(crate) struct ForegroundReturn {
    hidden: bool,
    pending: bool,
}

impl ForegroundReturn {
    pub(crate) fn observe(&mut self, hidden: bool) {
        if self.hidden && !hidden {
            self.pending = true;
        }
        self.hidden = hidden;
    }

    pub(crate) fn take(&mut self) -> bool {
        if self.hidden {
            return false;
        }
        std::mem::take(&mut self.pending)
    }
}

#[derive(Default)]
pub(crate) struct Recovery {
    previous_time: f64,
    wait_first_tick: bool,
    attempts: u8,
    active: bool,
    exhausted: bool,
}

impl Recovery {
    /// A new foreground transition or user gesture permits another attempt.
    pub(crate) fn arm(&mut self, current_time: f64) {
        *self = Self {
            previous_time: current_time,
            wait_first_tick: true,
            active: true,
            ..Self::default()
        };
    }

    pub(crate) fn gesture(&mut self, current_time: f64) {
        if !self.active {
            self.arm(current_time);
        }
    }

    /// State changes during our own recovery must not reset the retry budget.
    pub(crate) fn interrupted(&mut self, current_time: f64) {
        if !self.active && !self.exhausted {
            self.arm(current_time);
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.active = false;
    }

    /// Called at 500 ms intervals. Returns true when a recovery is needed.
    pub(crate) fn check(&mut self, running: bool, current_time: f64) -> bool {
        if !self.active {
            return false;
        }
        // The shared interval might fire immediately after arm(). Give the
        // browser a full interval before declaring the audio clock frozen.
        if self.wait_first_tick {
            self.wait_first_tick = false;
            self.previous_time = current_time;
            return false;
        }
        if running && current_time > self.previous_time {
            self.active = false;
            self.exhausted = false;
            return false;
        }
        if self.attempts == 3 {
            self.active = false;
            self.exhausted = true;
            return false;
        }
        self.previous_time = current_time;
        self.attempts += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_return_is_once_per_hide_show_not_per_event() {
        let mut state = ForegroundReturn::default();
        state.observe(false); // initial pageshow
        assert!(!state.take());
        state.observe(true); // pagehide
        state.observe(true); // visibilitychange
        assert!(!state.take());
        state.observe(false); // visibilitychange
        assert!(state.take());
        state.observe(false); // pageshow arrives after the game frame
        assert!(!state.take());
        state.observe(true);
        state.observe(false);
        assert!(state.take());
    }

    #[test]
    fn rapid_hide_show_hide_defers_replacement_until_visible() {
        let mut state = ForegroundReturn::default();
        state.observe(true);
        state.observe(false);
        state.observe(true);
        assert!(!state.take());
        state.observe(false);
        assert!(state.take());
        assert!(!state.take());
    }

    #[test]
    fn running_with_a_frozen_clock_requires_recovery() {
        let mut recovery = Recovery::default();
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0));
        assert!(recovery.check(true, 10.0));
        assert!(!recovery.check(true, 10.5));
        assert!(!recovery.check(true, 10.5));
    }

    #[test]
    fn brief_progress_before_the_first_check_is_not_enough() {
        let mut recovery = Recovery::default();
        recovery.arm(10.0);
        assert!(!recovery.check(true, 10.1));
        assert!(recovery.check(true, 10.1));
    }

    #[test]
    fn repeated_gestures_do_not_delay_the_clock_check() {
        let mut recovery = Recovery::default();
        recovery.gesture(10.0);
        assert!(!recovery.check(true, 10.0));
        recovery.gesture(10.0);
        assert!(recovery.check(true, 10.0));
    }

    #[test]
    fn normal_resume_needs_no_cycle() {
        let mut recovery = Recovery::default();
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0));
        assert!(!recovery.check(true, 10.5));
    }

    #[test]
    fn state_events_cannot_create_an_endless_retry_loop() {
        let mut recovery = Recovery::default();
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0));
        for _ in 0..3 {
            recovery.interrupted(10.0);
            assert!(recovery.check(false, 10.0));
        }
        assert!(!recovery.check(false, 10.0));
        recovery.interrupted(10.0);
        assert!(!recovery.check(false, 10.0));
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0)); // Another gesture/foreground transition can retry.
        assert!(recovery.check(false, 10.0));
    }

    #[test]
    fn hiding_cancels_pending_recovery() {
        let mut recovery = Recovery::default();
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0));
        recovery.cancel();
        assert!(!recovery.check(false, 10.0));
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0));
        assert!(recovery.check(false, 10.0));
    }

    #[test]
    fn late_interruption_after_success_can_recover() {
        let mut recovery = Recovery::default();
        recovery.arm(10.0);
        assert!(!recovery.check(false, 10.0));
        assert!(!recovery.check(true, 10.5));
        recovery.interrupted(10.5);
        assert!(!recovery.check(false, 10.5));
        assert!(recovery.check(false, 10.5));
    }
}
