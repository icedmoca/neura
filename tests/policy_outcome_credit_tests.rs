use neura::operational_policy::{
    OperationalPolicyState, PolicyAction, PolicyDecision, PolicyDomain, PolicyInfluenceAudit,
    policy_state_path,
};
use neura::policy_outcome_credit::{assign_credit, report};
use tempfile::TempDir;

#[test]
fn outcome_credit_updates_ledger() {
    let dir = TempDir::new().unwrap();
    unsafe {
        std::env::set_var(
            "NEURA_OPERATIONAL_POLICY_STATE",
            dir.path().join("policy.json"),
        )
    };
    unsafe { std::env::set_var("NEURA_POLICY_CREDIT_LEDGER", dir.path().join("credit.json")) };
    unsafe { std::env::set_var("NEURA_LATENT_MEMORY_STATE", dir.path().join("memory.json")) };
    let mut state = OperationalPolicyState::default();
    let decision = PolicyDecision {
        domain: PolicyDomain::ProviderChoice,
        action: PolicyAction::Downrank,
        rule_id: None,
        allowed: true,
        confidence: 0.7,
        reason: "test".into(),
        audit_id: "audit-x".into(),
    };
    state.audits.push(PolicyInfluenceAudit {
        id: "audit-x".into(),
        decision,
        target: "provider".into(),
        outcome: "pending".into(),
        timestamp_ms: 0,
    });
    state.save(&policy_state_path()).unwrap();
    let credit = assign_credit("audit-x", "success").unwrap().unwrap();
    assert_eq!(credit.score, 1.0);
    let report = report().unwrap();
    assert_eq!(report.total_credits, 1);
    assert_eq!(report.positive, 1);
}
