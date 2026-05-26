use kcode::autonomous_improvement::{
    render_evidence_ranked_tasks, synthesize_evidence_ranked_tasks, tiny_patch_gate,
};

#[test]
fn evidence_ranked_tasks_are_sorted_and_reported() {
    let report = synthesize_evidence_ranked_tasks().expect("task synthesis should run");
    assert!(report.tasks.len() >= 3);
    for pair in report.tasks.windows(2) {
        assert!(pair[0].rank_score >= pair[1].rank_score);
    }
    let rendered = render_evidence_ranked_tasks(&report);
    assert!(rendered.contains("Evidence-Ranked Self-Improvement Tasks"));
    assert!(rendered.contains("Tiny patch gate"));
}

#[test]
fn tiny_patch_gate_blocks_by_default_and_blocks_risky_patch() {
    let report = synthesize_evidence_ranked_tasks().expect("task synthesis should run");
    let top = report.tasks.first().expect("expected ranked task");
    let dry_gate = tiny_patch_gate(top, true, false);
    assert!(!dry_gate.allowed);
    assert!(dry_gate.reasons.iter().any(|r| r.contains("dry-run")));

    let risky = report
        .tasks
        .iter()
        .find(|task| task.id == "task-risky-runtime-rewrite")
        .expect("risky fixture should exist");
    let risky_gate = tiny_patch_gate(risky, false, true);
    assert!(!risky_gate.allowed);
    assert!(
        risky_gate
            .reasons
            .iter()
            .any(|r| r.contains("too many files"))
    );
}
