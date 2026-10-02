// Match the backend's existing startup headroom. Only add this after a missed
// deadline; adding it on every callback would introduce gaps between buffers.
const RECOVERY_HEADROOM_SECS: f64 = 0.025;

pub(crate) fn worker_stalled(has_source: bool, end: f64, now: f64) -> bool {
    has_source && now > end + 0.1
}

pub(crate) fn accept_callback(has_source: bool, ended_event: bool, same_source: bool) -> bool {
    if has_source {
        ended_event && same_source
    } else {
        !ended_event
    }
}

pub(crate) fn next_start(cursor: f64, now: f64) -> f64 {
    if cursor <= now {
        now + RECOVERY_HEADROOM_SECS
    } else {
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_ended_event_can_be_recovered_after_grace_period() {
        assert!(!worker_stalled(true, 10.0, 9.0));
        assert!(!worker_stalled(true, 10.0, 10.05));
        assert!(worker_stalled(true, 10.0, 10.2));
        assert!(!worker_stalled(false, 0.0, 10.2));
    }

    #[test]
    fn queued_old_events_and_duplicate_starts_cannot_fork_a_restarted_chain() {
        assert!(
            accept_callback(false, false, false),
            "start an empty worker"
        );
        assert!(
            !accept_callback(true, false, false),
            "ignore duplicate timer/start"
        );
        assert!(
            !accept_callback(true, true, false),
            "ignore replaced source event"
        );
        assert!(
            accept_callback(true, true, true),
            "current source continues chain"
        );
        assert!(
            !accept_callback(false, true, false),
            "ignore event after source removed"
        );
    }

    #[test]
    fn initial_buffer_gets_headroom() {
        assert_eq!(next_start(0.0, 0.0), 0.025);
        assert_eq!(next_start(0.0, 12.0), 12.025);
    }

    #[test]
    fn recovers_from_reported_sixteen_second_backlog() {
        assert_eq!(next_start(4.0, 20.0), 20.025);
    }

    #[test]
    fn exact_deadline_needs_headroom() {
        assert_eq!(next_start(20.0, 20.0), 20.025);
    }

    #[test]
    fn preserves_future_start_even_with_little_headroom() {
        assert_eq!(next_start(20.001, 20.0), 20.001);
    }

    #[test]
    fn rechecks_deadline_after_slow_render() {
        let before_render = next_start(0.0, 20.0);
        assert_eq!(next_start(before_render, 20.030), 20.055);
    }

    #[test]
    fn both_workers_remain_contiguous_after_recovery() {
        let duration = 1024.0 / 44100.0;
        let first = next_start(4.0, 20.0);
        let mut cursor = first + duration;
        // Alternating worker callbacks arrive as the previous buffer ends.
        for index in 0..1000 {
            let now = first + index as f64 * duration + 0.003;
            let start = next_start(cursor, now);
            assert_eq!(start, cursor, "healthy playback must not add gaps");
            cursor = start + duration;
        }
    }
}
