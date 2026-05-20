use crate::operational_policy::{
    PolicyDecision, PolicyDomain, load_policy_and_synthesize, policy_state_path,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyRuntimeDecision {
    pub domain: PolicyDomain,
    pub target: String,
    pub decision: PolicyDecision,
    pub active_runtime_hook: bool,
}

pub fn decide(
    domain: PolicyDomain,
    target: &str,
    active_runtime_hook: bool,
) -> Option<PolicyRuntimeDecision> {
    let mut state = load_policy_and_synthesize().ok()?;
    let decision = state.decide(domain.clone(), target);
    state.record_outcome(
        &decision.audit_id,
        if active_runtime_hook {
            "runtime-hook-observed"
        } else {
            "policy-api-observed"
        },
    );
    let _ = state.save(&policy_state_path());
    Some(PolicyRuntimeDecision {
        domain,
        target: target.into(),
        decision,
        active_runtime_hook,
    })
}

pub fn should_require_validation(target: &str) -> bool {
    decide(PolicyDomain::TestValidation, target, true)
        .map(|d| {
            matches!(
                d.decision.action,
                crate::operational_policy::PolicyAction::RequireValidation
            )
        })
        .unwrap_or(false)
}

pub fn memory_retrieval_bias(target: &str) -> f32 {
    decide(PolicyDomain::MemoryRetrieval, target, false)
        .map(|d| d.decision.confidence)
        .unwrap_or(0.0)
}

pub fn context_budget_bias(target: &str) -> f32 {
    decide(PolicyDomain::ContextBudget, target, false)
        .map(|d| d.decision.confidence)
        .unwrap_or(0.0)
}

pub fn drift_control_bias(target: &str) -> f32 {
    decide(PolicyDomain::DriftControl, target, false)
        .map(|d| d.decision.confidence)
        .unwrap_or(0.0)
}

pub fn repair_bias(target: &str) -> f32 {
    decide(PolicyDomain::RepairStrategy, target, false)
        .map(|d| d.decision.confidence)
        .unwrap_or(0.0)
}
