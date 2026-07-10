#![cfg_attr(test, allow(clippy::await_holding_lock))]

use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::ToSocketAddrs;

use crate::{browser, gateway, memory, storage, tui};

use super::terminal::{cleanup_tui_runtime, init_tui_runtime};

mod report_info;
mod restart;

pub use super::auth_test::run_auth_test_command;
pub(crate) use super::auth_test::run_post_login_validation;
#[cfg(test)]
pub(crate) use super::auth_test::{
    AuthTestChoicePlan, AuthTestTarget, ResolvedAuthTestTarget, auth_test_choice_plan,
    auth_test_error_is_retryable, configured_auth_test_targets, resolve_auth_test_targets,
};
pub use restart::{
    maybe_run_pending_restart_restore_on_startup, run_restart_clear_command,
    run_restart_restore_command, run_restart_save_command, run_restart_status_command,
};

pub enum AmbientSubcommand {
    Status,
    Log,
    Trigger,
    Stop,
    RunVisible,
}

pub async fn run_ambient_command(cmd: AmbientSubcommand) -> Result<()> {
    if let AmbientSubcommand::RunVisible = cmd {
        return run_ambient_visible().await;
    }

    let debug_cmd = match cmd {
        AmbientSubcommand::Status => "ambient:status",
        AmbientSubcommand::Log => "ambient:log",
        AmbientSubcommand::Trigger => "ambient:trigger",
        AmbientSubcommand::Stop => "ambient:stop",
        AmbientSubcommand::RunVisible => unreachable!(),
    };

    super::debug::run_debug_command(debug_cmd, "", None, None, false).await
}

pub async fn run_transcript_command(
    text: Option<String>,
    mode: crate::protocol::TranscriptMode,
    session: Option<String>,
) -> Result<()> {
    let text = if let Some(text) = text {
        text
    } else {
        let mut stdin = String::new();
        std::io::stdin().read_to_string(&mut stdin)?;
        let trimmed = stdin.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            anyhow::bail!("Provide transcript text as an argument or pipe it via stdin")
        }
        trimmed.to_string()
    };

    let mut client = crate::server::Client::connect_debug().await?;
    let request_id = client.send_transcript(&text, mode, session).await?;

    loop {
        match client.read_event().await? {
            crate::protocol::ServerEvent::Ack { id } if id == request_id => {}
            crate::protocol::ServerEvent::Done { id } if id == request_id => return Ok(()),
            crate::protocol::ServerEvent::Error { id, message, .. } if id == request_id => {
                anyhow::bail!(message)
            }
            _ => {}
        }
    }
}

pub async fn run_dictate_command(type_output: bool) -> Result<()> {
    let run = crate::dictation::run_configured().await?;

    if type_output {
        crate::dictation::type_text(&run.text)
    } else {
        run_transcript_command(Some(run.text), run.mode, None).await
    }
}

async fn run_ambient_visible() -> Result<()> {
    use crate::ambient::VisibleCycleContext;

    let context = VisibleCycleContext::load().map_err(|e| {
        anyhow::anyhow!(
            "Failed to load visible cycle context: {}\nIs the ambient runner running?",
            e
        )
    })?;

    let (provider, registry) = super::provider_init::init_provider_and_registry(
        &super::provider_init::ProviderChoice::Auto,
        None,
    )
    .await?;

    registry.register_ambient_tools().await;

    let safety = std::sync::Arc::new(crate::safety::SafetySystem::new());
    crate::tool::ambient::init_safety_system(safety);

    let (terminal, tui_runtime) = init_tui_runtime()?;

    let mut app = tui::App::new(provider, registry);
    app.set_ambient_mode(context.system_prompt, context.initial_message);

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle("🤖 neura ambient cycle")
    );

    let result = app.run(terminal).await;

    cleanup_tui_runtime(&tui_runtime, true);

    if let Some(cycle_result) = crate::tool::ambient::take_cycle_result() {
        let result_path = VisibleCycleContext::result_path()?;
        crate::storage::write_json(&result_path, &cycle_result)?;
        eprintln!("Ambient cycle result saved.");
    }

    result?;
    Ok(())
}

pub enum MemorySubcommand {
    List {
        scope: String,
        tag: Option<String>,
    },
    Search {
        query: String,
        semantic: bool,
    },
    Export {
        output: String,
        scope: String,
    },
    Import {
        input: String,
        scope: String,
        overwrite: bool,
    },
    Stats,
    Graph {
        max_nodes: usize,
        mermaid: bool,
    },
    Sleep {
        json: bool,
    },
    Reason {
        args: Vec<String>,
    },
    Health {
        json: bool,
    },
    Report {
        args: Vec<String>,
    },
    ClearTest,
    SidecarEnsure {
        json: bool,
    },
    Eval {
        json: bool,
    },
}

pub enum KnowledgeSubcommand {
    Ingest { path: String, full: bool, json: bool },
    Status { json: bool },
    Sync { json: bool },
    Reason { query: String, json: bool },
    Impact { target: String, json: bool },
    Insights { json: bool, record: bool },
    Reflect { json: bool },
    Goals { json: bool },
    Decision(Box<crate::knowledge::engineering::DecisionInput>),
    Plan { topic: String, json: bool },
    Verify { tests: bool, json: bool },
    Health { json: bool },
    History { query: String, json: bool },
}

/// `neura knowledge ...` — unified knowledge-source layer. Repositories (and
/// future sources) are ingested as semantic concepts in the existing memory
/// graph; status/sync expose the incremental pipeline.
pub async fn run_knowledge_command(cmd: KnowledgeSubcommand) -> Result<()> {
    use crate::knowledge::{self, IngestOptions, KnowledgeSource, repo::RepositorySource};
    use memory::MemoryManager;

    let manager = MemoryManager::new();

    match cmd {
        KnowledgeSubcommand::Ingest { path, full, json } => {
            let root = std::path::PathBuf::from(&path);
            if !root.is_dir() {
                anyhow::bail!("not a directory: {path}");
            }
            let mut source = RepositorySource::new(root);
            let opts = IngestOptions {
                full,
                ..Default::default()
            };
            let report = knowledge::ingest_source(&manager, &mut source, opts).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("{}", report.render(&source.display_name()));
                if report.items_deferred > 0 {
                    println!(
                        "{} items deferred; run `neura knowledge sync` (or let the sleep cycle) to continue.",
                        report.items_deferred
                    );
                }
                println!(
                    "Concepts now flow through the normal memory pipeline: `neura memory sleep`, `neura memory reason concept <text>`, `neura memory graph`."
                );
            }
        }

        KnowledgeSubcommand::Status { json } => {
            let graph = manager.load_project_graph()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&graph.metadata.knowledge_sources)?
                );
            } else {
                print!("{}", knowledge::render_sources_status(&graph));
                let queued = knowledge::evidence::queued_len();
                if queued > 0 {
                    println!("{queued} tool outcome(s) queued for evidence folding.");
                }
            }
        }

        KnowledgeSubcommand::Sync { json } => {
            let mut graph = manager.load_project_graph()?;
            if graph.metadata.knowledge_sources.is_empty() {
                println!("No knowledge sources registered. Run `neura knowledge ingest [path]`.");
                return Ok(());
            }
            let reports =
                knowledge::refresh_sources_in_graph(&mut graph, IngestOptions::default()).await;
            manager.save_project_graph(&graph)?;
            if json {
                let map: std::collections::BTreeMap<_, _> = reports.into_iter().collect();
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for (source_id, report) in &reports {
                    println!("{}", report.render(source_id));
                }
            }
        }

        KnowledgeSubcommand::Reason { query, json } => {
            let mut graph = manager.load_project_graph()?;
            let trace = knowledge::reasoning::reason(&mut graph, &query);
            if json {
                println!("{}", serde_json::to_string_pretty(&trace)?);
            } else {
                print!("{}", knowledge::reasoning::render_trace(&trace));
            }
        }

        KnowledgeSubcommand::Impact { target, json } => {
            let mut graph = manager.load_project_graph()?;
            let seeds = knowledge::reasoning::find_seeds(&graph, &target, 3);
            if seeds.is_empty() {
                println!("No concepts matched '{target}'.");
                return Ok(());
            }
            let ids: Vec<String> = seeds.iter().map(|s| s.id.clone()).collect();
            let model = knowledge::reasoning::impact_for(&graph, &ids, 3);
            // Impact queries are retrievals; count them like any other.
            graph.metadata.retrieval_count += 1;
            if json {
                println!("{}", serde_json::to_string_pretty(&model)?);
            } else {
                print!("{}", knowledge::reasoning::render_impact(&model));
            }
        }

        KnowledgeSubcommand::Insights { json, record } => {
            let graph = manager.load_project_graph()?;
            let insights = knowledge::insights::architecture_insights(&graph);
            if json {
                println!("{}", serde_json::to_string_pretty(&insights)?);
            } else {
                print!("{}", knowledge::insights::render_insights(&insights));
            }
            if record {
                let n = knowledge::insights::record_insights_as_evidence(&insights)?;
                println!("\nRecorded {n} observation(s) to the evidence ledger.");
            }
        }

        KnowledgeSubcommand::Reflect { json } => {
            let pending = knowledge::reasoning::pending_prediction_count();
            let blocks = crate::evidence_ledger::query_ledger(crate::evidence_ledger::LedgerQuery {
                kind: Some(crate::evidence_ledger::EvidenceKind::Reflection),
                limit: 10,
                ..Default::default()
            })
            .unwrap_or_default();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "pending_predictions": pending,
                        "recent_reflections": blocks,
                    }))?
                );
            } else {
                println!("Pending architectural predictions: {pending}");
                if blocks.is_empty() {
                    println!(
                        "No reflections recorded yet. Predictions are scored when tool \
                         evidence is folded in (memory sleep / knowledge sync)."
                    );
                } else {
                    println!("Recent prediction-vs-reality reflections:");
                    for b in blocks.iter().rev() {
                        println!(
                            "  #{} {} — {} (precision {})",
                            b.index,
                            b.subject,
                            b.summary,
                            b.score
                                .map(|s| format!("{s:.2}"))
                                .unwrap_or_else(|| "n/a".to_string()),
                        );
                    }
                }
            }
        }

        KnowledgeSubcommand::Goals { json } => {
            let mut graph = manager.load_project_graph()?;
            let (seen, links) = knowledge::engineering::sync_goals_into_graph(&mut graph, None)?;
            manager.save_project_graph(&graph)?;
            let goal_ids = knowledge::engineering::active_goal_ids(&graph);
            if json {
                let goals: Vec<_> = goal_ids
                    .iter()
                    .filter_map(|id| graph.get_memory(id))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&goals)?);
            } else {
                println!(
                    "Goal concepts in the graph: {seen} ({links} architectural link(s) refreshed)."
                );
                if goal_ids.is_empty() {
                    println!(
                        "No goal concepts yet. Create goals with the goal tool / side panel; \
                         they mirror into the graph automatically on sleep/sync."
                    );
                }
                for id in &goal_ids {
                    if let Some(m) = graph.get_memory(id) {
                        println!("  [{:.2}] {}", m.confidence, m.content.lines().next().unwrap_or(""));
                        let related: Vec<String> = graph
                            .ranked_relations(id)
                            .into_iter()
                            .take(3)
                            .map(|(kind, other, _, _)| {
                                format!(
                                    "{} {}",
                                    kind.label(),
                                    knowledge::reasoning::concept_label(&graph, &other)
                                )
                            })
                            .collect();
                        if !related.is_empty() {
                            println!("      ↳ {}", related.join(" · "));
                        }
                    }
                }
            }
        }

        KnowledgeSubcommand::Decision(input) => {
            let mut graph = manager.load_project_graph()?;
            let id = knowledge::engineering::record_decision(&mut graph, &input)?;
            manager.save_project_graph(&graph)?;
            println!("Recorded decision as concept {id} (tag `decision`), linked into the graph and the evidence ledger.");
            let related: Vec<String> = graph
                .ranked_relations(&id)
                .into_iter()
                .take(4)
                .map(|(kind, other, _, _)| {
                    format!(
                        "{} {}",
                        kind.label(),
                        knowledge::reasoning::concept_label(&graph, &other)
                    )
                })
                .collect();
            if !related.is_empty() {
                println!("Linked: {}", related.join(" · "));
            }
        }

        KnowledgeSubcommand::Plan { topic, json } => {
            let mut graph = manager.load_project_graph()?;
            let plan = knowledge::engineering::decompose(&mut graph, &topic)?;
            manager.save_project_graph(&graph)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            } else {
                print!("{}", knowledge::engineering::render_plan(&plan));
            }
        }

        KnowledgeSubcommand::Verify { tests, json } => {
            let root = std::env::current_dir()?;
            let report = knowledge::verify::run_verification(&manager, &root, tests).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", knowledge::verify::render_report(&report));
            }
            if !report.passed {
                std::process::exit(1);
            }
        }

        KnowledgeSubcommand::Health { json } => {
            let graph = manager.load_project_graph()?;
            let report = knowledge::insights::health_report(&graph);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", knowledge::insights::render_health(&report));
            }
        }

        KnowledgeSubcommand::History { query, json } => {
            let graph = manager.load_project_graph()?;
            if query.trim().is_empty() {
                // Per-source architectural evolution timeline.
                if json {
                    let map: std::collections::BTreeMap<_, _> = graph
                        .metadata
                        .knowledge_sources
                        .iter()
                        .map(|(id, s)| (id.clone(), s.history.clone()))
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&map)?);
                } else {
                    for (id, state) in &graph.metadata.knowledge_sources {
                        println!("{id}:");
                        for p in &state.history {
                            println!(
                                "  {} · {} items, {} active concepts (+{} ~{} retired {})",
                                p.at.format("%Y-%m-%d %H:%M"),
                                p.items,
                                p.active_concepts,
                                p.concepts_created,
                                p.concepts_updated,
                                p.concepts_retired
                            );
                        }
                    }
                }
            } else {
                // Concept timeline: when it appeared, how it evolved, why.
                let seeds = knowledge::reasoning::find_seeds(&graph, &query, 3);
                if seeds.is_empty() {
                    println!("No concepts matched '{query}'.");
                    return Ok(());
                }
                for seed in seeds {
                    let Some(m) = graph.get_memory(&seed.id) else { continue };
                    println!("{}", knowledge::reasoning::concept_label(&graph, &seed.id));
                    println!(
                        "  appeared {} · last evolved {} · strength {} · confidence {:.2}{}",
                        m.created_at.format("%Y-%m-%d"),
                        m.updated_at.format("%Y-%m-%d"),
                        m.strength,
                        m.confidence,
                        if m.active { "" } else { " · RETIRED" }
                    );
                    for ev in &m.evidence {
                        let what = ev.note.clone().unwrap_or_else(|| ev.id.clone());
                        println!("  {} · {}", ev.at.format("%Y-%m-%d %H:%M"), what);
                    }
                    if json {
                        println!("{}", serde_json::to_string_pretty(&m)?);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Graph reasoning operations for `neura memory reason ...`.
fn run_memory_reason(manager: &memory::MemoryManager, args: &[String]) -> Result<()> {
    use crate::memory_graph::MemoryGraph;

    let project = manager
        .load_project_graph()
        .unwrap_or_else(|_| MemoryGraph::new());
    let global = manager
        .load_global_graph()
        .unwrap_or_else(|_| MemoryGraph::new());

    // Locate which graph owns a memory id.
    let locate = |id: &str| -> Option<&MemoryGraph> {
        if project.get_memory(id).is_some() {
            Some(&project)
        } else if global.get_memory(id).is_some() {
            Some(&global)
        } else {
            None
        }
    };
    let short = |g: &MemoryGraph, id: &str| -> String {
        g.get_memory(id)
            .map(|m| m.content.chars().take(72).collect::<String>())
            .unwrap_or_else(|| id.to_string())
    };

    let op = args[0].as_str();
    match op {
        "why" | "explain" => {
            let id = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: why <memory-id>"))?;
            let Some(g) = locate(id) else {
                println!("No memory found with id '{id}'");
                return Ok(());
            };
            println!("Why '{}' matters:", short(g, id));
            println!("  importance: {:.3}", g.importance(id));
            let m = g.get_memory(id).unwrap();
            println!("  confidence: {:.3}  (evidence: {})", m.confidence, m.evidence.len());
            let relations = g.ranked_relations(id);
            if relations.is_empty() {
                println!("  (no semantic relations yet)");
            } else {
                println!("  supported / related by:");
                for (kind, target, weight, conf) in relations.iter().take(10) {
                    println!(
                        "    --{}--> [{:.2}|c{:.2}] {}",
                        kind.label(),
                        weight,
                        conf,
                        short(g, target)
                    );
                }
            }
            let contra = g.contradictions_of(id);
            if !contra.is_empty() {
                println!("  ⚠ contradicted by:");
                for c in contra {
                    println!("    {}", short(g, &c));
                }
            }
        }
        "path" => {
            let (a, b) = (
                args.get(1).ok_or_else(|| anyhow::anyhow!("usage: path <a> <b>"))?,
                args.get(2).ok_or_else(|| anyhow::anyhow!("usage: path <a> <b>"))?,
            );
            let Some(g) = locate(a).filter(|_| locate(b).is_some()) else {
                println!("Both memories must exist in the same store.");
                return Ok(());
            };
            match g.shortest_semantic_path(a, b, 6) {
                Some(path) => {
                    println!("Reasoning path ({} hops):", path.len().saturating_sub(1));
                    for (node, kind) in path {
                        match kind {
                            Some(k) => println!("   --{}--> {}", k.label(), short(g, &node)),
                            None => println!("   {}", short(g, &node)),
                        }
                    }
                }
                None => println!("No semantic path within 6 hops."),
            }
        }
        "compare" => {
            let (a, b) = (
                args.get(1).ok_or_else(|| anyhow::anyhow!("usage: compare <a> <b>"))?,
                args.get(2).ok_or_else(|| anyhow::anyhow!("usage: compare <a> <b>"))?,
            );
            let Some(g) = locate(a).filter(|_| locate(b).is_some()) else {
                println!("Both memories must exist in the same store.");
                return Ok(());
            };
            let (direct, tags, neighbours) = g.compare_memories(a, b);
            println!("Comparing:");
            println!("   A: {}", short(g, a));
            println!("   B: {}", short(g, b));
            match direct {
                Some(k) => println!("  direct relation: {}", k.label()),
                None => println!("  direct relation: none"),
            }
            println!("  shared tags: {}", if tags.is_empty() { "(none)".into() } else { tags.join(", ") });
            if neighbours.is_empty() {
                println!("  shared neighbours: (none)");
            } else {
                println!("  shared neighbours:");
                for n in neighbours.iter().take(8) {
                    println!("    {}", short(g, n));
                }
            }
        }
        "contradictions" => {
            if let Some(id) = args.get(1) {
                let Some(g) = locate(id) else {
                    println!("No memory found with id '{id}'");
                    return Ok(());
                };
                let contra = g.contradictions_of(id);
                if contra.is_empty() {
                    println!("No contradictions for '{}'.", short(g, id));
                } else {
                    println!("Contradictions of '{}':", short(g, id));
                    for c in contra {
                        println!("   {}", short(g, &c));
                    }
                }
            } else {
                let mut found = false;
                for g in [&project, &global] {
                    if let Some((a, b, conf)) = g.strongest_contradiction() {
                        println!(
                            "Strongest contradiction (confidence {:.2}):\n   {}\n   vs\n   {}",
                            conf,
                            short(g, &a),
                            short(g, &b)
                        );
                        found = true;
                    }
                }
                if !found {
                    println!("No contradictions recorded in the graph.");
                }
            }
        }
        "concept" => {
            if args.len() < 2 {
                return Err(anyhow::anyhow!("usage: concept <text...>"));
            }
            let query = args[1..].join(" ");
            match manager.find_similar_with_cascade(&query, 0.3, 8) {
                Ok(hits) if !hits.is_empty() => {
                    println!("Concept '{}' — {} related memories:", query, hits.len());
                    for (entry, score) in &hits {
                        println!("   [{:.2}] {}", score, entry.content.chars().take(72).collect::<String>());
                    }
                    // Anchor on the top hit and show its strongest relations.
                    if let Some((top, _)) = hits.first()
                        && let Some(g) = locate(&top.id)
                    {
                        let relations = g.ranked_relations(&top.id);
                        if !relations.is_empty() {
                            println!("  anchored on top hit, related concepts:");
                            for (kind, target, weight, _) in relations.iter().take(6) {
                                println!("    --{}--> [{:.2}] {}", kind.label(), weight, short(g, target));
                            }
                        }
                    }
                }
                Ok(_) => println!("No memories matched concept '{}'.", query),
                Err(e) => println!("Concept search failed: {e}"),
            }
        }
        other => {
            println!(
                "Unknown reason op '{other}'. Try: why <id> | path <a> <b> | compare <a> <b> | contradictions [id] | concept <text..>"
            );
        }
    }
    Ok(())
}

/// Graph-integrity report for `neura memory health`.
fn run_memory_health(
    project: &[crate::memory_graph::GraphIssue],
    global: &[crate::memory_graph::GraphIssue],
    json: bool,
) -> Result<()> {
    use std::collections::BTreeMap;

    let summarize = |issues: &[crate::memory_graph::GraphIssue]| -> BTreeMap<String, usize> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for i in issues {
            *counts.entry(i.category().to_string()).or_default() += 1;
        }
        counts
    };

    if json {
        let obj = serde_json::json!({
            "healthy": project.is_empty() && global.is_empty(),
            "project": {
                "issues": project.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                "by_category": summarize(project),
            },
            "global": {
                "issues": global.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
                "by_category": summarize(global),
            },
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    for (label, issues) in [("project", project), ("global", global)] {
        if issues.is_empty() {
            println!("{label} graph: ✓ healthy (no integrity issues)");
        } else {
            println!("{label} graph: ⚠ {} issue(s)", issues.len());
            for (cat, n) in summarize(issues) {
                println!("   {n:>3}  {cat}");
            }
            for i in issues.iter().take(20) {
                println!("     - {i}");
            }
            if issues.len() > 20 {
                println!("     … and {} more", issues.len() - 20);
            }
        }
    }
    Ok(())
}

/// Diagnostics for `neura memory report <kind>`.
fn run_memory_report(manager: &memory::MemoryManager, args: &[String]) -> Result<()> {
    use crate::memory_graph::{EdgeKind, MemoryGraph};

    let project = manager.load_project_graph().unwrap_or_else(|_| MemoryGraph::new());
    let global = manager.load_global_graph().unwrap_or_else(|_| MemoryGraph::new());
    let graphs: [(&str, &MemoryGraph); 2] = [("project", &project), ("global", &global)];
    let short = |g: &MemoryGraph, id: &str| -> String {
        g.get_memory(id)
            .map(|m| m.content.chars().take(72).collect::<String>())
            .unwrap_or_else(|| id.to_string())
    };

    match args[0].as_str() {
        "consolidations" | "consolidation" => {
            let mut any = false;
            for (label, g) in graphs {
                let recs = &g.metadata.consolidations;
                if recs.is_empty() {
                    continue;
                }
                any = true;
                println!("{label} — {} consolidation(s):", recs.len());
                for r in recs.iter().rev().take(20) {
                    println!(
                        "   [{}] {} ({} sources) -> {}",
                        r.at.format("%Y-%m-%d %H:%M"),
                        if r.concept.is_empty() { "concept" } else { &r.concept },
                        r.sources.len(),
                        short(g, &r.semantic_id)
                    );
                }
            }
            if !any {
                println!("No consolidations recorded yet.");
            }
        }
        "contradictions" => {
            let mut any = false;
            for (label, g) in graphs {
                let mut seen: std::collections::HashSet<(String, String)> = Default::default();
                for (from, edges) in &g.edges {
                    for e in edges {
                        if e.kind != EdgeKind::Contradicts {
                            continue;
                        }
                        let key = if *from < e.target {
                            (from.clone(), e.target.clone())
                        } else {
                            (e.target.clone(), from.clone())
                        };
                        if !seen.insert(key) {
                            continue;
                        }
                        any = true;
                        let reason = e
                            .meta
                            .evidence
                            .last()
                            .and_then(|ev| ev.note.clone())
                            .unwrap_or_else(|| "(no reason recorded)".into());
                        println!("[{label}] c{:.2}", e.meta.confidence);
                        println!("   {}", short(g, from));
                        println!("   vs {}", short(g, &e.target));
                        println!("   why: {reason}");
                    }
                }
            }
            if !any {
                println!("No contradictions discovered in the graph.");
            }
        }
        "confidence" => {
            let mut buckets = [0usize; 10];
            let mut total = 0usize;
            for (_, g) in graphs {
                for m in g.all_memories() {
                    let idx = ((m.confidence.clamp(0.0, 0.999) * 10.0) as usize).min(9);
                    buckets[idx] += 1;
                    total += 1;
                }
            }
            println!("Confidence distribution ({total} memories):");
            for (i, count) in buckets.iter().enumerate() {
                let lo = i as f32 / 10.0;
                let hi = (i as f32 + 1.0) / 10.0;
                let bar = "#".repeat((*count).min(50));
                println!("   {lo:.1}-{hi:.1} | {count:>4} {bar}");
            }
        }
        "communities" => {
            let mut comms: Vec<(String, String, u32)> = Vec::new();
            for (label, g) in graphs {
                for (id, c) in &g.clusters {
                    if id.starts_with("cluster:comm-") {
                        comms.push((
                            label.to_string(),
                            c.name.clone().unwrap_or_else(|| id.clone()),
                            c.member_count,
                        ));
                    }
                }
            }
            comms.sort_by(|a, b| b.2.cmp(&a.2));
            if comms.is_empty() {
                println!("No concept communities detected yet (run `neura memory sleep`).");
            } else {
                println!("Largest concept communities:");
                for (label, name, n) in comms.iter().take(15) {
                    println!("   [{label}] {n:>3} members — {name}");
                }
            }
        }
        "important" => {
            for (label, g) in graphs {
                let ranking = g.importance_ranking(10);
                if ranking.is_empty() {
                    continue;
                }
                println!("{label} — most important memories:");
                for (id, score) in ranking {
                    println!("   [{score:.3}] {}", short(g, &id));
                }
            }
        }
        "evidence" => {
            let id = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: report evidence <memory-id>"))?;
            let Some(g) = graphs.iter().map(|(_, g)| *g).find(|g| g.get_memory(id).is_some())
            else {
                println!("No memory found with id '{id}'");
                return Ok(());
            };
            let m = g.get_memory(id).unwrap();
            println!("Evidence chain for '{}':", short(g, id));
            println!("  confidence: {:.3}", m.confidence);
            if m.evidence.is_empty() {
                println!("  (no fact-level evidence recorded)");
            } else {
                for ev in &m.evidence {
                    let note = ev.note.clone().unwrap_or_default();
                    println!("   - {:?} {} {}", ev.kind, ev.id, note);
                }
            }
            let sources = g.derived_sources(id);
            if !sources.is_empty() {
                println!("  derived from {} episode(s):", sources.len());
                for s in sources {
                    println!("     · {}", short(g, &s));
                }
            }
        }
        "sleep" => {
            let mut any = false;
            for (label, g) in graphs {
                if let Some(r) = &g.metadata.last_sleep {
                    any = true;
                    println!("{label} — last sleep {}:", r.at.map(|a| a.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default());
                    println!("   linked {}  weakened {}  pruned {}", r.linked, r.weakened, r.pruned);
                    println!(
                        "   communities {}  consolidated {}  contradictions {}  concept-embeds {}  decayed {}",
                        r.communities,
                        r.consolidated,
                        r.contradictions_found,
                        r.concept_embeddings_refreshed,
                        r.confidence_decayed
                    );
                }
            }
            if !any {
                println!("No sleep cycle has run yet.");
            }
        }
        "health" => {
            let (p, gl) = manager.validate_graphs()?;
            run_memory_health(&p, &gl, false)?;
        }
        other => {
            println!(
                "Unknown report '{other}'. Try: consolidations | contradictions | confidence | communities | important | evidence <id> | sleep | health"
            );
        }
    }
    Ok(())
}

pub async fn run_memory_command(cmd: MemorySubcommand) -> Result<()> {
    use memory::{MemoryEntry, MemoryManager};

    let manager = MemoryManager::new();

    match cmd {
        MemorySubcommand::List { scope, tag } => {
            let mut all_memories: Vec<MemoryEntry> = Vec::new();

            if (scope == "all" || scope == "project")
                && let Ok(graph) = manager.load_project_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }
            if (scope == "all" || scope == "global")
                && let Ok(graph) = manager.load_global_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }

            if let Some(tag_filter) = tag {
                all_memories.retain(|m| m.tags.contains(&tag_filter));
            }

            all_memories.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

            if all_memories.is_empty() {
                println!("No memories found.");
            } else {
                println!("Found {} memories:\n", all_memories.len());
                for entry in &all_memories {
                    let tags_str = if entry.tags.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", entry.tags.join(", "))
                    };
                    let conf = entry.effective_confidence();
                    println!(
                        "- [{}] {}{}\n  id: {} (conf: {:.0}%, accessed: {}x)",
                        entry.category,
                        entry.content,
                        tags_str,
                        entry.id,
                        conf * 100.0,
                        entry.access_count
                    );
                    println!();
                }
            }
        }

        MemorySubcommand::Search { query, semantic } => {
            if semantic {
                // Use the graph cascade path so tag/link/cluster traversal
                // contributes (falls back to plain embedding hits when the
                // graph has no extra neighbours).
                match manager.find_similar_with_cascade(&query, 0.3, 20) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (semantic + graph cascade):\n",
                                results.len(),
                                query
                            );
                            for (entry, score) in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {} (score: {:.0}%)",
                                    entry.category,
                                    entry.content,
                                    tags_str,
                                    entry.id,
                                    score * 100.0
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            } else {
                match manager.search(&query) {
                    Ok(results) => {
                        if results.is_empty() {
                            println!("No memories found matching '{}'", query);
                        } else {
                            println!(
                                "Found {} memories matching '{}' (keyword):\n",
                                results.len(),
                                query
                            );
                            for entry in results {
                                let tags_str = if entry.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [{}]", entry.tags.join(", "))
                                };
                                println!(
                                    "- [{}] {}{}\n  id: {}",
                                    entry.category, entry.content, tags_str, entry.id
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Search failed: {}", e);
                    }
                }
            }
        }

        MemorySubcommand::Export { output, scope } => {
            let mut all_memories: Vec<memory::MemoryEntry> = Vec::new();

            if (scope == "all" || scope == "project")
                && let Ok(graph) = manager.load_project_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }
            if (scope == "all" || scope == "global")
                && let Ok(graph) = manager.load_global_graph()
            {
                all_memories.extend(graph.all_memories().cloned());
            }

            let json = serde_json::to_string_pretty(&all_memories)?;
            std::fs::write(&output, json)?;
            println!("Exported {} memories to {}", all_memories.len(), output);
        }

        MemorySubcommand::Import {
            input,
            scope,
            overwrite,
        } => {
            let content = std::fs::read_to_string(&input)?;
            let memories: Vec<memory::MemoryEntry> = serde_json::from_str(&content)?;

            let mut imported = 0;
            let mut skipped = 0;

            for entry in memories {
                let result = if scope == "global" {
                    if !overwrite
                        && let Ok(graph) = manager.load_global_graph()
                        && graph.get_memory(&entry.id).is_some()
                    {
                        skipped += 1;
                        continue;
                    }
                    manager.remember_global(entry)
                } else {
                    if !overwrite
                        && let Ok(graph) = manager.load_project_graph()
                        && graph.get_memory(&entry.id).is_some()
                    {
                        skipped += 1;
                        continue;
                    }
                    manager.remember_project(entry)
                };

                if result.is_ok() {
                    imported += 1;
                }
            }

            println!("Imported {} memories ({} skipped)", imported, skipped);
        }

        MemorySubcommand::Stats => {
            let mut project_count = 0;
            let mut global_count = 0;
            let mut total_tags = std::collections::HashSet::new();
            let mut categories: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();

            if let Ok(graph) = manager.load_project_graph() {
                project_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            if let Ok(graph) = manager.load_global_graph() {
                global_count = graph.memory_count();
                for entry in graph.all_memories() {
                    for tag in &entry.tags {
                        total_tags.insert(tag.clone());
                    }
                    *categories.entry(entry.category.to_string()).or_default() += 1;
                }
            }

            // Aggregate edge-type + link metadata across both stores.
            let mut edge_kinds: std::collections::BTreeMap<&'static str, usize> =
                std::collections::BTreeMap::new();
            let mut clusters = 0usize;
            let mut retrievals = 0u64;
            let mut links_discovered = 0u64;
            for graph in [manager.load_project_graph(), manager.load_global_graph()]
                .into_iter()
                .flatten()
            {
                for (label, n) in graph.edge_type_counts() {
                    *edge_kinds.entry(label).or_insert(0) += n;
                }
                clusters += graph.clusters.len();
                retrievals += graph.metadata.retrieval_count;
                links_discovered += graph.metadata.link_discovery_count;
            }

            println!("Memory Statistics:");
            println!("  Project memories: {}", project_count);
            println!("  Global memories:  {}", global_count);
            println!("  Total:            {}", project_count + global_count);
            println!("  Unique tags:      {}", total_tags.len());
            println!("  Clusters:         {}", clusters);
            println!("\nGraph edges (by semantic type):");
            if edge_kinds.is_empty() {
                println!("  (none yet)");
            } else {
                for (label, n) in &edge_kinds {
                    println!("  {:<13} {}", format!("{label}:"), n);
                }
            }
            println!("\nGraph activity:");
            println!("  cascade retrievals: {}", retrievals);
            println!("  links discovered:   {}", links_discovered);
            println!("\nBy category:");
            for (cat, count) in &categories {
                println!("  {}: {}", cat, count);
            }
        }

        MemorySubcommand::Graph { max_nodes, mermaid } => {
            let project = manager
                .load_project_graph()
                .unwrap_or_else(|_| crate::memory_graph::MemoryGraph::new());
            let global = manager
                .load_global_graph()
                .unwrap_or_else(|_| crate::memory_graph::MemoryGraph::new());

            if !mermaid {
                for (label, graph) in [("project", &project), ("global", &global)] {
                    if graph.memory_count() == 0 {
                        continue;
                    }
                    let e = graph.edge_type_counts();
                    println!(
                        "── {label} graph ── {} memories, {} tags, {} clusters",
                        graph.memory_count(),
                        graph.tags.len(),
                        graph.clusters.len(),
                    );
                    let edge_summary = e
                        .iter()
                        .map(|(k, n)| format!("{k}={n}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!(
                        "   edges: {}",
                        if edge_summary.is_empty() {
                            "(none)".to_string()
                        } else {
                            edge_summary
                        }
                    );
                    println!(
                        "   activity: retrievals={} links_discovered={}",
                        graph.metadata.retrieval_count, graph.metadata.link_discovery_count
                    );
                    let hubs = graph.top_hubs(5);
                    if !hubs.is_empty() {
                        println!("   top hubs:");
                        for (id, degree) in hubs {
                            let content = graph
                                .get_memory(&id)
                                .map(|m| m.content.chars().take(56).collect::<String>())
                                .unwrap_or_else(|| id.clone());
                            println!("     ({degree}) {content}");
                        }
                    }
                    println!();
                }
            }

            // Mermaid diagram of the richer (project) graph, then global if present.
            for (label, graph) in [("project", &project), ("global", &global)] {
                if graph.memory_count() == 0 {
                    continue;
                }
                let diagram = graph.to_mermaid(max_nodes);
                // Only print a diagram if it has at least one edge line.
                if diagram.lines().count() > 1 {
                    if !mermaid {
                        println!("```mermaid  # {label} associations");
                    }
                    print!("{diagram}");
                    if !mermaid {
                        println!("```\n");
                    }
                }
            }
        }

        MemorySubcommand::Sleep { json } => {
            let report = manager.run_full_sleep_cycle().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Sleep cycle complete:");
                println!("  associations linked/strengthened: {}", report.linked);
                println!("  associations weakened:            {}", report.weakened);
                println!("  associations pruned:              {}", report.pruned);
                println!("  concept communities:              {}", report.communities);
                println!("  semantic consolidations:          {}", report.consolidated);
                println!("  contradictions discovered:        {}", report.contradictions_found);
                println!(
                    "  concept embeddings refreshed:     {}",
                    report.concept_embeddings_refreshed
                );
                println!("  memories with confidence decayed: {}", report.confidence_decayed);
                if report.knowledge_concepts_refreshed > 0 {
                    println!(
                        "  knowledge concepts refreshed:     {}",
                        report.knowledge_concepts_refreshed
                    );
                }
            }
        }

        MemorySubcommand::Reason { args } => {
            run_memory_reason(&manager, &args)?;
        }

        MemorySubcommand::Health { json } => {
            let (project, global) = manager.validate_graphs()?;
            run_memory_health(&project, &global, json)?;
        }

        MemorySubcommand::Report { args } => {
            run_memory_report(&manager, &args)?;
        }

        MemorySubcommand::ClearTest => {
            let test_dir = storage::neura_dir()?.join("memory").join("test");
            if test_dir.exists() {
                let count = std::fs::read_dir(&test_dir)?.count();
                std::fs::remove_dir_all(&test_dir)?;
                println!("Cleared test memory storage ({} files)", count);
            } else {
                println!("Test memory storage is already empty");
            }
        }
        MemorySubcommand::SidecarEnsure { json } => {
            let cfg = crate::local_model::LocalModelConfig::default();
            let status = crate::local_model::ensure_local_model_server(&cfg)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "ok={} url={} model={} model_path={} message={}",
                    status.ok,
                    status.base_url,
                    status.model,
                    status
                        .model_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    status.message
                );
            }
        }
        MemorySubcommand::Eval { json } => {
            let report = crate::memory_eval::run_memory_eval();
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "memory eval: {}/{} accuracy={:.2}",
                    report.passed, report.total, report.accuracy
                );
            }
        }
    }

    Ok(())
}

pub fn run_pair_command(list: bool, revoke: Option<String>) -> Result<()> {
    let mut registry = gateway::DeviceRegistry::load();

    if list {
        if registry.devices.is_empty() {
            eprintln!("No paired devices.");
        } else {
            eprintln!("\x1b[1mPaired devices:\x1b[0m\n");
            for device in &registry.devices {
                let last_seen = &device.last_seen;
                eprintln!("  \x1b[36m{}\x1b[0m  ({})", device.name, device.id);
                eprintln!("    Paired: {}  Last seen: {}", device.paired_at, last_seen);
                if let Some(ref apns) = device.apns_token {
                    eprintln!("    APNs: {}...", &apns[..apns.len().min(16)]);
                }
                eprintln!();
            }
        }
        return Ok(());
    }

    if let Some(ref target) = revoke {
        let before = registry.devices.len();
        registry
            .devices
            .retain(|d| d.id != *target && d.name != *target);
        if registry.devices.len() < before {
            registry.save()?;
            eprintln!("\x1b[32m✓\x1b[0m Revoked device: {}", target);
        } else {
            eprintln!("\x1b[31m✗\x1b[0m No device found matching: {}", target);
        }
        return Ok(());
    }

    let gw_config = &crate::config::config().gateway;

    if !gw_config.enabled {
        eprintln!("\x1b[33m⚠\x1b[0m  Gateway is disabled. Enable it in ~/.neura/config.toml:\n");
        eprintln!("    \x1b[2m[gateway]\x1b[0m");
        eprintln!("    \x1b[2menabled = true\x1b[0m");
        eprintln!("    \x1b[2mport = {}\x1b[0m\n", gw_config.port);
        eprintln!("  Then restart the neura server.\n");
    }

    let code = registry.generate_pairing_code();
    let connect_host = resolve_connect_host(&gw_config.bind_addr);
    let pair_uri = format!(
        "neura://pair?host={}&port={}&code={}",
        connect_host, gw_config.port, code
    );

    eprintln!();
    eprintln!("  \x1b[1mScan with the neura iOS app:\x1b[0m\n");
    if qr2term::print_qr(&pair_uri).is_err() {
        eprintln!("  \x1b[33m(QR code generation failed)\x1b[0m\n");
    }
    eprintln!();
    eprintln!(
        "  Pairing code:  \x1b[1;37m{} {}\x1b[0m   \x1b[2m(expires in 5 minutes)\x1b[0m",
        &code[..3],
        &code[3..]
    );
    let resolved_hint = format!("{}:{}", connect_host, gw_config.port);
    let bind_hint = format!("{}:{}", gw_config.bind_addr, gw_config.port);
    eprintln!("  Connect host:  \x1b[36m{}\x1b[0m", resolved_hint);
    if connect_host != gw_config.bind_addr {
        eprintln!("  Bind address:  \x1b[2m{}\x1b[0m", bind_hint);
    }

    if connect_host == "<your-mac-hostname>" {
        eprintln!(
            "\n  \x1b[33mTip:\x1b[0m set NEURA_GATEWAY_HOST to your reachable Tailscale hostname."
        );
    }

    if (gw_config.bind_addr.as_str(), gw_config.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .is_none()
    {
        eprintln!(
            "  \x1b[33mWarning:\x1b[0m gateway bind address appears invalid: {}",
            bind_hint
        );
    }
    eprintln!();

    Ok(())
}

pub fn resolve_connect_host(bind_addr: &str) -> String {
    if bind_addr == "0.0.0.0" || bind_addr == "::" {
        if let Some(host) = std::env::var("NEURA_GATEWAY_HOST")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return host;
        }

        if let Some(host) = detect_tailscale_dns_name() {
            return host;
        }

        return std::env::var("HOSTNAME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "<your-mac-hostname>".to_string());
    }
    bind_addr.to_string()
}

pub fn parse_tailscale_dns_name(status_json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(status_json).ok()?;
    let dns_name = value
        .get("Self")?
        .get("DNSName")?
        .as_str()?
        .trim()
        .trim_end_matches('.')
        .to_string();

    if dns_name.is_empty() {
        None
    } else {
        Some(dns_name)
    }
}

pub fn detect_tailscale_dns_name() -> Option<String> {
    let output = std::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_tailscale_dns_name(&output.stdout)
}

pub async fn run_browser(action: &str) -> Result<()> {
    match action {
        "setup" => browser::run_setup_command().await?,
        "status" => {
            let status = browser::ensure_browser_ready_noninteractive().await?;
            println!("Browser automation");
            println!("  backend: {}", status.backend);
            println!("  browser: {}", status.browser);
            println!(
                "  binary: {}",
                if status.binary_installed {
                    "installed"
                } else {
                    "missing"
                }
            );
            println!(
                "  setup: {}",
                if status.setup_complete {
                    "complete"
                } else {
                    "not complete"
                }
            );
            println!(
                "  bridge: {}",
                if status.responding {
                    "responding"
                } else {
                    "not responding"
                }
            );
            println!(
                "  compatibility: {}",
                if status.compatible {
                    "ok"
                } else {
                    "extension/bridge mismatch"
                }
            );
            if !status.missing_actions.is_empty() {
                println!("  missing actions: {}", status.missing_actions.join(", "));
            }

            if status.ready {
                println!("\nBuilt-in browser tool is ready.");
            } else if status.responding && !status.compatible {
                println!(
                    "\nThe browser bridge is connected, but the installed Firefox extension is out of date for this neura build. Run `neura browser setup` to repair or update it."
                );
            } else {
                println!("\nRun `neura browser setup` to install or repair it.");
            }
        }
        other => {
            eprintln!("Unknown browser action: {}", other);
            eprintln!("Available: setup, status");
            std::process::exit(1);
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct ModelListReport {
    provider: String,
    selected_model: String,
    models: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RunCommandReport {
    session_id: String,
    provider: String,
    model: String,
    text: String,
    usage: crate::agent::TokenUsage,
}

#[derive(Debug, Default)]
struct NdjsonRunState {
    text: String,
    session_id: Option<String>,
    upstream_provider: Option<String>,
    connection_type: Option<String>,
    connection_phase: Option<String>,
    status_detail: Option<String>,
    usage: crate::agent::TokenUsage,
}

pub fn run_auth_status_command(emit_json: bool) -> Result<()> {
    report_info::run_auth_status_command(emit_json)
}

pub async fn run_auth_doctor_command(
    provider_arg: Option<&str>,
    validate: bool,
    emit_json: bool,
) -> Result<()> {
    report_info::run_auth_doctor_command(provider_arg, validate, emit_json).await
}

pub fn run_provider_list_command(emit_json: bool) -> Result<()> {
    report_info::run_provider_list_command(emit_json)
}

pub async fn run_provider_current_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    report_info::run_provider_current_command(choice, model, emit_json).await
}

pub fn run_version_command(emit_json: bool) -> Result<()> {
    report_info::run_version_command(emit_json)
}

pub async fn run_usage_command(emit_json: bool) -> Result<()> {
    report_info::run_usage_command(emit_json).await
}

pub async fn run_single_message_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    resume_session: Option<&str>,
    message: &str,
    emit_json: bool,
    emit_ndjson: bool,
) -> Result<()> {
    let provider = if emit_json || emit_ndjson {
        super::provider_init::init_provider_quiet(choice, model).await?
    } else {
        super::provider_init::init_provider_for_validation(choice, model).await?
    };
    let registry = crate::tool::Registry::new(provider.clone()).await;
    let mut agent = crate::agent::Agent::new(provider.clone(), registry);
    restore_agent_session_if_requested(&mut agent, resume_session)?;

    if emit_json {
        let text = agent.run_once_capture(message).await?;
        let report = RunCommandReport {
            session_id: agent.session_id().to_string(),
            provider: provider.name().to_string(),
            model: provider.model(),
            text,
            usage: agent.last_usage().clone(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if emit_ndjson {
        run_single_message_command_ndjson(&mut agent, provider.clone(), message).await?;
    } else {
        agent.run_once(message).await?;
    }

    Ok(())
}

fn restore_agent_session_if_requested(
    agent: &mut crate::agent::Agent,
    resume_session: Option<&str>,
) -> Result<()> {
    if let Some(session_id) = resume_session {
        agent.restore_session(session_id)?;
    }
    Ok(())
}

async fn run_single_message_command_ndjson(
    agent: &mut crate::agent::Agent,
    provider: std::sync::Arc<dyn crate::provider::Provider>,
    message: &str,
) -> Result<()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let session_id = agent.session_id().to_string();
    let mut run_future =
        std::pin::pin!(agent.run_once_streaming_mpsc(message, Vec::new(), None, event_tx,));
    let mut stdout = std::io::stdout().lock();
    let mut state = NdjsonRunState {
        session_id: Some(session_id.clone()),
        ..NdjsonRunState::default()
    };
    write_json_line(
        &mut stdout,
        &serde_json::json!({
            "type": "start",
            "session_id": session_id,
            "provider": provider.name(),
            "model": provider.model(),
        }),
    )?;

    let mut run_result: Option<Result<()>> = None;
    loop {
        tokio::select! {
            result = &mut run_future, if run_result.is_none() => {
                run_result = Some(result);
            }
            event = event_rx.recv() => {
                match event {
                    Some(event) => emit_ndjson_event(&mut stdout, &mut state, event)?,
                    None => break,
                }
            }
        }
    }

    let result = run_result.unwrap_or(Ok(()));
    match result {
        Ok(()) => {
            write_json_line(
                &mut stdout,
                &serde_json::json!({
                    "type": "done",
                    "session_id": session_id,
                    "provider": provider.name(),
                    "model": provider.model(),
                    "text": state.text,
                    "usage": state.usage,
                    "upstream_provider": state.upstream_provider,
                    "connection_type": state.connection_type,
                    "connection_phase": state.connection_phase,
                    "status_detail": state.status_detail,
                }),
            )?;
            Ok(())
        }
        Err(err) => {
            write_json_line(
                &mut stdout,
                &serde_json::json!({
                    "type": "error",
                    "session_id": session_id,
                    "provider": provider.name(),
                    "model": provider.model(),
                    "message": format!("{err:#}"),
                }),
            )?;
            Err(err)
        }
    }
}

fn emit_ndjson_event(
    stdout: &mut impl Write,
    state: &mut NdjsonRunState,
    event: crate::protocol::ServerEvent,
) -> Result<()> {
    use crate::protocol::ServerEvent;

    match event {
        ServerEvent::TextDelta { text } => {
            state.text.push_str(&text);
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "text_delta", "text": text }),
            )
        }
        ServerEvent::TextReplace { text } => {
            state.text = text.clone();
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "text_replace", "text": text }),
            )
        }
        ServerEvent::ToolStart { id, name } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_start", "id": id, "name": name }),
        ),
        ServerEvent::ToolInput { delta } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_input", "delta": delta }),
        ),
        ServerEvent::ToolExec { id, name } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "tool_exec", "id": id, "name": name }),
        ),
        ServerEvent::ToolDone {
            id,
            name,
            output,
            error,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "tool_done",
                "id": id,
                "name": name,
                "output": output,
                "error": error,
            }),
        ),
        ServerEvent::TokenUsage {
            input,
            output,
            cache_read_input,
            cache_creation_input,
        } => {
            state.usage = crate::agent::TokenUsage {
                input_tokens: input,
                output_tokens: output,
                cache_read_input_tokens: cache_read_input,
                cache_creation_input_tokens: cache_creation_input,
            };
            write_json_line(
                stdout,
                &serde_json::json!({
                    "type": "tokens",
                    "input": input,
                    "output": output,
                    "cache_read_input": cache_read_input,
                    "cache_creation_input": cache_creation_input,
                }),
            )
        }
        ServerEvent::ConnectionType { connection } => {
            state.connection_type = Some(connection.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "connection_type", "connection": connection }),
            )
        }
        ServerEvent::ConnectionPhase { phase } => {
            state.connection_phase = Some(phase.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "connection_phase", "phase": phase }),
            )
        }
        ServerEvent::StatusDetail { detail } => {
            state.status_detail = Some(detail.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "status_detail", "detail": detail }),
            )
        }
        ServerEvent::MessageEnd => {
            write_json_line(stdout, &serde_json::json!({ "type": "message_end" }))
        }
        ServerEvent::UpstreamProvider { provider } => {
            state.upstream_provider = Some(provider.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "upstream_provider", "provider": provider }),
            )
        }
        ServerEvent::SessionId { session_id } => {
            state.session_id = Some(session_id.clone());
            write_json_line(
                stdout,
                &serde_json::json!({ "type": "session", "session_id": session_id }),
            )
        }
        ServerEvent::Compaction {
            trigger,
            pre_tokens,
            messages_dropped,
            post_tokens,
            tokens_saved,
            duration_ms,
            messages_compacted,
            summary_chars,
            active_messages,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "compaction",
                "trigger": trigger,
                "pre_tokens": pre_tokens,
                "messages_dropped": messages_dropped,
                "post_tokens": post_tokens,
                "tokens_saved": tokens_saved,
                "duration_ms": duration_ms,
                "messages_compacted": messages_compacted,
                "summary_chars": summary_chars,
                "active_messages": active_messages,
            }),
        ),
        ServerEvent::MemoryInjected {
            count,
            prompt_chars,
            computed_age_ms,
            ..
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "memory_injected",
                "count": count,
                "prompt_chars": prompt_chars,
                "computed_age_ms": computed_age_ms,
            }),
        ),
        ServerEvent::Interrupted => {
            write_json_line(stdout, &serde_json::json!({ "type": "interrupted" }))
        }
        ServerEvent::SoftInterruptInjected {
            content,
            display_role,
            point,
            tools_skipped,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "soft_interrupt_injected",
                "content": content,
                "display_role": display_role,
                "point": point,
                "tools_skipped": tools_skipped,
            }),
        ),
        ServerEvent::BatchProgress { progress } => write_json_line(
            stdout,
            &serde_json::json!({ "type": "batch_progress", "progress": progress }),
        ),
        ServerEvent::Error {
            message,
            retry_after_secs,
            ..
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "error",
                "message": message,
                "retry_after_secs": retry_after_secs,
            }),
        ),
        ServerEvent::SubtextLatent {
            phase,
            token,
            latent,
            text,
        } => write_json_line(
            stdout,
            &serde_json::json!({
                "type": "subtext_latent",
                "phase": phase,
                "token": token,
                "latent": latent,
                "text": text,
            }),
        ),
        ServerEvent::Ack { .. } | ServerEvent::Done { .. } | ServerEvent::Pong { .. } => Ok(()),
        _ => Ok(()),
    }
}

fn write_json_line(stdout: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub async fn run_model_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
    verbose: bool,
) -> Result<()> {
    let provider = super::provider_init::init_provider_quiet(choice, model).await?;

    if let Err(err) = provider.prefetch_models().await
        && !super::output::quiet_enabled()
    {
        eprintln!("Warning: failed to refresh dynamic model list: {}", err);
    }

    let routes = provider.model_routes();
    let models = collect_cli_model_names(&routes, provider.available_models_display());

    if models.is_empty() {
        anyhow::bail!(
            "No models found for provider '{}'. Check credentials or try a different --provider.",
            provider.name()
        );
    }

    if emit_json {
        let report = ModelListReport {
            provider: provider.name().to_string(),
            selected_model: provider.model(),
            models,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        if verbose {
            println!("Provider: {}", provider.name());
            println!("Selected model: {}", provider.model());
            println!("Available models: {}", models.len());
            println!();
        }
        for model in models {
            println!("{}", model);
        }
    }

    Ok(())
}

fn collect_cli_model_names(
    routes: &[crate::provider::ModelRoute],
    display_models: Vec<String>,
) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = BTreeSet::new();

    fn push_model(deduped: &mut Vec<String>, seen: &mut BTreeSet<String>, model: &str) {
        let trimmed = model.trim();
        if !crate::provider::is_listable_model_name(trimmed) {
            return;
        }
        if seen.insert(trimmed.to_string()) {
            deduped.push(trimmed.to_string());
        }
    }

    for route in routes.iter().filter(|route| route.available) {
        push_model(&mut deduped, &mut seen, &route.model);
    }

    if deduped.is_empty() {
        for route in routes {
            push_model(&mut deduped, &mut seen, &route.model);
        }
    }

    for model in display_models {
        push_model(&mut deduped, &mut seen, &model);
    }

    deduped
}
#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
