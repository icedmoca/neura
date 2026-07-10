use super::repo::{RepositorySource, module_key};
use super::*;
use crate::memory_graph::MemoryGraph;
use std::fs;
use std::path::Path;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// Options that keep tests hermetic: no sidecar, no embedding model needed.
fn test_opts() -> IngestOptions {
    IngestOptions {
        full: false,
        max_items_per_pass: 100,
        max_embeddings_per_pass: 0,
        abstraction_budget: 0,
    }
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(fut)
}

/// The tool-outcome and prediction queues are process-global; serialize the
/// tests that touch them so parallel test threads don't drain each other.
static GLOBAL_QUEUES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn queues_guard() -> std::sync::MutexGuard<'static, ()> {
    GLOBAL_QUEUES_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn unit_ids_are_deterministic_and_distinct() {
    let a1 = unit_memory_id("repo:/x", "module:src/a.rs");
    let a2 = unit_memory_id("repo:/x", "module:src/a.rs");
    let b = unit_memory_id("repo:/x", "module:src/b.rs");
    let other_source = unit_memory_id("repo:/y", "module:src/a.rs");
    assert_eq!(a1, a2);
    assert_ne!(a1, b);
    assert_ne!(a1, other_source);
    assert!(a1.starts_with("mem-src-"));
}

#[test]
fn repository_ingest_is_incremental_idempotent_and_retires() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    write(
        root,
        "src/lib.rs",
        "//! Library root: coordinates the demo subsystems.\n\
         use crate::agent::turn;\npub struct AppState;\npub fn run() {}\n",
    );
    write(
        root,
        "src/agent/turn.rs",
        "//! Turn execution for the demo agent.\n\
         pub struct Turn;\npub enum Phase { A, B }\npub fn execute() {}\n",
    );
    write(
        root,
        "src/agent/turn_tests.rs",
        "#[test]\nfn works() {}\npub fn helper() {}\n",
    );
    write(
        root,
        "docs/GUIDE.md",
        "# Demo Guide\n\nThe agent turn logic lives in src/agent/turn.rs and runs phases.\n",
    );

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();

    // ---- First ingest: concepts + typed edges appear ----
    let report =
        block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();
    assert!(report.concepts_created >= 6, "repo + packages + files: {report:?}");
    assert_eq!(report.concepts_retired, 0);

    let module_id = unit_memory_id(&source_id, &module_key("src/agent/turn.rs"));
    let module = graph.get_memory(&module_id).expect("module concept exists");
    assert!(module.content.contains("Turn execution for the demo agent"));
    assert!(module.content.contains("Key symbols"));
    assert!(!module.evidence.is_empty(), "structure lands as evidence");

    // lib.rs depends on agent::turn → DependsOn edge between module concepts.
    let lib_id = unit_memory_id(&source_id, &module_key("src/lib.rs"));
    let deps: Vec<_> = graph
        .get_edges(&lib_id)
        .iter()
        .filter(|e| e.kind == crate::memory_graph::EdgeKind::DependsOn)
        .collect();
    assert!(
        deps.iter().any(|e| e.target == module_id),
        "lib.rs should DependOn agent::turn"
    );

    // Test file supports the module it exercises.
    let test_id = unit_memory_id(&source_id, &super::repo::test_key("src/agent/turn_tests.rs"));
    assert!(
        graph
            .get_edges(&test_id)
            .iter()
            .any(|e| e.kind == crate::memory_graph::EdgeKind::Supports
                && e.target == module_id),
        "tests should Support the module under test"
    );

    // Doc mentioning src/agent/turn.rs supports it.
    let doc_id = unit_memory_id(&source_id, &super::repo::doc_key("docs/GUIDE.md"));
    assert!(
        graph
            .get_edges(&doc_id)
            .iter()
            .any(|e| e.kind == crate::memory_graph::EdgeKind::Supports
                && e.target == module_id),
        "docs mentioning a module should Support it"
    );

    // ---- Second ingest with no changes: fully idempotent (extraction is
    // skipped entirely when the fingerprint diff is empty) ----
    let report2 =
        block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();
    assert_eq!(report2.items_changed, 0, "{report2:?}");
    assert_eq!(report2.concepts_created, 0);
    assert_eq!(report2.concepts_updated, 0);
    assert_eq!(report2.concepts_retired, 0);

    // ---- Structural change: only the touched concept updates ----
    write(
        root,
        "src/agent/turn.rs",
        "//! Turn execution for the demo agent.\n\
         pub struct Turn;\npub enum Phase { A, B }\npub fn execute() {}\npub fn cancel() {}\n",
    );
    let mut source = RepositorySource::new(root);
    let report3 =
        block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();
    assert_eq!(report3.items_changed, 1, "{report3:?}");
    assert_eq!(report3.concepts_created, 0);
    assert!(
        graph
            .get_memory(&module_id)
            .unwrap()
            .content
            .contains("cancel"),
        "updated concept should reflect the new symbol"
    );

    // ---- Removal: concept retired (never deleted), package survives ----
    fs::remove_file(root.join("src/agent/turn.rs")).unwrap();
    let mut source = RepositorySource::new(root);
    let report4 =
        block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();
    assert!(report4.concepts_retired >= 1, "{report4:?}");
    let retired = graph.get_memory(&module_id).expect("retired, not deleted");
    assert!(!retired.active);
    assert!(retired.tags.iter().any(|t| t == TAG_RETIRED));
    let pkg_id = unit_memory_id(&source_id, &super::repo::package_key("src/agent"));
    assert!(
        graph.get_memory(&pkg_id).map(|m| m.active).unwrap_or(false),
        "package still has a backing member (turn_tests.rs) and must survive"
    );

    // ---- Reappearance: same deterministic id reactivates ----
    write(
        root,
        "src/agent/turn.rs",
        "//! Turn execution for the demo agent.\npub fn execute() {}\npub struct Turn;\npub enum P {A}\n",
    );
    let mut source = RepositorySource::new(root);
    let report5 =
        block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();
    assert!(report5.concepts_reactivated >= 1, "{report5:?}");
    assert!(graph.get_memory(&module_id).unwrap().active);

    // State is persisted inside graph metadata — no parallel store.
    let state = graph
        .metadata
        .knowledge_sources
        .get(&source_id)
        .expect("source state lives in GraphMetadata");
    assert!(state.fingerprints.contains_key("src/agent/turn.rs"));
    assert!(state.last_report.is_some());
}

#[test]
fn tool_outcomes_fold_into_concept_evidence() {
    let _guard = queues_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/core.rs",
        "//! Core module.\npub struct Core;\npub fn go() {}\npub fn stop() {}\n",
    );

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    let module_id = unit_memory_id(&source_id, &module_key("src/core.rs"));
    let strength_before = graph.get_memory(&module_id).unwrap().strength;

    // Absolute path under the source root, as an edit tool would supply.
    let abs = root
        .canonicalize()
        .unwrap()
        .join("src/core.rs")
        .display()
        .to_string();
    evidence::note_tool_outcome(
        "edit",
        &serde_json::json!({ "file_path": abs, "old_string": "x" }),
        true,
    );
    let applied = evidence::apply_queued_outcomes(&mut graph);
    assert!(applied >= 1, "outcome should map back to the module concept");

    let module = graph.get_memory(&module_id).unwrap();
    assert!(module.strength > strength_before, "success reinforces the concept");
    assert!(
        module
            .evidence
            .iter()
            .any(|ev| ev.note.as_deref().is_some_and(|n| n.contains("tool edit succeeded"))),
        "the tool outcome is recorded as evidence"
    );
}

#[test]
fn reasoning_traces_are_deterministic_and_graph_driven() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/lib.rs",
        "//! Root: wires routing and storage together.\nuse crate::router::route;\npub fn boot() {}\npub struct App;\npub enum Mode {A}\n",
    );
    write(
        root,
        "src/router/route.rs",
        "//! Routing: dispatches requests to handlers.\npub struct Router;\npub enum Verb {Get}\npub fn dispatch() {}\n",
    );
    write(
        root,
        "src/router/route_tests.rs",
        "#[test]\nfn routes() {}\npub fn helper() {}\n",
    );

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    // Reason: seeds found by keyword, expansion through typed edges,
    // relations and evidence explain the conclusion.
    let trace = reasoning::reason(&mut graph, "routing dispatch requests");
    assert!(!trace.seeds.is_empty(), "should match the router concept");
    let route_id = unit_memory_id(&source_id, &module_key("src/router/route.rs"));
    assert!(
        trace.seeds.iter().any(|s| s.id == route_id),
        "router module should be a seed: {:?}",
        trace.seeds
    );
    assert!(!trace.expanded.is_empty(), "cascade should expand the neighborhood");
    assert!(!trace.relations.is_empty(), "typed relations justify the trace");
    assert!(!trace.evidence.is_empty(), "evidence rides along");
    assert!(trace.confidence > 0.0);

    // Same query, same graph → identical trace (determinism).
    let trace2 = reasoning::reason(&mut graph, "routing dispatch requests");
    assert_eq!(
        serde_json::to_string(&trace.seeds).unwrap(),
        serde_json::to_string(&trace2.seeds).unwrap()
    );

    // Impact: lib.rs DependsOn route.rs, so changing route.rs affects lib;
    // the test file shows up as a likely-affected test.
    let model = reasoning::impact_for(&graph, &[route_id.clone()], 3);
    let lib_id = unit_memory_id(&source_id, &module_key("src/lib.rs"));
    assert!(
        model.affected.iter().any(|a| a.id == lib_id && a.via == "depends_on"),
        "lib should be affected via depends_on: {:?}",
        model.affected
    );
    assert!(
        model.likely_tests.iter().any(|t| t.contains("route_tests")),
        "route tests should be likely-affected: {:?}",
        model.likely_tests
    );
    assert!(model.uncertainty < 1.0);
}

#[test]
fn turn_brief_predicts_and_reflection_reinforces() {
    let _guard = queues_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/engine.rs",
        "//! Engine: schedules and executes jobs.\npub struct Engine;\npub enum Job {A}\npub fn schedule_job() {}\n",
    );
    write(
        root,
        "src/lib.rs",
        "//! Root.\nuse crate::engine;\npub fn main_loop() {}\npub struct S;\npub enum E {A}\n",
    );

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    let brief = reasoning::turn_brief_for_graph(&mut graph, "fix the engine job scheduling bug")
        .expect("engine mention should produce an architectural brief");
    assert!(brief.contains("architectural context"));
    assert!(brief.contains("Engine") || brief.contains("engine"));
    assert!(reasoning::pending_prediction_count() >= 1);

    // Execution touches the predicted concept → reflection confirms and
    // reinforces it through the existing evidence machinery.
    let engine_id = unit_memory_id(&source_id, &module_key("src/engine.rs"));
    let strength_before = graph.get_memory(&engine_id).unwrap().strength;
    let mut touched = std::collections::BTreeSet::new();
    touched.insert(engine_id.clone());
    let stats = reasoning::reflect_on_outcomes(&mut graph, &touched)
        .expect("pending prediction should produce a reflection");
    assert!(stats.confirmed >= 1, "{stats:?}");
    assert!(graph.get_memory(&engine_id).unwrap().strength > strength_before);
    assert_eq!(reasoning::pending_prediction_count(), 0, "queue drained");

    // Pure comparison arithmetic.
    let pred = reasoning::TurnPrediction {
        session_id: "s".into(),
        predicted_concepts: vec!["a".into(), "b".into()],
        query_preview: "q".into(),
        at: chrono::Utc::now(),
    };
    let mut touched = std::collections::BTreeSet::new();
    touched.insert("a".to_string());
    touched.insert("c".to_string());
    let stats = reasoning::compare_predictions(&[pred], &touched);
    assert_eq!(stats.confirmed, 1);
    assert_eq!(stats.missed, 1);
    assert_eq!(stats.unexpected, 1);
    assert!((stats.precision - 0.5).abs() < f32::EPSILON);
}

#[test]
fn architecture_insights_detect_hubs_duplicates_and_dead_concepts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // hub.rs is imported by several modules → coupling hotspot + centrality.
    write(root, "src/hub.rs", "//! Hub.\npub struct Hub;\npub fn hub() {}\npub enum H {A}\n");
    for i in 0..4 {
        write(
            root,
            &format!("src/user_{i}.rs"),
            &format!("//! User {i}.\nuse crate::hub;\npub struct U{i};\npub fn use_{i}() {{}}\npub enum E{i} {{A}}\n"),
        );
    }

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    // Duplicate abstraction: two active concepts with identical content.
    let mut dup_a = crate::memory::MemoryEntry::new(
        crate::memory::MemoryCategory::Fact,
        "The build pipeline caches artifacts between stages for speed and reproducibility.",
    );
    dup_a.id = "mem-dup-a".into();
    let mut dup_b = dup_a.clone();
    dup_b.id = "mem-dup-b".into();
    graph.add_memory(dup_a);
    graph.add_memory(dup_b);

    // Dead concept: active, no relations, never used.
    let mut dead = crate::memory::MemoryEntry::new(
        crate::memory::MemoryCategory::Fact,
        "An orphaned note about a subsystem that was deleted long ago, nobody links here.",
    );
    dead.id = "mem-dead-1".into();
    dead.strength = 1;
    graph.add_memory(dead);

    let insights = insights::architecture_insights(&graph);
    let kinds: Vec<_> = insights.iter().map(|i| i.kind).collect();
    assert!(
        kinds.contains(&insights::InsightKind::CouplingHotspot),
        "hub with 4 dependents should be a coupling hotspot: {insights:?}"
    );
    assert!(kinds.contains(&insights::InsightKind::HighCentrality));
    assert!(
        insights.iter().any(|i| i.kind == insights::InsightKind::DuplicateAbstraction
            && i.detail.contains("mem-dup-a")),
        "identical concepts should be flagged: {insights:?}"
    );
    assert!(
        insights
            .iter()
            .any(|i| i.kind == insights::InsightKind::DeadConcept && i.subject_id == "mem-dead-1"),
        "orphaned concept should be flagged: {insights:?}"
    );
    // Observation only: the graph is untouched by analysis.
    let hub_id = unit_memory_id(&source_id, &module_key("src/hub.rs"));
    assert!(graph.get_memory(&hub_id).unwrap().active);
}

#[test]
fn plans_are_dependency_ordered_and_evolve_in_place() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Dependency chain: lib → engine → store (lib depends on engine, …).
    write(root, "src/store.rs", "//! Store: persists records.\npub struct Store;\npub fn save_record() {}\npub enum K {A}\n");
    write(root, "src/engine.rs", "//! Engine: runs jobs using the store.\nuse crate::store;\npub struct Engine;\npub fn run_engine_job() {}\npub enum J {A}\n");
    write(root, "src/lib.rs", "//! Root.\nuse crate::engine;\npub fn boot() {}\npub struct App;\npub enum M {A}\n");

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    let plan = engineering::decompose(&mut graph, "engine store record job").unwrap();
    assert!(plan.stages.len() >= 2, "{plan:?}");

    // Dependencies come first: store before engine, engine before lib.
    let pos = |needle: &str| {
        plan.stages
            .iter()
            .position(|s| s.concept_id == unit_memory_id(&source_id, &module_key(needle)))
    };
    let (store_pos, engine_pos) = (pos("src/store.rs"), pos("src/engine.rs"));
    if let (Some(sp), Some(ep)) = (store_pos, engine_pos) {
        assert!(sp < ep, "store must be staged before engine: {plan:?}");
    } else {
        panic!("store and engine should both be plan stages: {plan:?}");
    }
    if let (Some(ep), Some(lp)) = (engine_pos, pos("src/lib.rs")) {
        assert!(ep < lp, "engine must be staged before its dependent lib: {plan:?}");
    }

    // The plan persists as an evolving concept: same topic → same id.
    let plan_memory = graph.get_memory(&plan.memory_id).expect("plan concept persisted");
    assert!(plan_memory.tags.iter().any(|t| t == engineering::TAG_PLAN));
    let before = graph.memory_count();
    let plan2 = engineering::decompose(&mut graph, "engine store record job").unwrap();
    assert_eq!(plan2.memory_id, plan.memory_id);
    assert_eq!(graph.memory_count(), before, "re-planning must not duplicate");
}

#[test]
fn decisions_are_first_class_linked_concepts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/cache.rs", "//! Cache: in-memory caching layer.\npub struct Cache;\npub fn get_cached() {}\npub enum C {A}\n");

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    let id = engineering::record_decision(
        &mut graph,
        &engineering::DecisionInput {
            decision: "Use an in-memory cache with mtime invalidation".to_string(),
            reasoning: "The cache layer is read-heavy and graphs are small".to_string(),
            alternatives: vec!["No cache".to_string(), "On-disk LRU".to_string()],
            tradeoffs: Some("memory for latency".to_string()),
            assumptions: vec!["graphs stay under ~50MB".to_string()],
            confidence: 0.8,
        },
    )
    .unwrap();

    let m = graph.get_memory(&id).expect("decision concept exists");
    assert!(m.tags.iter().any(|t| t == engineering::TAG_DECISION));
    assert!(m.content.contains("Alternatives considered"));
    assert!((m.confidence - 0.8).abs() < 0.01);

    // Linked to the architecture it concerns (Supports → cache module).
    let cache_id = unit_memory_id(&source_id, &module_key("src/cache.rs"));
    assert!(
        graph
            .get_edges(&id)
            .iter()
            .any(|e| e.kind == crate::memory_graph::EdgeKind::Supports && e.target == cache_id),
        "decision should support the cache concept"
    );

    // Future reasoning finds it before inventing a new one.
    let found = engineering::relevant_decisions(&graph, "cache invalidation approach", 3);
    assert!(found.iter().any(|(fid, _)| fid == &id), "{found:?}");

    // Re-recording the same decision evolves the same concept.
    let before = graph.memory_count();
    let id2 = engineering::record_decision(
        &mut graph,
        &engineering::DecisionInput {
            decision: "Use an in-memory cache with mtime invalidation".to_string(),
            reasoning: "Confirmed by profiling".to_string(),
            alternatives: vec![],
            tradeoffs: None,
            assumptions: vec![],
            confidence: 0.9,
        },
    )
    .unwrap();
    assert_eq!(id2, id);
    assert_eq!(graph.memory_count(), before);
}

#[test]
fn calibration_history_and_health_evolve_with_work() {
    let _guard = queues_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/alpha.rs", "//! Alpha module.\npub struct Alpha;\npub fn alpha_run() {}\npub enum A {X}\n");
    write(root, "src/beta.rs", "//! Beta module.\npub struct Beta;\npub fn beta_run() {}\npub enum B {X}\n");
    write(root, "docs/ALPHA.md", "# Alpha\n\nAlpha lives in src/alpha.rs and runs things.\n");

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    let source_id = source.source_id();
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    // Evolution history: first pass recorded.
    let state = graph.metadata.knowledge_sources.get(&source_id).unwrap();
    assert_eq!(state.history.len(), 1);
    assert!(state.history[0].concepts_created > 0);

    // Change the same two files together twice → co-change pair in health.
    for round in 0..2 {
        write(root, "src/alpha.rs", &format!("//! Alpha module.\npub struct Alpha;\npub fn alpha_run() {{}}\npub fn extra_{round}() {{}}\npub enum A {{X}}\n"));
        write(root, "src/beta.rs", &format!("//! Beta module.\npub struct Beta;\npub fn beta_run() {{}}\npub fn extra_{round}() {{}}\npub enum B {{X}}\n"));
        let mut source = RepositorySource::new(root);
        block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();
    }
    let state = graph.metadata.knowledge_sources.get(&source_id).unwrap();
    assert_eq!(state.history.len(), 3, "one point per changing pass");

    // Calibration: a brief's prediction reflected against reality updates
    // the persisted EWMA precision.
    let brief = reasoning::turn_brief_for_graph(&mut graph, "update the alpha module run path")
        .expect("alpha mention should brief");
    assert!(brief.contains("architectural context"));
    let alpha_id = unit_memory_id(&source_id, &module_key("src/alpha.rs"));
    let mut touched = std::collections::BTreeSet::new();
    touched.insert(alpha_id);
    reasoning::reflect_on_outcomes(&mut graph, &touched).expect("reflection");
    let stats = &graph.metadata.prediction_stats;
    assert_eq!(stats.reflections, 1);
    assert!(stats.precision_ewma > 0.0);

    // Health report: doc coverage sees ALPHA.md; co-change sees alpha↔beta;
    // calibration is surfaced.
    let health = insights::health_report(&graph);
    assert!(health.concepts_active > 0);
    assert!(health.doc_coverage > 0.0, "{health:?}");
    assert!(
        health
            .co_change_pairs
            .iter()
            .any(|(a, b, n)| a.contains("alpha") && b.contains("beta") && *n >= 2),
        "{:?}",
        health.co_change_pairs
    );
    assert_eq!(health.prediction_precision, Some(stats.precision_ewma));
}

#[test]
fn multiple_repositories_share_one_graph_with_boundaries() {
    let temp_a = tempfile::tempdir().unwrap();
    let temp_b = tempfile::tempdir().unwrap();
    write(temp_a.path(), "src/parser.rs", "//! Parser for the shared wire format.\npub struct Parser;\npub fn parse_frame() {}\npub enum P {A}\n");
    write(temp_b.path(), "src/consumer.rs", "//! Consumes parsed wire frames.\npub struct Consumer;\npub fn consume_frame() {}\npub enum C {A}\n");

    let mut graph = MemoryGraph::new();
    let mut src_a = RepositorySource::new(temp_a.path());
    let mut src_b = RepositorySource::new(temp_b.path());
    block_on(ingest_source_into_graph(&mut graph, &mut src_a, test_opts())).unwrap();
    block_on(ingest_source_into_graph(&mut graph, &mut src_b, test_opts())).unwrap();

    // Both sources registered with their own state (boundaries maintained).
    assert_eq!(graph.metadata.knowledge_sources.len(), 2);

    // One reasoning pass spans both repositories.
    let trace = reasoning::reason(&mut graph, "wire frame parse consume");
    let sources_hit: std::collections::BTreeSet<_> = trace
        .seeds
        .iter()
        .filter_map(|s| graph.get_memory(&s.id).and_then(|m| m.source.clone()))
        .collect();
    assert!(
        sources_hit.len() >= 2,
        "reasoning should span both repositories: {sources_hit:?}"
    );

    // Refreshing one source leaves the other's concepts untouched.
    let mut src_a = RepositorySource::new(temp_a.path());
    let report = block_on(ingest_source_into_graph(&mut graph, &mut src_a, test_opts())).unwrap();
    assert_eq!(report.concepts_retired, 0);
}

#[test]
fn verification_detects_project_checks() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "Cargo.toml", "[package]\nname='x'\n");
    let checks = verify::detect_checks(temp.path(), false);
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].0, "build");
    assert!(checks[0].1.join(" ").contains("cargo check"));
    let with_tests = verify::detect_checks(temp.path(), true);
    assert_eq!(with_tests.len(), 2);

    let temp_js = tempfile::tempdir().unwrap();
    write(temp_js.path(), "package.json", "{}");
    let js = verify::detect_checks(temp_js.path(), false);
    assert_eq!(js[0].1[0], "npm");
}

#[test]
fn concepts_participate_in_the_existing_sleep_pipeline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for i in 0..4 {
        write(
            root,
            &format!("src/web/handler_{i}.rs"),
            &format!("//! Web handler {i}.\npub struct H{i};\npub fn handle_{i}() {{}}\n"),
        );
    }

    let mut graph = MemoryGraph::new();
    let mut source = RepositorySource::new(root);
    block_on(ingest_source_into_graph(&mut graph, &mut source, test_opts())).unwrap();

    // The existing deterministic sleep pass runs over repository concepts
    // exactly as it does over conversational memories.
    let report = graph.run_sleep_cycle(crate::memory_graph::SleepConfig::default());
    assert!(report.confidence_decayed > 0);
    assert!(graph.validate().is_empty(), "ingest must keep the graph healthy");
}
