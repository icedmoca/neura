use kcode::latent_operational_recurrence::{
    LatentOperationalState, OperationalEvent, anti_sludge, default_invariants, encode_event,
    remap_vector, translate_invariants,
};

#[test]
fn latent_state_records_temporal_provenance() {
    let mut state = LatentOperationalState::default();
    let mut event = OperationalEvent::new("test", "success");
    event.tags = vec!["test".into(), "validation".into(), "provenance".into()];
    event.tool = Some("cargo".into());

    let decision = state.observe(event);

    assert!(decision.accepted);
    assert_eq!(state.events_seen, 1);
    assert_eq!(state.temporal_memory.len(), 1);
    assert_eq!(
        state.temporal_memory[0].provenance.source,
        "kcode-latent-observe"
    );
}

#[test]
fn invariant_layer_translates_multiple_operational_concerns() {
    let mut event = OperationalEvent::new("memory-validation", "success");
    event.tags = vec![
        "memory".into(),
        "provenance".into(),
        "test".into(),
        "validation".into(),
    ];

    let translated = translate_invariants(&event, &default_invariants());

    assert!(
        translated
            .iter()
            .any(|item| item.invariant_id == "validate-before-done" && item.matched)
    );
    assert!(
        translated
            .iter()
            .any(|item| item.invariant_id == "track-provenance" && item.matched)
    );
}

#[test]
fn remap_preserves_vector_shape_and_updates_schema() {
    let event = OperationalEvent::new("provider", "error");
    let encoded = encode_event(&event);
    let remapped = remap_vector(&encoded, 7);

    assert_eq!(remapped.schema_version, 7);
    assert_eq!(remapped.dimensions.len(), encoded.dimensions.len());
}

#[test]
fn anti_sludge_detects_duplicate_memory() {
    let mut state = LatentOperationalState::default();
    for idx in 0..3 {
        let mut event = OperationalEvent::new("repeat", "success");
        event.tags = vec!["test".into(), format!("sample-{idx}")];
        state.observe(event);
    }
    let duplicate = state.temporal_memory[0].clone();
    state.temporal_memory.push(duplicate);

    let report = anti_sludge(&state.temporal_memory);
    assert!(report.duplicate_ratio > 0.0);
}
