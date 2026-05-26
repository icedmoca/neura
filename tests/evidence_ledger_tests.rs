use kcode::evidence_ledger::{
    EvidenceKind, EvidenceLedger, LedgerQuery, append_evidence_with_links, explain_evidence,
    query_ledger, render_ledger_report,
};
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

#[test]
fn ledger_supports_receipts_links_query_and_explain() {
    let _ = std::fs::remove_file(kcode::evidence_ledger::ledger_path());
    let first = append_evidence_with_links(
        EvidenceKind::PolicyDecision,
        "policy-parent",
        "parent decision",
        Some(0.8),
        Some(true),
        &json!({"policy": true}),
        vec![],
        vec![],
        "policy",
    )
    .unwrap();
    let second = append_evidence_with_links(
        EvidenceKind::TinyPatchGate,
        "tiny-child",
        "child gate",
        Some(0.9),
        Some(false),
        &json!({"gate": false}),
        vec![first.receipt.hash.clone()],
        vec![first.receipt.hash.clone()],
        "self-improvement",
    )
    .unwrap();
    assert!(second.verification.valid);
    assert_eq!(second.receipt.subsystem, "self-improvement");

    let queried = query_ledger(LedgerQuery {
        kind: Some(EvidenceKind::TinyPatchGate),
        subject_contains: Some("tiny".into()),
        subsystem: Some("self-improvement".into()),
        limit: 10,
    })
    .unwrap();
    assert_eq!(queried.len(), 1);

    let explained = explain_evidence(&second.receipt.hash[..12])
        .unwrap()
        .unwrap();
    assert!(explained.verifies);
    assert_eq!(explained.parents.len(), 1);
    assert_eq!(explained.causes.len(), 1);
}
