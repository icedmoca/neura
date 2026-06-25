use neura::latent_learning::{
    LatentLearningState, convergence_metrics, counterfactual_probe, remap_plan,
};
use neura::latent_operational_recurrence::{LatentOperationalState, OperationalEvent};

#[test]
fn learning_accepts_validated_success_and_forms_attractor() {
    let recurrence = LatentOperationalState::default();
    let mut learning = LatentLearningState::default();
    let mut event = OperationalEvent::new("build", "success");
    event.tags = vec!["test".into(), "validation".into()];

    let step = learning.learn(&recurrence, event);

    assert!(!step.immune.triggered);
    assert_eq!(learning.samples_seen, 1);
    assert!(!learning.learned_vectors.is_empty());
    assert!(!learning.attractors.is_empty());
}

#[test]
fn immune_layer_blocks_negative_destructive_learning() {
    let recurrence = LatentOperationalState::default();
    let mut learning = LatentLearningState::default();
    let mut event = OperationalEvent::new("rm", "success");
    event.tags = vec!["destructive".into()];

    let step = learning.learn(&recurrence, event);

    assert!(step.immune.triggered);
    assert_eq!(learning.samples_seen, 0);
    assert_eq!(learning.immune_history.len(), 1);
}

#[test]
fn counterfactual_reports_delta() {
    let recurrence = LatentOperationalState::default();
    let mut event = OperationalEvent::new("memory", "success");
    event.tags = vec!["memory".into()];

    let probe = counterfactual_probe(
        &recurrence,
        &event,
        vec!["memory".into(), "provenance".into()],
    );

    assert_eq!(probe.event_kind, "memory");
    assert!(probe.projected_score >= probe.baseline_score);
}

#[test]
fn convergence_and_remap_are_inspectable() {
    let recurrence = LatentOperationalState::default();
    let learning = LatentLearningState::default();
    let metrics = convergence_metrics(&learning, &recurrence);
    let plan = remap_plan(&recurrence.vector, 2);

    assert_eq!(metrics.attractor_count, 0);
    assert_eq!(plan.to_schema, 2);
    assert_eq!(plan.preserve_dimensions, recurrence.vector.dimensions.len());
}
