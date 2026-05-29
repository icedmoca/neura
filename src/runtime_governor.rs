//! Closed-loop runtime governor.
//!
//! The governor turns recent observations (latency samples, queue pressure, and
//! interactive input/stream activity) into conservative policy recommendations.
//! It is deliberately small and deterministic so subsystems can adopt it without
//! changing core model/tool semantics.

use crate::latency::{self, LatencyKind, LatencySummary};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Normal,
    InteractiveProtect,
    Backpressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GovernorPolicy {
    pub mode: RuntimeMode,
    pub force_stream_chunking: bool,
    pub defer_low_priority_work: bool,
    pub active_turn_tick_ms: u64,
}

impl Default for GovernorPolicy {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Normal,
            force_stream_chunking: false,
            defer_low_priority_work: false,
            active_turn_tick_ms: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GovernorObservation {
    pub recent_key_events: u32,
    pub recent_stream_events: u32,
    pub backend_queue_len: usize,
    pub backend_queue_capacity: usize,
}

#[derive(Debug, Clone)]
pub struct RuntimeGovernor {
    policy: GovernorPolicy,
}

impl RuntimeGovernor {
    pub fn new() -> Self {
        Self {
            policy: GovernorPolicy::default(),
        }
    }

    pub fn policy(&self) -> GovernorPolicy {
        self.policy
    }

    pub fn evaluate(
        &mut self,
        summaries: &[LatencySummary],
        observation: GovernorObservation,
    ) -> GovernorPolicy {
        let slow_render = summary_is_slow(summaries, LatencyKind::TuiRender, 24);
        let slow_stream = summary_is_slow(summaries, LatencyKind::StreamAppend, 16)
            || summary_is_slow(summaries, LatencyKind::StreamFlush, 16);
        let slow_session_save = summary_is_slow(summaries, LatencyKind::SessionSave, 75);
        let queue_pressure = observation.backend_queue_capacity > 0
            && observation.backend_queue_len * 100 / observation.backend_queue_capacity >= 75;
        let typing_during_stream =
            observation.recent_key_events > 0 && observation.recent_stream_events > 0;

        self.policy = if queue_pressure || slow_session_save {
            GovernorPolicy {
                mode: RuntimeMode::Backpressure,
                force_stream_chunking: true,
                defer_low_priority_work: true,
                active_turn_tick_ms: 150,
            }
        } else if typing_during_stream || slow_render || slow_stream {
            GovernorPolicy {
                mode: RuntimeMode::InteractiveProtect,
                force_stream_chunking: true,
                defer_low_priority_work: false,
                active_turn_tick_ms: 125,
            }
        } else {
            GovernorPolicy::default()
        };

        self.policy
    }
}

impl Default for RuntimeGovernor {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RollingObservation {
    recent_key_events: u32,
    recent_stream_events: u32,
    backend_queue_len: usize,
    backend_queue_capacity: usize,
}

#[derive(Debug, Default)]
struct GovernorState {
    governor: RuntimeGovernor,
    observation: RollingObservation,
}

static GLOBAL_GOVERNOR: OnceLock<Mutex<GovernorState>> = OnceLock::new();

fn global_state() -> &'static Mutex<GovernorState> {
    GLOBAL_GOVERNOR.get_or_init(|| Mutex::new(GovernorState::default()))
}

pub fn observe_key_event() {
    if let Ok(mut state) = global_state().lock() {
        state.observation.recent_key_events = state
            .observation
            .recent_key_events
            .saturating_add(1)
            .min(1_000);
    }
}

pub fn observe_stream_event() {
    if let Ok(mut state) = global_state().lock() {
        state.observation.recent_stream_events = state
            .observation
            .recent_stream_events
            .saturating_add(1)
            .min(10_000);
    }
}

pub fn observe_backend_queue(len: usize, capacity: usize) {
    if let Ok(mut state) = global_state().lock() {
        state.observation.backend_queue_len = len;
        state.observation.backend_queue_capacity = capacity;
    }
}

pub fn evaluate_global() -> GovernorPolicy {
    if let Ok(mut state) = global_state().lock() {
        let observation = GovernorObservation {
            recent_key_events: state.observation.recent_key_events,
            recent_stream_events: state.observation.recent_stream_events,
            backend_queue_len: state.observation.backend_queue_len,
            backend_queue_capacity: state.observation.backend_queue_capacity,
        };
        // Decay event counters each evaluation window so short bursts influence
        // policy without permanently locking the runtime into protective modes.
        state.observation.recent_key_events /= 2;
        state.observation.recent_stream_events /= 2;
        state.governor.evaluate(&latency::summaries(), observation)
    } else {
        GovernorPolicy::default()
    }
}

pub fn current_policy() -> GovernorPolicy {
    global_state()
        .lock()
        .map(|state| state.governor.policy())
        .unwrap_or_default()
}

pub fn format_policy(policy: GovernorPolicy) -> String {
    format!(
        "runtime governor: mode={:?} stream_chunking={} defer_low_priority={} active_tick_ms={}",
        policy.mode,
        policy.force_stream_chunking,
        policy.defer_low_priority_work,
        policy.active_turn_tick_ms
    )
}

pub fn format_current_policy() -> String {
    format_policy(current_policy())
}

fn summary_is_slow(summaries: &[LatencySummary], kind: LatencyKind, avg_ms: u64) -> bool {
    summaries.iter().any(|summary| {
        summary.kind == kind
            && summary.count > 0
            && (summary.average() >= Duration::from_millis(avg_ms) || summary.slow_count > 0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(kind: LatencyKind, avg_ms: u64, slow_count: usize) -> LatencySummary {
        LatencySummary {
            kind,
            count: 1,
            slow_count,
            total: Duration::from_millis(avg_ms),
            max: Duration::from_millis(avg_ms),
        }
    }

    #[test]
    fn protects_interactivity_when_typing_during_stream() {
        let mut governor = RuntimeGovernor::new();
        let policy = governor.evaluate(
            &[],
            GovernorObservation {
                recent_key_events: 1,
                recent_stream_events: 8,
                backend_queue_len: 0,
                backend_queue_capacity: 16,
            },
        );
        assert_eq!(policy.mode, RuntimeMode::InteractiveProtect);
        assert!(policy.force_stream_chunking);
        assert!(!policy.defer_low_priority_work);
    }

    #[test]
    fn enters_backpressure_for_slow_session_save() {
        let mut governor = RuntimeGovernor::new();
        let policy = governor.evaluate(
            &[summary(LatencyKind::SessionSave, 90, 1)],
            GovernorObservation::default(),
        );
        assert_eq!(policy.mode, RuntimeMode::Backpressure);
        assert!(policy.defer_low_priority_work);
    }

    #[test]
    fn stays_normal_without_pressure() {
        let mut governor = RuntimeGovernor::new();
        let policy = governor.evaluate(&[], GovernorObservation::default());
        assert_eq!(policy, GovernorPolicy::default());
    }

    #[test]
    fn global_policy_reacts_to_observations() {
        observe_key_event();
        observe_stream_event();
        let policy = evaluate_global();
        assert_eq!(policy.mode, RuntimeMode::InteractiveProtect);
        assert!(current_policy().force_stream_chunking);
    }

    #[test]
    fn formats_policy() {
        let text = format_policy(GovernorPolicy::default());
        assert!(text.contains("runtime governor"));
        assert!(text.contains("mode=Normal"));
    }
}
