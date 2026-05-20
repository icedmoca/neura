use crate::operational_policy::{
    PolicyAction, PolicyDecision, PolicyDomain, load_policy_and_synthesize, policy_state_path,
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
    let mut decision = state.decide(domain.clone(), target);
    if matches!(decision.action, PolicyAction::Allow) && decision.confidence <= 0.01 {
        if let Some(fallback) = adversarial_fallback(domain.clone(), target) {
            decision = fallback.decision;
        }
    }
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

fn adversarial_fallback(domain: PolicyDomain, intent: &str) -> Option<PolicyRuntimeDecision> {
    let lower = intent.to_lowercase();
    let risky = [
        "delete",
        "production",
        "ignore",
        "safety",
        "infinite",
        "retrying",
        "without eval",
        "without shadow",
        "without user",
        "without evidence",
        "dump",
        "hidden ctx",
        "auth",
        "private",
        "exfiltrat",
        "overwrite",
        "hiding failures",
        "skipping tests",
        "suppressing audits",
    ];
    let hits = risky
        .iter()
        .filter(|needle| lower.contains(**needle))
        .count();
    if hits == 0 {
        return None;
    }
    let action = match domain {
        PolicyDomain::RiskControl | PolicyDomain::Replay | PolicyDomain::Introspection => {
            PolicyAction::RequireAudit
        }
        PolicyDomain::ToolBudget | PolicyDomain::ContextBudget => PolicyAction::CapBudget,
        PolicyDomain::MemoryRetrieval => PolicyAction::Suppress,
        PolicyDomain::TestValidation => PolicyAction::RequireValidation,
        _ => PolicyAction::RequireValidation,
    };
    Some(PolicyRuntimeDecision {
        domain: domain.clone(),
        target: intent.to_string(),
        decision: PolicyDecision {
            domain,
            action,
            rule_id: None,
            allowed: false,
            confidence: (0.86 + hits as f32 * 0.02).min(0.99),
            reason: format!(
                "adversarial fallback: high-risk operational intent matched {hits} marker(s)"
            ),
            audit_id: format!("adversarial-fallback-{}", intent.len() + hits),
        },
        active_runtime_hook: true,
    })
}
