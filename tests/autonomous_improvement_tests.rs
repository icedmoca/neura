use kcode::autonomous_improvement::{
    ImprovementConfig, render_self_improvement_report, run_self_improvement_cycle,
};

#[test]
fn self_improvement_cycle_runs_bounded_dry_run() {
    let report = run_self_improvement_cycle(ImprovementConfig {
        max_iterations: 1,
        dry_run: true,
        allow_mutation: false,
        ..ImprovementConfig::default()
    })
    .expect("self improvement cycle should run");
    assert_eq!(report.iterations.len(), 1);
    assert!(report.iterations[0].applied_actions.is_empty());
    assert!(!report.iterations[0].candidate_actions.is_empty());
    assert!(!report.iterations[0].validation.is_empty());
}

#[test]
fn self_improvement_report_renders_safety_model() {
    let report = run_self_improvement_cycle(ImprovementConfig {
        max_iterations: 1,
        dry_run: true,
        allow_mutation: false,
        ..ImprovementConfig::default()
    })
    .expect("self improvement cycle should run");
    let rendered = render_self_improvement_report(&report);
    assert!(rendered.contains("Autonomous Self-Improvement"));
    assert!(rendered.contains("Safety Model"));
    assert!(rendered.contains("Mutation allowed: `false`"));
}
