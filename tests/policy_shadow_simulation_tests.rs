use neura::operational_policy::{
    OperationalPolicyRule, OperationalPolicyState, PolicyAction, PolicyDomain, PolicyWiringStatus,
    policy_state_path,
};
use neura::policy_shadow_simulation::{
    PolicyShadowLedger, PolicyShadowResult, demote_bad, promote_safe, shadow_ledger_path,
};
use tempfile::TempDir;

#[test]
fn promotes_and_demotes_from_shadow_ledger() {
    let dir = TempDir::new().unwrap();
    unsafe {
        std::env::set_var(
            "NEURA_OPERATIONAL_POLICY_STATE",
            dir.path().join("policy.json"),
        )
    };
    unsafe { std::env::set_var("NEURA_POLICY_SHADOW_LEDGER", dir.path().join("shadow.json")) };
    let mut state = OperationalPolicyState::default();
    state.rules.push(OperationalPolicyRule {
        id: "r1".into(),
        domain: PolicyDomain::TestValidation,
        action: PolicyAction::RequireValidation,
        condition: "test".into(),
        confidence: 0.6,
        support: 1,
        source_memory_id: None,
        enabled: true,
        wiring_status: PolicyWiringStatus::ActiveRuntimeHook,
    });
    state.save(&policy_state_path()).unwrap();
    let ledger = PolicyShadowLedger {
        schema_version: 1,
        results: vec![PolicyShadowResult {
            schema_version: 1,
            source: "test".into(),
            domain: PolicyDomain::TestValidation,
            target: "final".into(),
            rule_id: Some("r1".into()),
            action: PolicyAction::RequireValidation,
            baseline_outcome: "success".into(),
            baseline_score: 0.0,
            simulated_score: 0.2,
            delta: 0.2,
            safe_to_promote: true,
            should_demote: false,
            reason: "win".into(),
        }],
    };
    ledger.save(&shadow_ledger_path()).unwrap();
    assert_eq!(promote_safe().unwrap(), 1);
    assert_eq!(demote_bad().unwrap(), 0);
}
