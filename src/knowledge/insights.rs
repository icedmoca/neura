//! Architectural intelligence: deterministic observations about the evolving
//! architecture, computed purely from the semantic graph.
//!
//! This module *observes* — it never modifies code and never mutates the
//! graph. Insights can be recorded to the evidence ledger (append-only) so
//! architectural understanding accrues explicit history: emerging hubs,
//! coupling growth, weak/strong communities, dead or duplicate concepts, and
//! documentation drifting behind the modules it describes.

use crate::memory_graph::{EdgeKind, MemoryGraph};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const MAX_PER_KIND: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightKind {
    /// Concept with outsized importance — a change here ripples widely.
    HighCentrality,
    /// Module with many dependency edges in+out — coupling hotspot.
    CouplingHotspot,
    /// Community large and cohesive enough to be a real subsystem boundary.
    StrongCommunity,
    /// Community too small to be meaningful — a fragmenting abstraction.
    WeakCommunity,
    /// Active concept with no relations and no recorded use.
    DeadConcept,
    /// Two active concepts whose content is near-identical.
    DuplicateAbstraction,
    /// Documentation older than the module it supports by a wide margin.
    DocDrift,
}

impl InsightKind {
    pub fn label(&self) -> &'static str {
        match self {
            InsightKind::HighCentrality => "high_centrality",
            InsightKind::CouplingHotspot => "coupling_hotspot",
            InsightKind::StrongCommunity => "strong_community",
            InsightKind::WeakCommunity => "weak_community",
            InsightKind::DeadConcept => "dead_concept",
            InsightKind::DuplicateAbstraction => "duplicate_abstraction",
            InsightKind::DocDrift => "doc_drift",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchInsight {
    pub kind: InsightKind,
    /// Memory / cluster id the observation is about.
    pub subject_id: String,
    pub subject: String,
    pub detail: String,
    /// Kind-relative magnitude, for ordering within a kind.
    pub score: f32,
}

/// Compute the current set of architectural observations. Deterministic for
/// a given graph state; read-only.
pub fn architecture_insights(graph: &MemoryGraph) -> Vec<ArchInsight> {
    let mut insights = Vec::new();

    // ---- High-centrality concepts (existing importance metric) ----
    for (id, importance) in graph.importance_ranking(MAX_PER_KIND) {
        if importance <= 0.0 {
            continue;
        }
        insights.push(ArchInsight {
            kind: InsightKind::HighCentrality,
            subject: super::reasoning::concept_label(graph, &id),
            subject_id: id,
            detail: format!(
                "importance {importance:.3}; changes here propagate widely — review impact before editing"
            ),
            score: importance,
        });
    }

    // ---- Coupling hotspots: DependsOn degree (in + out) ----
    let mut coupling: Vec<(String, usize, usize)> = Vec::new();
    for m in graph.active_memories() {
        let out = graph
            .get_edges(&m.id)
            .iter()
            .filter(|e| e.kind == EdgeKind::DependsOn)
            .count();
        let inbound = graph
            .get_incoming(&m.id)
            .iter()
            .filter(|s| {
                graph
                    .get_edges(s)
                    .iter()
                    .any(|e| e.kind == EdgeKind::DependsOn && e.target == m.id)
            })
            .count();
        if out + inbound >= 4 {
            coupling.push((m.id.clone(), out, inbound));
        }
    }
    coupling.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)).then_with(|| a.0.cmp(&b.0)));
    for (id, out, inbound) in coupling.into_iter().take(MAX_PER_KIND) {
        insights.push(ArchInsight {
            kind: InsightKind::CouplingHotspot,
            subject: super::reasoning::concept_label(graph, &id),
            subject_id: id,
            detail: format!(
                "{out} outgoing / {inbound} incoming dependency relations; growing coupling"
            ),
            score: (out + inbound) as f32,
        });
    }

    // ---- Community strength ----
    let mut clusters: Vec<(&String, u32)> = graph
        .clusters
        .iter()
        .map(|(id, c)| (id, c.member_count))
        .collect();
    clusters.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    for (id, members) in &clusters {
        if *members >= 8 {
            insights.push(ArchInsight {
                kind: InsightKind::StrongCommunity,
                subject_id: (*id).clone(),
                subject: id.trim_start_matches("cluster:").to_string(),
                detail: format!("{members} members; behaves as a real subsystem boundary"),
                score: *members as f32,
            });
        } else if *members >= 1 && *members < 3 {
            insights.push(ArchInsight {
                kind: InsightKind::WeakCommunity,
                subject_id: (*id).clone(),
                subject: id.trim_start_matches("cluster:").to_string(),
                detail: format!("only {members} member(s); fragmented or fading abstraction"),
                score: 3.0 - *members as f32,
            });
        }
    }

    // ---- Dead concepts: active, isolated, never used ----
    let mut dead = 0usize;
    for m in graph.active_memories() {
        if dead >= MAX_PER_KIND {
            break;
        }
        let has_semantic_edges = graph
            .get_edges(&m.id)
            .iter()
            .any(|e| e.kind.is_semantic() && graph.get_memory(&e.target).is_some())
            || graph
                .get_incoming(&m.id)
                .iter()
                .any(|s| graph.get_memory(s).is_some());
        if !has_semantic_edges && m.access_count == 0 && m.strength <= 1 {
            insights.push(ArchInsight {
                kind: InsightKind::DeadConcept,
                subject: super::reasoning::concept_label(graph, &m.id),
                subject_id: m.id.clone(),
                detail: "no relations, never retrieved, never reinforced".to_string(),
                score: 1.0,
            });
            dead += 1;
        }
    }

    // ---- Duplicate abstractions: identical normalized content prefixes ----
    let mut by_prefix: HashMap<String, Vec<&str>> = HashMap::new();
    for m in graph.active_memories() {
        let prefix: String = m
            .searchable_text()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(80)
            .collect();
        if prefix.len() >= 40 {
            by_prefix.entry(prefix).or_default().push(m.id.as_str());
        }
    }
    let mut dup_groups: Vec<Vec<&str>> = by_prefix
        .into_values()
        .filter(|ids| ids.len() > 1)
        .collect();
    for ids in &mut dup_groups {
        ids.sort();
    }
    dup_groups.sort();
    for ids in dup_groups.into_iter().take(MAX_PER_KIND) {
        insights.push(ArchInsight {
            kind: InsightKind::DuplicateAbstraction,
            subject: super::reasoning::concept_label(graph, ids[0]),
            subject_id: ids[0].to_string(),
            detail: format!(
                "{} concepts share near-identical content: {}",
                ids.len(),
                ids.join(", ")
            ),
            score: ids.len() as f32,
        });
    }

    // ---- Documentation drift: doc Supports module, module moved on ----
    const DRIFT_DAYS: i64 = 30;
    let mut drift = 0usize;
    for m in graph.active_memories() {
        if drift >= MAX_PER_KIND {
            break;
        }
        if !m.tags.iter().any(|t| t == "docs") {
            continue;
        }
        for edge in graph.get_edges(&m.id) {
            if edge.kind != EdgeKind::Supports {
                continue;
            }
            if let Some(module) = graph.get_memory(&edge.target) {
                let lag = (module.updated_at - m.updated_at).num_days();
                if lag >= DRIFT_DAYS {
                    insights.push(ArchInsight {
                        kind: InsightKind::DocDrift,
                        subject: super::reasoning::concept_label(graph, &m.id),
                        subject_id: m.id.clone(),
                        detail: format!(
                            "supports \"{}\" but is {lag} days behind its last change",
                            super::reasoning::concept_label(graph, &edge.target)
                        ),
                        score: lag as f32,
                    });
                    drift += 1;
                    break;
                }
            }
        }
    }

    insights
}

pub fn render_insights(insights: &[ArchInsight]) -> String {
    if insights.is_empty() {
        return "No architectural observations (graph too small or perfectly boring).".to_string();
    }
    let mut grouped: BTreeMap<&'static str, Vec<&ArchInsight>> = BTreeMap::new();
    for i in insights {
        grouped.entry(i.kind.label()).or_default().push(i);
    }
    let mut out = String::from("Architectural observations (read-only; recorded as evidence when asked):\n");
    for (kind, list) in grouped {
        out.push_str(&format!("\n{kind}:\n"));
        for i in list {
            out.push_str(&format!("  [{:.2}] {} — {}\n", i.score, i.subject, i.detail));
        }
    }
    out
}

// ==================== Continuous architecture health ====================

/// Explainable project-health metrics, computed deterministically from the
/// graph. Read-only; renders as a report and can be recorded as evidence.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthReport {
    pub concepts_total: usize,
    pub concepts_active: usize,
    pub semantic_edges: usize,
    /// Mean DependsOn degree over module concepts (coupling).
    pub avg_coupling: f32,
    pub communities: usize,
    pub duplicate_groups: usize,
    /// Modules with at least one doc `Supports` edge / all modules.
    pub doc_coverage: f32,
    /// Concepts upgraded by sidecar abstraction / concepts wanting it.
    pub knowledge_coverage: f32,
    /// Confidence distribution over active concepts.
    pub confidence_low: usize,
    pub confidence_mid: usize,
    pub confidence_high: usize,
    /// Historical prediction precision (EWMA), if any reflections occurred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prediction_precision: Option<f32>,
    /// Item pairs that repeatedly change together across ingest passes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub co_change_pairs: Vec<(String, String, usize)>,
}

pub fn health_report(graph: &MemoryGraph) -> HealthReport {
    let mut report = HealthReport {
        concepts_total: graph.memory_count(),
        communities: graph.clusters.len(),
        ..Default::default()
    };

    let mut modules = 0usize;
    let mut modules_with_docs = 0usize;
    let mut coupling_sum = 0usize;
    let mut wants_abstraction = 0usize;
    let mut abstracted = 0usize;

    for m in graph.active_memories() {
        report.concepts_active += 1;
        match m.confidence {
            c if c < 0.4 => report.confidence_low += 1,
            c if c < 0.7 => report.confidence_mid += 1,
            _ => report.confidence_high += 1,
        }
        if m.id.starts_with("mem-src-") && m.tags.iter().any(|t| t.starts_with("pkg-")) {
            modules += 1;
            coupling_sum += graph
                .get_edges(&m.id)
                .iter()
                .filter(|e| e.kind == EdgeKind::DependsOn)
                .count();
            let has_doc_support = graph.get_incoming(&m.id).iter().any(|s| {
                graph
                    .get_memory(s)
                    .map(|d| d.tags.iter().any(|t| t == "docs"))
                    .unwrap_or(false)
                    && graph
                        .get_edges(s)
                        .iter()
                        .any(|e| e.kind == EdgeKind::Supports && e.target == m.id)
            });
            if has_doc_support {
                modules_with_docs += 1;
            }
            wants_abstraction += 1;
            if m.tags.iter().any(|t| t == super::TAG_ABSTRACTED) {
                abstracted += 1;
            }
        }
    }
    report.semantic_edges = graph
        .edges
        .values()
        .flatten()
        .filter(|e| e.kind.is_semantic())
        .count();
    report.avg_coupling = if modules > 0 {
        coupling_sum as f32 / modules as f32
    } else {
        0.0
    };
    report.doc_coverage = if modules > 0 {
        modules_with_docs as f32 / modules as f32
    } else {
        0.0
    };
    report.knowledge_coverage = if wants_abstraction > 0 {
        abstracted as f32 / wants_abstraction as f32
    } else {
        0.0
    };
    report.duplicate_groups = architecture_insights(graph)
        .iter()
        .filter(|i| i.kind == InsightKind::DuplicateAbstraction)
        .count();
    let stats = &graph.metadata.prediction_stats;
    if stats.reflections > 0 {
        report.prediction_precision = Some(stats.precision_ewma);
    }

    // Co-change: item pairs appearing together in the changed sample of the
    // same evolution point, across recent history, in ≥2 passes.
    let mut pair_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for state in graph.metadata.knowledge_sources.values() {
        for point in &state.history {
            let sample = &point.changed_sample;
            for i in 0..sample.len() {
                for j in (i + 1)..sample.len() {
                    let (a, b) = if sample[i] <= sample[j] {
                        (sample[i].clone(), sample[j].clone())
                    } else {
                        (sample[j].clone(), sample[i].clone())
                    };
                    *pair_counts.entry((a, b)).or_default() += 1;
                }
            }
        }
    }
    let mut pairs: Vec<(String, String, usize)> = pair_counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .map(|((a, b), n)| (a, b, n))
        .collect();
    pairs.sort_by(|x, y| y.2.cmp(&x.2).then_with(|| x.0.cmp(&y.0)));
    pairs.truncate(8);
    report.co_change_pairs = pairs;

    report
}

pub fn render_health(report: &HealthReport) -> String {
    let mut out = String::from("Architecture health (deterministic, graph-derived):\n");
    out.push_str(&format!(
        "  concepts: {} active / {} total · semantic edges: {} · communities: {}\n",
        report.concepts_active, report.concepts_total, report.semantic_edges, report.communities
    ));
    out.push_str(&format!(
        "  coupling (avg DependsOn per module): {:.2} · duplicate groups: {}\n",
        report.avg_coupling, report.duplicate_groups
    ));
    out.push_str(&format!(
        "  doc coverage: {:.0}% · abstraction coverage: {:.0}%\n",
        report.doc_coverage * 100.0,
        report.knowledge_coverage * 100.0
    ));
    out.push_str(&format!(
        "  confidence: {} low / {} mid / {} high\n",
        report.confidence_low, report.confidence_mid, report.confidence_high
    ));
    if let Some(p) = report.prediction_precision {
        out.push_str(&format!("  prediction precision (EWMA): {p:.2}\n"));
    }
    if !report.co_change_pairs.is_empty() {
        out.push_str("  frequently change together:\n");
        for (a, b, n) in &report.co_change_pairs {
            out.push_str(&format!("    {a} ↔ {b} ({n} passes)\n"));
        }
    }
    out
}

/// Append the current observations to the evidence ledger as one block.
/// Observations never modify code or the graph; they become history.
pub fn record_insights_as_evidence(insights: &[ArchInsight]) -> anyhow::Result<usize> {
    if insights.is_empty() {
        return Ok(0);
    }
    crate::evidence_ledger::append_evidence(
        crate::evidence_ledger::EvidenceKind::ArchitecturalInsight,
        "knowledge.architecture_insights",
        format!("{} architectural observation(s)", insights.len()),
        None,
        None,
        &insights,
    )?;
    crate::memory_log::log_knowledge(
        "knowledge_insights",
        serde_json::json!({ "count": insights.len() }),
    );
    Ok(insights.len())
}
