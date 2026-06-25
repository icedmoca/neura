//! Shared self-improvement prompt generation and scheduling policy.
//!
//! This module keeps the core self-improvement intent outside of the TUI slash
//! command layer so it can be reused by background/ambient automation.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

/// Minimum time between autonomous self-improvement jobs.
pub const DEFAULT_AUTONOMOUS_COOLDOWN: ChronoDuration = ChronoDuration::hours(12);

/// Session activity threshold that should trigger a background improvement pass.
pub const DEFAULT_LONG_SESSION_TURNS: u64 = 24;

/// Repair/debugging burst threshold that should trigger a background improvement pass.
pub const DEFAULT_REPAIR_BURST_COUNT: u64 = 3;

/// Memory pruning/compression activity threshold that should trigger a meta-cognitive pass.
pub const DEFAULT_MEMORY_MAINTENANCE_EVENTS: u64 = 5;

/// Mode for a self-improvement run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfImprovementMode {
    /// Inspect and plan only. Do not edit code.
    PlanOnly,
    /// Implement a bounded improvement, validate it, and commit it.
    Implement,
}

impl SelfImprovementMode {
    pub fn from_plan_only(plan_only: bool) -> Self {
        if plan_only {
            Self::PlanOnly
        } else {
            Self::Implement
        }
    }
}

/// Reason an autonomous self-improvement run was scheduled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomousTriggerReason {
    Cooldown,
    LongSession { turns: u64 },
    RepairBurst { repairs: u64 },
    MemoryMaintenance { events: u64 },
}

impl AutonomousTriggerReason {
    pub fn focus(&self) -> String {
        match self {
            Self::Cooldown => "autonomous background maintenance: pick one small, safe, high-confidence improvement".to_string(),
            Self::LongSession { turns } => format!(
                "post-session learning after a long interaction ({turns} turns): identify friction, missing automation, or reliability issues exposed by the session"
            ),
            Self::RepairBurst { repairs } => format!(
                "repair-pattern learning after {repairs} fixes: look for the smallest systemic improvement that prevents similar failures"
            ),
            Self::MemoryMaintenance { events } => format!(
                "meta-cognitive memory maintenance after {events} pruning/compression events: improve recall, pruning, compression, or context quality heuristics"
            ),
        }
    }
}

/// Signals that can trigger autonomous self-improvement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelfImprovementSignals {
    pub completed_session_turns: u64,
    pub repair_events: u64,
    pub memory_maintenance_events: u64,
}

/// Tunable thresholds for autonomous triggering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfImprovementPolicy {
    pub cooldown: ChronoDuration,
    pub long_session_turns: u64,
    pub repair_burst_count: u64,
    pub memory_maintenance_events: u64,
}

impl Default for SelfImprovementPolicy {
    fn default() -> Self {
        Self {
            cooldown: DEFAULT_AUTONOMOUS_COOLDOWN,
            long_session_turns: DEFAULT_LONG_SESSION_TURNS,
            repair_burst_count: DEFAULT_REPAIR_BURST_COUNT,
            memory_maintenance_events: DEFAULT_MEMORY_MAINTENANCE_EVENTS,
        }
    }
}

impl SelfImprovementPolicy {
    pub fn choose_reason(
        &self,
        state: &AutonomousSelfImprovementState,
        signals: SelfImprovementSignals,
        now: DateTime<Utc>,
    ) -> Option<AutonomousTriggerReason> {
        if !state.can_start(now, self.cooldown) {
            return None;
        }
        if signals.memory_maintenance_events >= self.memory_maintenance_events {
            return Some(AutonomousTriggerReason::MemoryMaintenance {
                events: signals.memory_maintenance_events,
            });
        }
        if signals.repair_events >= self.repair_burst_count {
            return Some(AutonomousTriggerReason::RepairBurst {
                repairs: signals.repair_events,
            });
        }
        if signals.completed_session_turns >= self.long_session_turns {
            return Some(AutonomousTriggerReason::LongSession {
                turns: signals.completed_session_turns,
            });
        }
        if state.last_started_at().is_none() {
            return Some(AutonomousTriggerReason::Cooldown);
        }
        None
    }
}

/// Request describing a self-improvement job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfImprovementRequest {
    pub mode: SelfImprovementMode,
    pub focus: Option<String>,
    pub trigger: SelfImprovementTrigger,
}

impl SelfImprovementRequest {
    pub fn manual(plan_only: bool, focus: Option<String>) -> Self {
        Self {
            mode: SelfImprovementMode::from_plan_only(plan_only),
            focus,
            trigger: SelfImprovementTrigger::Manual,
        }
    }

    pub fn autonomous(reason: AutonomousTriggerReason) -> Self {
        Self {
            mode: SelfImprovementMode::Implement,
            focus: Some(reason.focus()),
            trigger: SelfImprovementTrigger::Autonomous(reason),
        }
    }

    pub fn plan_only(&self) -> bool {
        matches!(self.mode, SelfImprovementMode::PlanOnly)
    }

    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }
}

/// Source of a self-improvement request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfImprovementTrigger {
    Manual,
    Autonomous(AutonomousTriggerReason),
}

/// Runtime state used to keep autonomous improvement conservative.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutonomousSelfImprovementState {
    last_started_at: Option<DateTime<Utc>>,
    in_flight: bool,
    pending_session_turns: u64,
    pending_repair_events: u64,
    pending_memory_maintenance_events: u64,
}

impl AutonomousSelfImprovementState {
    pub fn can_start(&self, now: DateTime<Utc>, cooldown: ChronoDuration) -> bool {
        if self.in_flight {
            return false;
        }
        match self.last_started_at {
            Some(last) => now.signed_duration_since(last) >= cooldown,
            None => true,
        }
    }

    pub fn record_session_turns(&mut self, turns: u64) {
        self.pending_session_turns = self.pending_session_turns.saturating_add(turns);
    }

    pub fn record_repair_event(&mut self) {
        self.pending_repair_events = self.pending_repair_events.saturating_add(1);
    }

    pub fn record_memory_maintenance_event(&mut self) {
        self.pending_memory_maintenance_events =
            self.pending_memory_maintenance_events.saturating_add(1);
    }

    pub fn signals(&self) -> SelfImprovementSignals {
        SelfImprovementSignals {
            completed_session_turns: self.pending_session_turns,
            repair_events: self.pending_repair_events,
            memory_maintenance_events: self.pending_memory_maintenance_events,
        }
    }

    pub fn mark_started(&mut self, now: DateTime<Utc>) {
        self.last_started_at = Some(now);
        self.in_flight = true;
        self.pending_session_turns = 0;
        self.pending_repair_events = 0;
        self.pending_memory_maintenance_events = 0;
    }

    pub fn mark_finished(&mut self) {
        self.in_flight = false;
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    pub fn last_started_at(&self) -> Option<DateTime<Utc>> {
        self.last_started_at
    }
}

/// Build the self-improvement prompt shared by `/improve` and background jobs.
pub fn build_self_improvement_prompt(request: &SelfImprovementRequest) -> String {
    let plan_only = request.plan_only();
    let focus = request.focus();
    let mut prompt = String::new();
    if plan_only {
        prompt.push_str(
            "Plan a concrete Neura code-quality improvement. Do not modify files yet.\n\n",
        );
    } else {
        prompt.push_str(
            "Improve Neura autonomously with one focused, high-impact code-quality change.\n\n",
        );
    }
    if let SelfImprovementTrigger::Autonomous(reason) = &request.trigger {
        prompt.push_str(
            "This run was started automatically in the background. Be conservative: avoid user-visible behavior changes unless clearly beneficial, keep the patch small, and stop if the repo is dirty with unrelated work.\n\n",
        );
        prompt.push_str("Autonomous trigger: ");
        prompt.push_str(&format!("{reason:?}"));
        prompt.push_str("\n\n");
    }
    if let Some(focus) = focus {
        prompt.push_str("Focus area: ");
        prompt.push_str(focus.trim());
        prompt.push_str("\n\n");
    }
    prompt.push_str(
        "Workflow:\n\
         1. Inspect the repository state and relevant code before editing.\n\
         2. Pick exactly one small improvement that reduces complexity, removes duplication, fixes a correctness issue, improves tests, or strengthens maintainability.\n\
         3. Avoid cosmetic churn and broad rewrites.\n\
         4. Preserve existing public behavior unless the change is an obvious bug fix.\n\
         5. Run targeted validation, then broader validation if practical.\n\
         6. Commit only your changes with a concise message.\n",
    );
    if plan_only {
        prompt.push_str(
            "\nReturn a short implementation plan with files to touch, risks, and validation steps. Do not edit or commit.\n",
        );
    } else {
        prompt.push_str(
            "\nAfter validation, commit the completed improvement. If validation fails, fix it or revert your own changes before stopping.\n",
        );
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autonomous_state_respects_cooldown_and_in_flight() {
        let mut state = AutonomousSelfImprovementState::default();
        let now = Utc::now();
        assert!(state.can_start(now, DEFAULT_AUTONOMOUS_COOLDOWN));

        state.mark_started(now);
        assert!(state.in_flight());
        assert!(!state.can_start(now + ChronoDuration::hours(24), DEFAULT_AUTONOMOUS_COOLDOWN));

        state.mark_finished();
        assert!(!state.can_start(now + ChronoDuration::hours(1), DEFAULT_AUTONOMOUS_COOLDOWN));
        assert!(state.can_start(now + ChronoDuration::hours(13), DEFAULT_AUTONOMOUS_COOLDOWN));
    }

    #[test]
    fn policy_prioritizes_memory_then_repairs_then_long_sessions() {
        let state = AutonomousSelfImprovementState::default();
        let policy = SelfImprovementPolicy::default();
        let now = Utc::now();

        let reason = policy.choose_reason(
            &state,
            SelfImprovementSignals {
                completed_session_turns: DEFAULT_LONG_SESSION_TURNS,
                repair_events: DEFAULT_REPAIR_BURST_COUNT,
                memory_maintenance_events: DEFAULT_MEMORY_MAINTENANCE_EVENTS,
            },
            now,
        );
        assert!(matches!(
            reason,
            Some(AutonomousTriggerReason::MemoryMaintenance { .. })
        ));
    }

    #[test]
    fn signals_are_cleared_when_autonomous_run_starts() {
        let mut state = AutonomousSelfImprovementState::default();
        state.record_session_turns(DEFAULT_LONG_SESSION_TURNS);
        state.record_repair_event();
        state.record_memory_maintenance_event();
        assert!(state.signals().completed_session_turns > 0);

        state.mark_started(Utc::now());
        assert_eq!(state.signals(), SelfImprovementSignals::default());
    }

    #[test]
    fn autonomous_prompt_is_conservative_and_meta_cognitive_for_memory() {
        let prompt = build_self_improvement_prompt(&SelfImprovementRequest::autonomous(
            AutonomousTriggerReason::MemoryMaintenance { events: 5 },
        ));
        assert!(prompt.contains("automatically in the background"));
        assert!(prompt.contains("Be conservative"));
        assert!(prompt.contains("meta-cognitive memory maintenance"));
        assert!(prompt.contains("pruning") || prompt.contains("compression"));
    }
}
