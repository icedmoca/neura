//! Shared scheduling policy for interactive TUI work.
//!
//! The TUI receives terminal input, model/backend stream events, and redraw
//! ticks on latency-sensitive paths. Keeping the timing knobs in one place makes
//! it harder for backend stream traffic to accidentally degrade typing latency.

use std::time::Duration;

/// Minimum time between redraw ticks while a model turn is actively streaming.
pub(crate) const ACTIVE_TURN_REDRAW_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum time to hold tiny streaming text fragments before flushing them.
pub(crate) const STREAM_CHUNK_TIMEOUT: Duration = Duration::from_millis(250);

/// Byte threshold that forces streaming text to flush even without a natural
/// boundary. This bounds perceived latency while still coalescing token bursts.
pub(crate) const STREAM_CHUNK_MAX_BYTES: usize = 500;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_policy_keeps_interactive_bounds() {
        assert!(ACTIVE_TURN_REDRAW_INTERVAL <= Duration::from_millis(100));
        assert!(STREAM_CHUNK_TIMEOUT <= Duration::from_millis(250));
        assert!(STREAM_CHUNK_MAX_BYTES >= 256);
    }
}
