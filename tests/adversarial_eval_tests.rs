use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

use kcode::adversarial_eval::{
    enforce_adversarial_eval_gate, render_adversarial_eval_report, run_adversarial_eval_suite,
};
use kcode::operational_eval::enforce_operational_eval_gate;

#[test]
fn adversarial_eval_runs_and_renders() {
    let _guard = TEST_LOCK.lock().unwrap();
    let report = run_adversarial_eval_suite().expect("adversarial eval should run");
    assert!(report.cases.len() >= 7);
    assert!(
        report.mean_score >= 0.85,
        "mean_score={}",
        report.mean_score
    );
    assert!(report.passed, "gate={}", report.gate.reason);
    let rendered = render_adversarial_eval_report(&report);
    assert!(rendered.contains("Adversarial Operational Eval"));
    assert!(rendered.contains("Promotion Hardening"));
}

#[test]
fn adversarial_gate_is_part_of_operational_gate() {
    let _guard = TEST_LOCK.lock().unwrap();
    let adversarial = enforce_adversarial_eval_gate().expect("adversarial gate should pass");
    assert!(adversarial.passed);
    let combined = enforce_operational_eval_gate().expect("combined operational gate should pass");
    assert!(combined.reason.contains("adversarial eval passed"));
}
