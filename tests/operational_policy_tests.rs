use neura::latent_memory::{LatentMemoryBank, LatentMemoryEntry, LatentMemoryKind};
use neura::latent_operational_recurrence::LatentVector;
use neura::operational_policy::{
    OperationalPolicyState, PolicyAction, PolicyDomain, synthesize_rules_from_latent_memory,
};

#[test]
fn useful_latent_memory_becomes_gated_policy() {
    let mut bank = LatentMemoryBank::default();
    bank.entries.push(LatentMemoryEntry {
        id: "validation-memory".into(),
        kind: LatentMemoryKind::ValidationDoctrine,
        summary: "validation helps".into(),
        ctx_block: "<ctx k=\"latent-memory\">validation</ctx>".into(),
        vector: LatentVector::default(),
        tags: vec!["test".into(), "validation".into()],
        confidence: 0.9,
        usefulness_score: 0.9,
        influence_count: 4,
        positive_outcomes: 4,
        negative_outcomes: 0,
        drift_reduction_total: 0.3,
        support: 4,
        last_seen_ms: 0,
    });
    let mut policy = OperationalPolicyState::default();
    synthesize_rules_from_latent_memory(&mut policy, &bank);
    let decision = policy.decide(PolicyDomain::TestValidation, "final-answer");
    assert_eq!(decision.action, PolicyAction::RequireValidation);
    assert!(decision.allowed);
    assert_eq!(policy.audits.len(), 1);
}
