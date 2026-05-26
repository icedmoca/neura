use kcode::evidence_ledger::{EvidenceKind, EvidenceLedger, render_ledger_report};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn ledger_hash_chain_verifies_and_detects_tampering() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut ledger = EvidenceLedger::default();
    ledger
        .append(
            EvidenceKind::System,
            "genesis-test",
            "first block",
            Some(1.0),
            Some(true),
            &json!({"a": 1}),
        )
        .unwrap();
    ledger
        .append(
            EvidenceKind::Validation,
            "second-test",
            "second block",
            Some(0.9),
            Some(true),
            &json!({"b": 2}),
        )
        .unwrap();
    let verification = ledger.verify();
    assert!(verification.valid, "{:?}", verification.errors);
    ledger.save(&path).unwrap();
    let loaded = EvidenceLedger::load_or_default(&path).unwrap();
    assert!(loaded.verify().valid);

    let mut tampered = loaded.clone();
    tampered.blocks[0].summary = "tampered".into();
    let bad = tampered.verify();
    assert!(!bad.valid);
    assert!(bad.errors.iter().any(|e| e.contains("hash mismatch")));
}

#[test]
fn ledger_report_renders_blocks() {
    let mut ledger = EvidenceLedger::default();
    ledger
        .append(
            EvidenceKind::OperationalEval,
            "op",
            "operational evidence",
            Some(0.95),
            Some(true),
            &json!({"ok": true}),
        )
        .unwrap();
    let report = render_ledger_report(&ledger);
    assert!(report.contains("Cognition Evidence Chain"));
    assert!(report.contains("Valid: `true`"));
    assert!(report.contains("OperationalEval"));
}
