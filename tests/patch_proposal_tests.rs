use kcode::evidence_ledger::ledger_path;
use kcode::patch_proposal::{build_patch_report, dry_run_patch, promotion_gate, validate_patch};

#[test]
fn patch_proposal_dry_run_has_positive_replay_delta() {
    let _ = std::fs::remove_file(ledger_path());
    let proposal = dry_run_patch(Some("top")).expect("proposal should build");
    assert!(proposal.patch_text.contains("Dry-run patch proposal"));
    assert!(proposal.replay_delta >= 0.0);
    assert!(!proposal.mutation_allowed);
    assert!(proposal.ledger_receipt_hash.is_some());
}

#[test]
fn patch_promotion_gate_remains_blocked_by_default() {
    let _ = std::fs::remove_file(ledger_path());
    let report = build_patch_report(Some("top"), false).expect("report should build");
    let gate = &report.promotion_gate;
    assert!(!gate.allowed);
    assert!(
        gate.reasons
            .iter()
            .any(|reason| reason.contains("mutation remains disabled"))
    );
}

#[test]
fn patch_validation_model_runs_checks() {
    let _ = std::fs::remove_file(ledger_path());
    let proposal = dry_run_patch(Some("top")).expect("proposal should build");
    let validation = validate_patch(&proposal);
    assert_eq!(validation.proposal_id, proposal.id);
    assert!(!validation.checks.is_empty());
    let gate = promotion_gate(&proposal, &validation);
    assert!(!gate.allowed);
}
