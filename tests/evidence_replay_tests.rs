use neura::evidence_ledger::{EvidenceKind, append_evidence_with_links, ledger_path};
use neura::evidence_replay::{ReplayConfig, render_replay_report, run_replay};
use serde_json::json;

#[test]
fn replay_runs_without_future_leak_and_generates_alternatives() {
    let _ = std::fs::remove_file(ledger_path());
    let parent = append_evidence_with_links(
        EvidenceKind::PolicyDecision,
        "policy replay parent",
        "parent policy decision",
        Some(0.8),
        Some(true),
        &json!({"parent": true}),
        vec![],
        vec![],
        "policy",
    )
    .unwrap();
    append_evidence_with_links(
        EvidenceKind::TinyPatchGate,
        "tiny patch replay child",
        "child gate decision",
        Some(0.7),
        Some(false),
        &json!({"gate": false}),
        vec![parent.receipt.hash.clone()],
        vec![parent.receipt.hash],
        "self-improvement",
    )
    .unwrap();

    let report = run_replay(ReplayConfig {
        limit: 10,
        include_alternatives: true,
        max_index: None,
        subject: None,
    })
    .unwrap();
    assert!(report.replayed >= 2);
    assert!(report.no_future_leaks);
    assert!(report.alternatives_considered >= 3);
    assert!(report.cases.iter().all(|case| case.no_future_leak));
}

#[test]
fn replay_report_renders_decision_cases() {
    let _ = std::fs::remove_file(ledger_path());
    append_evidence_with_links(
        EvidenceKind::OperationalEval,
        "operational replay",
        "operational replay evidence",
        Some(0.91),
        Some(true),
        &json!({"op": true}),
        vec![],
        vec![],
        "operational-eval",
    )
    .unwrap();
    let report = run_replay(ReplayConfig::default()).unwrap();
    let rendered = render_replay_report(&report);
    assert!(rendered.contains("Evidence Replay Report"));
    assert!(rendered.contains("No future leaks"));
    assert!(rendered.contains("Alternatives"));
}
