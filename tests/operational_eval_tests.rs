use kcode::operational_eval::{
    enforce_operational_eval_gate, render_operational_eval_report, run_operational_eval_suite,
};

#[test]
fn operational_eval_runs_and_renders() {
    let report = run_operational_eval_suite().expect("eval suite should run");
    assert!(!report.scenarios.is_empty());
    assert!(report.mean_score >= 0.0);
    let rendered = render_operational_eval_report(&report);
    assert!(rendered.contains("Operational Self-Eval"));
    assert!(rendered.contains("Closed Loop Automation"));
}

#[test]
fn operational_eval_gate_is_decidable() {
    let _ = run_operational_eval_suite().expect("eval suite should run before gate");
    let gate =
        enforce_operational_eval_gate().expect("gate should pass on deterministic self eval");
    assert!(gate.passed);
}
