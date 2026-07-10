//! Architecture-aware reasoning over the semantic memory graph.
//!
//! The graph is the reasoning substrate: seeds are found by deterministic
//! keyword scoring (embeddings assist discovery when the local model is
//! available), conclusions come from typed-edge traversal — cascade
//! expansion, ranked relations, reverse-dependency impact closure — and every
//! conclusion carries the evidence and confidence already stored on the
//! nodes and edges. Nothing here calls a hosted model; the sidecar/LLM layer
//! stays where it already lives (abstraction, consolidation review).
//!
//! Three consumers share this module:
//!   * `neura knowledge reason / impact` — explicit CLI reasoning with a
//!     rendered trace ("why did Neura conclude this").
//!   * The per-turn architectural prior ([`turn_brief`]) — a compact,
//!     deterministic system-reminder derived from the project graph, mirroring
//!     the existing cognition-trigger prior pattern (soft, fail-quiet, no
//!     control-flow changes).
//!   * Predictive reasoning: each turn brief records which concepts are
//!     expected to be touched; when tool outcomes are folded back
//!     ([`reflect_on_outcomes`]), predictions are compared against reality,
//!     correct expectations reinforce the graph, and the comparison is
//!     appended to the evidence ledger — explicit history, never hidden state.

use crate::memory_graph::{EdgeKind, EvidenceRef, MemoryGraph};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Mutex;

/// Cap on rendered/analysed items so traces stay prompt-sized.
const MAX_SEEDS: usize = 5;
const MAX_EXPANDED: usize = 12;
const MAX_IMPACT: usize = 16;
/// Turn briefs are injected into a live turn; keep them compact.
const MAX_BRIEF_CHARS: usize = 1_100;
/// Pending per-session predictions kept for later reflection.
const MAX_PENDING_PREDICTIONS: usize = 32;

// ==================== Reasoning traces ====================

/// How a concept entered the trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedVia {
    Keyword,
    Embedding,
    Graph,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptHit {
    pub id: String,
    pub label: String,
    pub score: f32,
    pub via: SeedVia,
}

/// A deterministic, explainable reasoning pass over the graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningTrace {
    pub query: String,
    /// Direct matches (keyword / embedding).
    pub seeds: Vec<ConceptHit>,
    /// Concepts reached by typed-edge cascade from the seeds.
    pub expanded: Vec<ConceptHit>,
    /// Human-readable relation lines for the top concepts.
    pub relations: Vec<String>,
    /// Evidence notes supporting the top concepts (bounded).
    pub evidence: Vec<String>,
    /// Communities the top concepts belong to.
    pub communities: Vec<String>,
    /// Aggregate confidence: seed strength × stored concept confidence.
    pub confidence: f32,
}

/// Short display label for a memory: first line of content, truncated.
pub fn concept_label(graph: &MemoryGraph, id: &str) -> String {
    graph
        .get_memory(id)
        .map(|m| {
            let first = m.content.lines().next().unwrap_or("");
            let label: String = first.chars().take(72).collect();
            label
        })
        .unwrap_or_else(|| id.to_string())
}

/// Function words that carry no architectural signal; keeping them would
/// dilute keyword overlap scores on natural-language queries.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "should", "would", "could",
    "can", "will", "its", "are", "was", "were", "has", "have", "had", "not", "but", "all",
    "any", "out", "our", "you", "your", "they", "them", "when", "where", "what", "how", "why",
    "which", "then", "than", "over", "about", "after", "before", "does", "did", "been",
    "being", "also", "just", "some", "more", "most", "other", "such", "only", "very", "via",
    "please", "need", "want", "let", "make", "there", "here",
];

fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens: Vec<String> = query
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Find seed concepts for a query: deterministic keyword overlap against the
/// pre-normalized search text, with embedding cosine similarity folded in
/// when the local model is available (embeddings assist discovery; they never
/// override an exact keyword hit).
pub fn find_seeds(graph: &MemoryGraph, query: &str, limit: usize) -> Vec<ConceptHit> {
    let tokens = query_tokens(query);
    if tokens.is_empty() {
        return Vec::new();
    }
    let query_embedding = if crate::embedding::is_model_available() {
        crate::embedding::embed(query).ok()
    } else {
        None
    };

    let mut hits: Vec<ConceptHit> = Vec::new();
    for m in graph.active_memories() {
        let text = m.searchable_text();
        let matched = tokens.iter().filter(|t| text.contains(t.as_str())).count();
        let keyword_score = matched as f32 / tokens.len() as f32;

        let embed_score = query_embedding
            .as_ref()
            .and_then(|q| {
                m.concept_embedding
                    .as_ref()
                    .or(m.embedding.as_ref())
                    .map(|e| cosine(q, e))
            })
            .unwrap_or(0.0);

        let (score, via) = if keyword_score >= embed_score {
            (keyword_score, SeedVia::Keyword)
        } else {
            (embed_score, SeedVia::Embedding)
        };
        if score >= 0.34 {
            hits.push(ConceptHit {
                id: m.id.clone(),
                label: String::new(), // filled below (m is borrowed here)
                score,
                via,
            });
        }
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    for hit in &mut hits {
        hit.label = concept_label(graph, &hit.id);
    }
    hits
}

/// Match concepts against a *long* text (goal descriptions, decisions) by
/// absolute token overlap instead of query-fraction scoring — a 40-word
/// decision should still link to the module it names. Requires ≥2 matched
/// tokens; score saturates with overlap.
pub fn match_concepts_by_overlap(graph: &MemoryGraph, text: &str, limit: usize) -> Vec<ConceptHit> {
    let tokens = query_tokens(text);
    if tokens.len() < 2 {
        return Vec::new();
    }
    let mut hits: Vec<ConceptHit> = Vec::new();
    for m in graph.active_memories() {
        let content = m.searchable_text();
        let matched = tokens.iter().filter(|t| content.contains(t.as_str())).count();
        if matched >= 2 {
            hits.push(ConceptHit {
                id: m.id.clone(),
                label: String::new(),
                score: matched as f32 / (matched as f32 + 2.0),
                via: SeedVia::Keyword,
            });
        }
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    for hit in &mut hits {
        hit.label = concept_label(graph, &hit.id);
    }
    hits
}

/// Reason about `query` over the graph: find seeds, expand through typed
/// edges, and collect the relations, communities, and evidence that justify
/// the result. `&mut` only because cascade retrieval counts retrievals.
pub fn reason(graph: &mut MemoryGraph, query: &str) -> ReasoningTrace {
    let seeds = find_seeds(graph, query, MAX_SEEDS);
    let seed_ids: Vec<String> = seeds.iter().map(|s| s.id.clone()).collect();
    let seed_scores: Vec<f32> = seeds.iter().map(|s| s.score).collect();

    let expanded_raw = graph.cascade_retrieve(&seed_ids, &seed_scores, 2, MAX_EXPANDED + MAX_SEEDS);
    let seed_set: BTreeSet<&String> = seed_ids.iter().collect();
    let expanded: Vec<ConceptHit> = expanded_raw
        .into_iter()
        .filter(|(id, _)| !seed_set.contains(id))
        .take(MAX_EXPANDED)
        .map(|(id, score)| ConceptHit {
            label: concept_label(graph, &id),
            id,
            score,
            via: SeedVia::Graph,
        })
        .collect();

    let mut relations = Vec::new();
    let mut evidence = Vec::new();
    let mut communities = BTreeSet::new();
    let mut confidence = 0.0f32;

    for seed in seeds.iter().take(3) {
        for (kind, other, weight, edge_conf) in graph.ranked_relations(&seed.id).into_iter().take(4)
        {
            relations.push(format!(
                "{} —{}→ {} (w {:.2}, conf {:.2})",
                seed.label,
                kind.label(),
                concept_label(graph, &other),
                weight,
                edge_conf,
            ));
        }
        if let Some(m) = graph.get_memory(&seed.id) {
            confidence = confidence.max(seed.score * m.confidence);
            for ev in m.evidence.iter().rev().take(2) {
                let what = ev.note.clone().unwrap_or_else(|| ev.id.clone());
                if !what.is_empty() {
                    evidence.push(format!("{}: {}", seed.label, what));
                }
            }
        }
        for edge in graph.get_edges(&seed.id) {
            if edge.kind == EdgeKind::InCluster
                && let Some(cluster) = graph.clusters.get(&edge.target)
            {
                communities.insert(format!(
                    "{} ({} members)",
                    edge.target.trim_start_matches("cluster:"),
                    cluster.member_count
                ));
            }
        }
    }

    ReasoningTrace {
        query: query.to_string(),
        seeds,
        expanded,
        relations,
        evidence,
        communities: communities.into_iter().collect(),
        confidence,
    }
}

pub fn render_trace(trace: &ReasoningTrace) -> String {
    let mut out = format!(
        "Reasoning over the knowledge graph for: {}\nconfidence {:.2}\n",
        trace.query, trace.confidence
    );
    if trace.seeds.is_empty() {
        out.push_str("No concepts matched.\n");
        return out;
    }
    out.push_str("\nConcepts (direct matches):\n");
    for s in &trace.seeds {
        out.push_str(&format!("  [{:.2} {:?}] {} ({})\n", s.score, s.via, s.label, s.id));
    }
    if !trace.expanded.is_empty() {
        out.push_str("\nGraph expansion:\n");
        for e in &trace.expanded {
            out.push_str(&format!("  [{:.2}] {}\n", e.score, e.label));
        }
    }
    if !trace.relations.is_empty() {
        out.push_str("\nTyped relations:\n");
        for r in &trace.relations {
            out.push_str(&format!("  {r}\n"));
        }
    }
    if !trace.communities.is_empty() {
        out.push_str("\nCommunities:\n");
        for c in &trace.communities {
            out.push_str(&format!("  {c}\n"));
        }
    }
    if !trace.evidence.is_empty() {
        out.push_str("\nSupporting evidence:\n");
        for e in &trace.evidence {
            out.push_str(&format!("  {e}\n"));
        }
    }
    out
}

// ==================== Impact analysis ====================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactEntry {
    pub id: String,
    pub label: String,
    /// The relation that propagates the impact (edge label).
    pub via: String,
    /// Hops from the target.
    pub distance: usize,
    /// Attenuated propagation confidence.
    pub confidence: f32,
}

/// Explicit model of what a change to the target concepts would touch,
/// derived purely from typed edges and their stored confidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactModel {
    pub targets: Vec<ConceptHit>,
    /// Concepts that depend (transitively) on the targets, containers the
    /// targets belong to, and semantic neighbours — ordered by confidence.
    pub affected: Vec<ImpactEntry>,
    /// Test concepts that `Supports` the targets or affected concepts.
    pub likely_tests: Vec<String>,
    /// 1 − mean edge confidence along the closure: how much of the impact
    /// model rests on weakly-evidenced edges.
    pub uncertainty: f32,
}

/// Compute the impact closure of the target concepts:
///   * reverse `DependsOn` (dependents break when the target changes),
///   * forward `PartOf` (containers whose aggregate description shifts),
///   * incoming `Supports` from test-tagged concepts (likely failing tests).
/// Attenuates confidence per hop with the edge's stored weight × confidence.
pub fn impact_for(graph: &MemoryGraph, target_ids: &[String], max_depth: usize) -> ImpactModel {
    let targets: Vec<ConceptHit> = target_ids
        .iter()
        .filter(|id| graph.get_memory(id).is_some())
        .map(|id| ConceptHit {
            label: concept_label(graph, id),
            id: id.clone(),
            score: 1.0,
            via: SeedVia::Graph,
        })
        .collect();

    let mut affected: HashMap<String, ImpactEntry> = HashMap::new();
    let mut likely_tests: BTreeSet<String> = BTreeSet::new();
    let mut edge_confs: Vec<f32> = Vec::new();
    let mut queue: VecDeque<(String, usize, f32)> = VecDeque::new();
    for t in &targets {
        queue.push_back((t.id.clone(), 0, 1.0));
    }
    let target_set: BTreeSet<&String> = target_ids.iter().collect();

    while let Some((node, depth, conf)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        // Dependents: sources with a DependsOn edge into `node`.
        for source in graph.get_incoming(&node) {
            let source = source.to_string();
            for edge in graph.get_edges(&source) {
                if edge.target != node {
                    continue;
                }
                let (is_test_support, relevant) = match edge.kind {
                    EdgeKind::DependsOn => (false, true),
                    EdgeKind::Supports => {
                        let is_test = graph
                            .get_memory(&source)
                            .map(|m| m.tags.iter().any(|t| t == "tests"))
                            .unwrap_or(false);
                        (is_test, is_test)
                    }
                    _ => (false, false),
                };
                if !relevant {
                    continue;
                }
                let step = (edge.meta.weight * edge.meta.confidence.max(0.3)).clamp(0.05, 1.0);
                let next_conf = conf * step;
                edge_confs.push(edge.meta.confidence);
                if is_test_support {
                    likely_tests.insert(concept_label(graph, &source));
                    continue;
                }
                if target_set.contains(&source) {
                    continue;
                }
                let entry = affected.entry(source.clone()).or_insert_with(|| ImpactEntry {
                    id: source.clone(),
                    label: concept_label(graph, &source),
                    via: edge.kind.label().to_string(),
                    distance: depth + 1,
                    confidence: next_conf,
                });
                if next_conf > entry.confidence {
                    entry.confidence = next_conf;
                    entry.distance = depth + 1;
                }
                queue.push_back((source.clone(), depth + 1, next_conf));
            }
        }
        // Containers: forward PartOf edges.
        for edge in graph.get_edges(&node) {
            if edge.kind != EdgeKind::PartOf || target_set.contains(&edge.target) {
                continue;
            }
            if graph.get_memory(&edge.target).is_none() {
                continue;
            }
            let next_conf = conf * 0.5;
            edge_confs.push(edge.meta.confidence);
            let entry = affected
                .entry(edge.target.clone())
                .or_insert_with(|| ImpactEntry {
                    id: edge.target.clone(),
                    label: concept_label(graph, &edge.target),
                    via: "part_of".to_string(),
                    distance: depth + 1,
                    confidence: next_conf,
                });
            if next_conf > entry.confidence {
                entry.confidence = next_conf;
            }
        }
    }

    let mut affected: Vec<ImpactEntry> = affected.into_values().collect();
    affected.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.id.cmp(&b.id))
    });
    affected.truncate(MAX_IMPACT);

    let uncertainty = if edge_confs.is_empty() {
        1.0
    } else {
        1.0 - (edge_confs.iter().sum::<f32>() / edge_confs.len() as f32)
    };

    ImpactModel {
        targets,
        affected,
        likely_tests: likely_tests.into_iter().collect(),
        uncertainty,
    }
}

pub fn render_impact(model: &ImpactModel) -> String {
    let mut out = String::from("Impact model (typed-edge closure):\n");
    if model.targets.is_empty() {
        out.push_str("No matching target concepts.\n");
        return out;
    }
    out.push_str("Targets:\n");
    for t in &model.targets {
        out.push_str(&format!("  {}\n", t.label));
    }
    if model.affected.is_empty() {
        out.push_str("No dependent or containing concepts found.\n");
    } else {
        out.push_str("Affected (dependents / containers):\n");
        for a in &model.affected {
            out.push_str(&format!(
                "  [{:.2} via {} d{}] {}\n",
                a.confidence, a.via, a.distance, a.label
            ));
        }
    }
    if !model.likely_tests.is_empty() {
        out.push_str("Likely affected tests:\n");
        for t in &model.likely_tests {
            out.push_str(&format!("  {t}\n"));
        }
    }
    out.push_str(&format!("Uncertainty: {:.2}\n", model.uncertainty));
    out
}

// ==================== Predictive reasoning ====================

/// One turn's architectural expectation, recorded before acting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnPrediction {
    pub session_id: String,
    /// Concept memory ids expected to be touched by the coming work.
    pub predicted_concepts: Vec<String>,
    pub query_preview: String,
    pub at: DateTime<Utc>,
}

static PENDING_PREDICTIONS: Mutex<Vec<TurnPrediction>> = Mutex::new(Vec::new());

pub fn record_prediction(prediction: TurnPrediction) {
    crate::memory_log::log_knowledge(
        "knowledge_prediction",
        serde_json::json!({
            "session_id": prediction.session_id,
            "predicted_concepts": prediction.predicted_concepts,
            "query_preview": prediction.query_preview,
        }),
    );
    if let Ok(mut pending) = PENDING_PREDICTIONS.lock() {
        pending.push(prediction);
        if pending.len() > MAX_PENDING_PREDICTIONS {
            let overflow = pending.len() - MAX_PENDING_PREDICTIONS;
            pending.drain(0..overflow);
        }
    }
}

pub fn pending_prediction_count() -> usize {
    PENDING_PREDICTIONS.lock().map(|p| p.len()).unwrap_or(0)
}

/// Rolling calibration of Neura's architectural predictions, persisted in
/// [`GraphMetadata`] so planning and briefs can state how reliable past
/// expectations have been. Updated only at reflection time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredictionStats {
    #[serde(default)]
    pub reflections: u64,
    #[serde(default)]
    pub predicted_total: u64,
    #[serde(default)]
    pub confirmed_total: u64,
    /// Exponentially-weighted precision (α = 0.3): recent engineering
    /// outcomes matter more than ancient ones.
    #[serde(default)]
    pub precision_ewma: f32,
}

impl Default for PredictionStats {
    fn default() -> Self {
        Self {
            reflections: 0,
            predicted_total: 0,
            confirmed_total: 0,
            precision_ewma: 0.0,
        }
    }
}

impl PredictionStats {
    pub fn record(&mut self, stats: &ReflectionStats) {
        self.reflections += 1;
        self.predicted_total += stats.predicted_concepts as u64;
        self.confirmed_total += stats.confirmed as u64;
        const ALPHA: f32 = 0.3;
        self.precision_ewma = if self.reflections == 1 {
            stats.precision
        } else {
            ALPHA * stats.precision + (1.0 - ALPHA) * self.precision_ewma
        };
    }
}

/// Pure comparison of a prediction against the concepts actually touched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReflectionStats {
    pub predictions: usize,
    pub predicted_concepts: usize,
    pub confirmed: usize,
    pub missed: usize,
    /// Touched concepts nobody predicted.
    pub unexpected: usize,
    /// confirmed / predicted (0 when nothing was predicted).
    pub precision: f32,
}

pub fn compare_predictions(
    predictions: &[TurnPrediction],
    touched_concepts: &BTreeSet<String>,
) -> ReflectionStats {
    let mut predicted: BTreeSet<&String> = BTreeSet::new();
    for p in predictions {
        predicted.extend(p.predicted_concepts.iter());
    }
    let confirmed = predicted.iter().filter(|c| touched_concepts.contains(**c)).count();
    let missed = predicted.len() - confirmed;
    let unexpected = touched_concepts
        .iter()
        .filter(|c| !predicted.contains(*c))
        .count();
    ReflectionStats {
        predictions: predictions.len(),
        predicted_concepts: predicted.len(),
        confirmed,
        missed,
        unexpected,
        precision: if predicted.is_empty() {
            0.0
        } else {
            confirmed as f32 / predicted.len() as f32
        },
    }
}

/// Structured reflection after execution: compare pending predictions with
/// the concepts actually touched by tool outcomes, reinforce confirmed
/// expectations on the graph, and append the comparison to the evidence
/// ledger. History is only ever appended, never rewritten.
pub fn reflect_on_outcomes(
    graph: &mut MemoryGraph,
    touched_concepts: &BTreeSet<String>,
) -> Option<ReflectionStats> {
    let predictions: Vec<TurnPrediction> = match PENDING_PREDICTIONS.lock() {
        Ok(mut pending) => {
            if pending.is_empty() {
                return None;
            }
            pending.drain(..).collect()
        }
        Err(_) => return None,
    };
    if touched_concepts.is_empty() {
        // Nothing executed against the architecture; drop stale predictions
        // but keep the fact observable.
        crate::memory_log::log_knowledge(
            "knowledge_reflection",
            serde_json::json!({ "predictions": predictions.len(), "touched": 0 }),
        );
        return None;
    }

    let stats = compare_predictions(&predictions, touched_concepts);

    // Adaptive planning: fold this reflection into the rolling calibration
    // that briefs and plan decomposition report.
    graph.metadata.prediction_stats.record(&stats);

    // Correct architectural expectations are themselves evidence: reinforce
    // the confirmed concepts through the existing observation machinery.
    for p in &predictions {
        for id in &p.predicted_concepts {
            if touched_concepts.contains(id) {
                graph.record_fact_observation(
                    id,
                    EvidenceRef::observation("architectural prediction confirmed by execution"),
                );
            }
        }
    }

    crate::memory_log::log_knowledge(
        "knowledge_reflection",
        serde_json::json!({
            "predictions": stats.predictions,
            "predicted_concepts": stats.predicted_concepts,
            "confirmed": stats.confirmed,
            "missed": stats.missed,
            "unexpected": stats.unexpected,
            "precision": stats.precision,
        }),
    );
    // Unit tests exercise reflection logic without touching the user-level
    // ledger chain; everything else appends real history.
    if !cfg!(test) {
        let _ = crate::evidence_ledger::append_evidence(
            crate::evidence_ledger::EvidenceKind::Reflection,
            "knowledge.prediction_reflection",
            format!(
                "compared {} prediction(s): {}/{} confirmed, {} unexpected",
                stats.predictions, stats.confirmed, stats.predicted_concepts, stats.unexpected
            ),
            Some(stats.precision as f64),
            Some(stats.confirmed > 0),
            &stats,
        );
    }

    Some(stats)
}

// ==================== Per-turn architectural prior ====================

/// Deterministic architectural context for a turn, mirroring the
/// cognition-trigger prior pattern: soft, bounded, fail-quiet, and clearly
/// labelled. Returns `None` instantly when the project has no knowledge
/// sources or the input matches no concepts. Also records the expectation as
/// a prediction so post-execution reflection can score it.
pub fn turn_brief_for_graph(graph: &mut MemoryGraph, user_text: &str) -> Option<String> {
    if graph.metadata.knowledge_sources.is_empty() {
        return None;
    }
    let trace = reason(graph, user_text);
    // Gate: a clear module mention scores ≈ 0.4 keyword × ~0.7 concept
    // confidence, so 0.2 admits real matches while rejecting noise.
    if trace.seeds.is_empty() || trace.confidence < 0.2 {
        return None;
    }
    let seed_ids: Vec<String> = trace.seeds.iter().map(|s| s.id.clone()).collect();
    let impact = impact_for(graph, &seed_ids, 2);

    // Record the expectation: seeds + high-confidence affected concepts.
    let mut predicted: Vec<String> = seed_ids.clone();
    predicted.extend(
        impact
            .affected
            .iter()
            .filter(|a| a.confidence >= 0.3)
            .map(|a| a.id.clone()),
    );
    predicted.dedup();
    record_prediction(TurnPrediction {
        session_id: crate::get_current_session().unwrap_or_default(),
        predicted_concepts: predicted,
        query_preview: user_text.chars().take(80).collect(),
        at: Utc::now(),
    });

    let mut brief = String::from(
        "[architectural context — derived deterministically from the project knowledge graph]\n",
    );
    for s in trace.seeds.iter().take(3) {
        brief.push_str(&format!("• {}\n", s.label));
    }
    let relations: Vec<&String> = trace.relations.iter().take(4).collect();
    if !relations.is_empty() {
        brief.push_str("Relations: ");
        brief.push_str(
            &relations
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        );
        brief.push('\n');
    }
    if !impact.affected.is_empty() {
        let names: Vec<String> = impact
            .affected
            .iter()
            .take(4)
            .map(|a| format!("{} (via {})", a.label, a.via))
            .collect();
        brief.push_str(&format!("Likely impact if changed: {}\n", names.join("; ")));
    }
    if !impact.likely_tests.is_empty() {
        brief.push_str(&format!(
            "Tests covering this area: {}\n",
            impact.likely_tests.join("; ")
        ));
    }
    // Prior decisions are reused before inventing new ones.
    let decisions = super::engineering::relevant_decisions(graph, user_text, 2);
    if !decisions.is_empty() {
        brief.push_str("Prior decisions on record: ");
        brief.push_str(
            &decisions
                .iter()
                .map(|(_, label)| label.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        );
        brief.push('\n');
    }
    let calibration = &graph.metadata.prediction_stats;
    let calibration_note = if calibration.reflections > 0 {
        format!(
            ", historical prediction precision {:.2} over {} reflection(s)",
            calibration.precision_ewma, calibration.reflections
        )
    } else {
        String::new()
    };
    brief.push_str(&format!(
        "(confidence {:.2}, impact uncertainty {:.2}{calibration_note}; treat as a prior, verify in source)",
        trace.confidence, impact.uncertainty
    ));

    if brief.len() > MAX_BRIEF_CHARS {
        brief.truncate(MAX_BRIEF_CHARS);
    }
    Some(brief)
}

/// Async entry point used by the agent turn path. Loads the (mtime-cached)
/// project graph off the async runtime and never fails the turn.
pub async fn turn_brief(project_dir: Option<std::path::PathBuf>, user_text: &str) -> Option<String> {
    if user_text.trim().len() < 8 {
        return None;
    }
    let text = user_text.to_string();
    tokio::task::spawn_blocking(move || {
        let manager = match project_dir {
            Some(dir) => crate::memory::MemoryManager::new().with_project_dir(dir),
            None => crate::memory::MemoryManager::new(),
        };
        let mut graph = manager.load_project_graph().ok()?;
        let brief = turn_brief_for_graph(&mut graph, &text);
        if brief.is_some() {
            crate::memory_log::log_knowledge(
                "knowledge_turn_brief",
                serde_json::json!({ "chars": brief.as_ref().map(|b| b.len()).unwrap_or(0) }),
            );
        }
        brief
    })
    .await
    .ok()
    .flatten()
}

/// Map source-relative item keys touched by tool outcomes to their concept
/// ids across all registered sources (helper shared with evidence folding).
pub fn touched_concepts_for_items(graph: &MemoryGraph, items: &[String]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for item in items {
        for id in super::concept_ids_for_item(graph, item) {
            map.insert(id, item.clone());
        }
    }
    map
}
