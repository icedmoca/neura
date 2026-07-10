//! Engineering intent inside the semantic graph: goals, architectural
//! decisions, and long-horizon plans.
//!
//! Nothing here is a new store. Goals already exist (`src/goal.rs`, the goal
//! tool, the side panel); this module *bridges* them into the graph as
//! first-class concepts so work, plans, and repository knowledge connect
//! back to the "why". Decisions and plans are ordinary memories with
//! deterministic ids — they retrieve, consolidate, decay, and gain evidence
//! exactly like every other concept, and they surface in reasoning traces
//! and turn briefs with zero special-casing.

use super::reasoning::{concept_label, find_seeds, impact_for};
use super::unit_memory_id;
use crate::memory::{MemoryCategory, MemoryEntry};
use crate::memory_graph::{EdgeKind, EdgeSource, EvidenceRef, MemoryGraph};
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Marker tags. Goals/decisions/plans are found by tag, never by a side table.
pub const TAG_GOAL: &str = "goal";
pub const TAG_DECISION: &str = "decision";
pub const TAG_PLAN: &str = "plan";

/// Virtual source id for engineering-intent concepts (`mem-src-…` ids are
/// derived from it, keeping the deterministic-id convention).
const INTENT_SOURCE: &str = "intent";

pub fn decision_memory_id(slug: &str) -> String {
    unit_memory_id(INTENT_SOURCE, &format!("decision:{slug}"))
}

pub fn plan_memory_id(topic: &str) -> String {
    unit_memory_id(INTENT_SOURCE, &format!("plan:{}", topic.trim().to_lowercase()))
}

/// Link an intent concept to the architectural concepts its text matches.
/// Deterministic (absolute token overlap — intent texts are long); weight
/// scaled by match strength.
fn link_to_matched_concepts(graph: &mut MemoryGraph, id: &str, text: &str, kind: EdgeKind) -> usize {
    let seeds = super::reasoning::match_concepts_by_overlap(graph, text, 5);
    let mut linked = 0;
    for seed in seeds {
        if seed.id == id {
            continue;
        }
        graph.add_typed_edge(
            id,
            &seed.id,
            kind,
            (0.4 + 0.4 * seed.score).clamp(0.0, 1.0),
            EdgeSource::System,
            Some(EvidenceRef::observation("matched by engineering-intent text")),
        );
        linked += 1;
    }
    linked
}

fn upsert_intent_memory(
    graph: &mut MemoryGraph,
    id: &str,
    content: String,
    tags: Vec<String>,
    evidence_note: String,
) -> bool {
    let created = graph.get_memory(id).is_none();
    if created {
        let mut entry = MemoryEntry::new(MemoryCategory::Custom("engineering".to_string()), content);
        entry.id = id.to_string();
        entry.tags = tags;
        entry.source = Some(INTENT_SOURCE.to_string());
        entry.confidence = 0.7;
        entry.refresh_search_text();
        graph.add_memory(entry);
    } else if let Some(m) = graph.get_memory_mut(id) {
        if m.content != content {
            m.content = content;
            m.embedding = None;
            m.concept_embedding = None;
            m.updated_at = Utc::now();
        }
        m.active = true;
    }
    if let Some(m) = graph.get_memory_mut(id) {
        m.record_evidence(EvidenceRef::observation(evidence_note));
    }
    created
}

// ==================== Goals ====================

/// Connect the project's goal concepts to the architecture they concern.
///
/// Goal *memories* already exist — `src/goal.rs` mirrors every goal into the
/// memory store as `goal:<id>` entries (tag `goal`) whenever goals are
/// created or updated. What that sync does not do is graph reasoning: this
/// pass adds `SimilarTo` edges from each active goal concept to the
/// architectural concepts its text matches, so plans, briefs, and impact
/// analysis can traverse from "why" to "where". Idempotent; edges reinforce
/// rather than duplicate. Returns (goals_seen, links_added).
pub fn sync_goals_into_graph(
    graph: &mut MemoryGraph,
    working_dir: Option<&Path>,
) -> Result<(usize, usize)> {
    // Refresh goal memories from goal storage first (covers graphs loaded in
    // contexts where goal.rs hasn't synced yet, e.g. sleep on a server).
    // list_relevant_goals is best-effort: no goals directory → empty.
    let _ = crate::goal::list_relevant_goals(working_dir);

    let goal_texts: Vec<(String, String)> = graph
        .active_memories()
        .filter(|m| m.tags.iter().any(|t| t == TAG_GOAL))
        .map(|m| (m.id.clone(), m.content.clone()))
        .collect();
    let seen = goal_texts.len();
    let mut links = 0usize;
    for (id, content) in goal_texts {
        links += link_to_matched_concepts(graph, &id, &content, EdgeKind::SimilarTo);
    }
    if seen > 0 {
        crate::memory_log::log_knowledge(
            "knowledge_goal_sync",
            serde_json::json!({ "goals": seen, "links": links }),
        );
    }
    Ok((seen, links))
}

/// Goal concepts currently active in the graph (for briefs / plans).
pub fn active_goal_ids(graph: &MemoryGraph) -> Vec<String> {
    graph
        .active_memories()
        .filter(|m| m.tags.iter().any(|t| t == TAG_GOAL))
        .map(|m| m.id.clone())
        .collect()
}

// ==================== Architectural decisions ====================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionInput {
    pub decision: String,
    pub reasoning: String,
    pub alternatives: Vec<String>,
    pub tradeoffs: Option<String>,
    pub assumptions: Vec<String>,
    /// 0.0–1.0 stated confidence in the decision.
    pub confidence: f32,
}

fn slugify(text: &str) -> String {
    let mut slug: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    slug.truncate(48);
    slug.trim_matches('-').to_string()
}

/// Preserve an architectural decision as a first-class concept: structured
/// content, links to the concepts and goals it concerns, and an append-only
/// evidence-ledger block. Re-recording the same decision text updates the
/// same concept (decisions evolve; history stays in the ledger).
pub fn record_decision(graph: &mut MemoryGraph, input: &DecisionInput) -> Result<String> {
    let slug = slugify(&input.decision);
    if slug.is_empty() {
        anyhow::bail!("decision text is empty");
    }
    let id = decision_memory_id(&slug);

    let mut content = format!("Decision: {}", input.decision.trim());
    if !input.reasoning.trim().is_empty() {
        content.push_str(&format!("\nReasoning: {}", input.reasoning.trim()));
    }
    if !input.alternatives.is_empty() {
        content.push_str(&format!(
            "\nAlternatives considered: {}",
            input.alternatives.join("; ")
        ));
    }
    if let Some(t) = input.tradeoffs.as_deref().filter(|t| !t.trim().is_empty()) {
        content.push_str(&format!("\nTradeoffs: {}", t.trim()));
    }
    if !input.assumptions.is_empty() {
        content.push_str(&format!("\nAssumptions: {}", input.assumptions.join("; ")));
    }

    upsert_intent_memory(
        graph,
        &id,
        content,
        vec![TAG_DECISION.to_string()],
        "engineering decision recorded".to_string(),
    );
    if let Some(m) = graph.get_memory_mut(&id) {
        m.confidence = input.confidence.clamp(0.0, 1.0);
    }

    // The decision supports the architecture it concerns and any active goals
    // whose text it matches.
    let match_text = format!("{} {}", input.decision, input.reasoning);
    link_to_matched_concepts(graph, &id, &match_text, EdgeKind::Supports);

    if !cfg!(test) {
        let _ = crate::evidence_ledger::append_evidence(
            crate::evidence_ledger::EvidenceKind::EngineeringDecision,
            format!("decision:{slug}"),
            input.decision.trim().to_string(),
            Some(input.confidence as f64),
            None,
            input,
        );
    }
    crate::memory_log::log_knowledge(
        "knowledge_decision",
        serde_json::json!({ "id": id, "decision": input.decision }),
    );
    Ok(id)
}

/// Prior decisions relevant to `query`, most-relevant first — so reasoning
/// can reuse existing decisions before inventing new ones.
pub fn relevant_decisions(graph: &MemoryGraph, query: &str, limit: usize) -> Vec<(String, String)> {
    find_seeds(graph, query, limit * 3)
        .into_iter()
        .filter(|hit| {
            graph
                .get_memory(&hit.id)
                .map(|m| m.tags.iter().any(|t| t == TAG_DECISION))
                .unwrap_or(false)
        })
        .take(limit)
        .map(|hit| (hit.id.clone(), concept_label(graph, &hit.id)))
        .collect()
}

// ==================== Long-horizon plans ====================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStage {
    pub order: usize,
    pub concept_id: String,
    pub concept: String,
    /// Why this stage exists (dependency chain justification).
    pub rationale: String,
    /// Tests covering this stage's area.
    pub verification: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineeringPlan {
    pub topic: String,
    pub memory_id: String,
    pub stages: Vec<PlanStage>,
    /// Impact uncertainty from the typed-edge closure.
    pub uncertainty: f32,
    /// Coarse complexity: concepts touched, weighted by uncertainty.
    pub complexity: f32,
    /// Historical prediction precision (EWMA), for calibrated expectations.
    pub calibration: Option<f32>,
    /// Goals this plan advances (concept labels).
    pub goals: Vec<String>,
}

/// Decompose an engineering topic into dependency-ordered stages over the
/// architectural concepts it touches. Deterministic: seeds → impact closure →
/// topological order over `DependsOn` (dependencies first), stable
/// tie-breaking by id. The plan persists as an evolving concept: same topic →
/// same memory id, refreshed rather than regenerated.
pub fn decompose(graph: &mut MemoryGraph, topic: &str) -> Result<EngineeringPlan> {
    let seeds = find_seeds(graph, topic, 4);
    if seeds.is_empty() {
        anyhow::bail!("no architectural concepts matched '{topic}' — ingest the repository first");
    }
    let seed_ids: Vec<String> = seeds.iter().map(|s| s.id.clone()).collect();
    let impact = impact_for(graph, &seed_ids, 2);

    // Work set: targets + affected concepts (dependents/containers).
    let mut work: Vec<String> = seed_ids.clone();
    work.extend(
        impact
            .affected
            .iter()
            .filter(|a| a.confidence >= 0.2 && a.via == "depends_on")
            .map(|a| a.id.clone()),
    );
    work.sort();
    work.dedup();

    // Topological order over DependsOn restricted to the work set:
    // if A depends on B (A —DependsOn→ B), B is staged before A.
    let in_set: std::collections::BTreeSet<&String> = work.iter().collect();
    let mut ordered: Vec<String> = Vec::new();
    let mut placed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut remaining: Vec<String> = work.clone();
    while !remaining.is_empty() {
        let mut progressed = false;
        let mut next_remaining = Vec::new();
        for id in remaining {
            let unmet = graph.get_edges(&id).iter().any(|e| {
                e.kind == EdgeKind::DependsOn
                    && in_set.contains(&e.target)
                    && !placed.contains(&e.target)
            });
            if unmet {
                next_remaining.push(id);
            } else {
                placed.insert(id.clone());
                ordered.push(id);
                progressed = true;
            }
        }
        if !progressed {
            // Dependency cycle within the work set: fall back to stable order
            // for the rest (the cycle itself is reported by insights).
            next_remaining.sort();
            ordered.extend(next_remaining);
            break;
        }
        remaining = next_remaining;
    }

    let calibration = {
        let stats = &graph.metadata.prediction_stats;
        if stats.reflections > 0 {
            Some(stats.precision_ewma)
        } else {
            None
        }
    };

    let stages: Vec<PlanStage> = ordered
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let deps: Vec<String> = graph
                .get_edges(id)
                .iter()
                .filter(|e| e.kind == EdgeKind::DependsOn && in_set.contains(&e.target))
                .map(|e| concept_label(graph, &e.target))
                .collect();
            let rationale = if seed_ids.contains(id) {
                if deps.is_empty() {
                    "directly matched target".to_string()
                } else {
                    format!("target; staged after its dependencies: {}", deps.join("; "))
                }
            } else {
                "dependent of a target — verify after upstream changes".to_string()
            };
            let sub_impact = impact_for(graph, std::slice::from_ref(id), 1);
            let confidence = graph.get_memory(id).map(|m| m.confidence).unwrap_or(0.5);
            PlanStage {
                order: i + 1,
                concept: concept_label(graph, id),
                concept_id: id.clone(),
                rationale,
                verification: sub_impact.likely_tests,
                confidence,
            }
        })
        .collect();

    let complexity = stages.len() as f32 * (1.0 + impact.uncertainty);
    let topic_matches: std::collections::BTreeSet<String> = find_seeds(graph, topic, 8)
        .into_iter()
        .map(|s| s.id)
        .collect();
    let goals: Vec<String> = active_goal_ids(graph)
        .into_iter()
        .filter(|gid| topic_matches.contains(gid))
        .map(|gid| concept_label(graph, &gid))
        .collect();

    let plan = EngineeringPlan {
        topic: topic.to_string(),
        memory_id: plan_memory_id(topic),
        stages,
        uncertainty: impact.uncertainty,
        complexity,
        calibration,
        goals,
    };

    // Persist as an evolving concept linked to its stages and goals.
    let content = render_plan_content(&plan);
    upsert_intent_memory(
        graph,
        &plan.memory_id.clone(),
        content,
        vec![TAG_PLAN.to_string()],
        "plan decomposed from architectural impact".to_string(),
    );
    let stage_ids: Vec<String> = plan.stages.iter().map(|s| s.concept_id.clone()).collect();
    for sid in &stage_ids {
        graph.add_typed_edge(
            &plan.memory_id,
            sid,
            EdgeKind::Uses,
            0.7,
            EdgeSource::System,
            Some(EvidenceRef::observation("plan stage")),
        );
    }

    crate::memory_log::log_knowledge(
        "knowledge_plan",
        serde_json::json!({
            "topic": plan.topic,
            "stages": plan.stages.len(),
            "uncertainty": plan.uncertainty,
            "complexity": plan.complexity,
        }),
    );
    Ok(plan)
}

fn render_plan_content(plan: &EngineeringPlan) -> String {
    let mut out = format!(
        "Plan: {} — {} stage(s), complexity {:.1}, uncertainty {:.2}",
        plan.topic,
        plan.stages.len(),
        plan.complexity,
        plan.uncertainty
    );
    for s in &plan.stages {
        out.push_str(&format!("\n{}. {}", s.order, s.concept));
    }
    out
}

pub fn render_plan(plan: &EngineeringPlan) -> String {
    let mut out = format!(
        "Engineering plan for: {}\ncomplexity {:.1} · uncertainty {:.2}{}\n",
        plan.topic,
        plan.complexity,
        plan.uncertainty,
        plan.calibration
            .map(|c| format!(" · historical prediction precision {c:.2}"))
            .unwrap_or_default(),
    );
    if !plan.goals.is_empty() {
        out.push_str(&format!("Advances goals: {}\n", plan.goals.join("; ")));
    }
    for s in &plan.stages {
        out.push_str(&format!(
            "\n{}. [{:.2}] {}\n   why: {}\n",
            s.order, s.confidence, s.concept, s.rationale
        ));
        if !s.verification.is_empty() {
            out.push_str(&format!("   verify: {}\n", s.verification.join("; ")));
        }
    }
    out.push_str(
        "\nThe plan is a concept (tag `plan`): completed work reinforces it through tool \
         evidence and reflection; re-running the same topic evolves it in place.\n",
    );
    out
}
