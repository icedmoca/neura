//! Shared self-improvement prompt generation and scheduling policy.
//!
//! This module keeps the core self-improvement intent outside of the TUI slash
//! command layer so it can be reused by background/ambient automation.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

/// Minimum time between autonomous self-improvement jobs.
pub const DEFAULT_AUTONOMOUS_COOLDOWN: ChronoDuration = ChronoDuration::hours(12);

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

    pub fn autonomous() -> Self {
        Self {
            mode: SelfImprovementMode::Implement,
            focus: Some(
                "autonomous background maintenance: pick one small, safe, high-confidence improvement"
                    .to_string(),
            ),
            trigger: SelfImprovementTrigger::Autonomous,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfImprovementTrigger {
    Manual,
    Autonomous,
}

/// Runtime state used to keep autonomous improvement conservative.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutonomousSelfImprovementState {
    last_started_at: Option<DateTime<Utc>>,
    in_flight: bool,
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

    pub fn mark_started(&mut self, now: DateTime<Utc>) {
        self.last_started_at = Some(now);
        self.in_flight = true;
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
            "Plan a concrete Kcode code-quality improvement. Do not modify files yet.\n\n",
        );
    } else {
        prompt.push_str(
            "Improve Kcode autonomously with one focused, high-impact code-quality change.\n\n",
        );
    }
    if matches!(request.trigger, SelfImprovementTrigger::Autonomous) {
        prompt.push_str(
            "This run was started automatically in the background. Be conservative: avoid user-visible behavior changes unless clearly beneficial, keep the patch small, and stop if the repo is dirty with unrelated work.\n\n",
        );
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
    fn autonomous_prompt_is_conservative() {
        let prompt = build_self_improvement_prompt(&SelfImprovementRequest::autonomous());
        assert!(prompt.contains("automatically in the background"));
        assert!(prompt.contains("Be conservative"));
        assert!(prompt.contains("commit"));
    }
}
