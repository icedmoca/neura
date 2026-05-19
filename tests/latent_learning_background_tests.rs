use kcode::latent_learning_background::{
    command_event, doctrine_summary, ingest_runtime_event, outcome_summary, run_background_cycle,
    set_paused, status,
};
use tempfile::TempDir;

fn isolate(dir: &TempDir) {
    unsafe { std::env::set_var("KCODE_LATENT_LEARNING_DIR", dir.path()) };
    unsafe { std::env::set_var("KCODE_LATENT_STATE", dir.path().join("latent.json")) };
    unsafe {
        std::env::set_var(
            "KCODE_LATENT_LEARNING_STATE",
            dir.path().join("learning.json"),
        )
    };
}

#[test]
fn command_derived_sample_updates_report_counts() {
    let dir = TempDir::new().unwrap();
    isolate(&dir);

    ingest_runtime_event(
        command_event(
            "build",
            "success",
            vec!["test".into(), "validation".into()],
            Some("cargo".into()),
        ),
        "command-test",
    )
    .unwrap();

    let before = status().unwrap();
    assert_eq!(before.total_samples, 1);
    assert_eq!(before.pending_samples, 1);

    let cycle = run_background_cycle(16).unwrap();
    assert_eq!(cycle.consumed, 1);

    let after = status().unwrap();
    assert_eq!(after.consumed_samples, 1);
    assert_eq!(after.pending_samples, 0);

    let outcomes = outcome_summary().unwrap();
    assert_eq!(outcomes.success, 1);

    let doctrines = doctrine_summary().unwrap();
    assert!(doctrines.convergence_score >= 0.0);
}

#[test]
fn pause_and_resume_are_persistent_controls() {
    let dir = TempDir::new().unwrap();
    isolate(&dir);

    ingest_runtime_event(
        command_event("test", "success", vec!["test".into()], None),
        "pause-test",
    )
    .unwrap();
    assert!(set_paused(true).unwrap().paused);
    assert_eq!(run_background_cycle(16).unwrap().consumed, 0);
    assert_eq!(status().unwrap().pending_samples, 1);

    assert!(!set_paused(false).unwrap().paused);
    assert_eq!(run_background_cycle(16).unwrap().consumed, 1);
    assert_eq!(status().unwrap().pending_samples, 0);
}
