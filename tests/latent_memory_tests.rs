use kcode::latent_learning_background::{
    command_event, ingest_runtime_event, run_background_cycle,
};
use kcode::latent_memory::{LatentMemoryBank, latent_memory_path};
use tempfile::TempDir;

fn isolate(dir: &TempDir) {
    unsafe { std::env::set_var("KCODE_LATENT_LEARNING_DIR", dir.path().join("learning")) };
    unsafe { std::env::set_var("KCODE_LATENT_STATE", dir.path().join("latent.json")) };
    unsafe {
        std::env::set_var(
            "KCODE_LATENT_LEARNING_STATE",
            dir.path().join("learning.json"),
        )
    };
    unsafe {
        std::env::set_var(
            "KCODE_LATENT_MEMORY_STATE",
            dir.path().join("latent-memory.json"),
        )
    };
}

#[test]
fn background_cycle_creates_ctx_style_latent_memory() {
    let dir = TempDir::new().unwrap();
    isolate(&dir);
    ingest_runtime_event(
        command_event(
            "build",
            "success",
            vec!["test".into(), "validation".into()],
            Some("cargo".into()),
        ),
        "latent-memory-test",
    )
    .unwrap();
    let result = run_background_cycle(16).unwrap();
    assert_eq!(result.consumed, 1);
    let bank = LatentMemoryBank::load_or_default(&latent_memory_path()).unwrap();
    assert!(!bank.entries.is_empty());
    assert!(bank.ctx_blocks(1)[0].contains("<ctx k=\"latent-memory\""));
}

#[test]
fn duplicate_noise_is_suppressed_by_latent_memory() {
    let dir = TempDir::new().unwrap();
    isolate(&dir);
    for _ in 0..2 {
        ingest_runtime_event(
            command_event(
                "live::TokenUsage",
                "observed",
                vec!["token".into(), "live-fabric".into()],
                Some("OpenAI".into()),
            ),
            "latent-memory-test",
        )
        .unwrap();
    }
    let first = run_background_cycle(1).unwrap();
    let second = run_background_cycle(1).unwrap();
    assert_eq!(first.consumed, 1);
    assert!(second.consumed + second.skipped >= 1);
}

#[test]
fn closed_loop_attribution_records_usefulness() {
    let dir = TempDir::new().unwrap();
    isolate(&dir);
    ingest_runtime_event(
        command_event(
            "build",
            "success",
            vec!["test".into(), "validation".into()],
            Some("cargo".into()),
        ),
        "latent-memory-attribution-test",
    )
    .unwrap();
    let result = run_background_cycle(16).unwrap();
    assert_eq!(result.consumed, 1);
    let bank = LatentMemoryBank::load_or_default(&latent_memory_path()).unwrap();
    let report = bank.usefulness_report();
    assert!(report.total_attributions >= 1);
    assert!(report.mean_outcome_score >= 0.0);
}
