//! Adaptive cognition substrate for persistent `.kcode` memory evolution.
//!
//! This module stores memory as versioned, scored graph nodes rather than flat
//! prompt snippets. It is intentionally deterministic and local-first so it can
//! run on every turn without provider calls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const STORE_VERSION: u32 = 1;
const MAX_DECISIONS: usize = 512;
const DEFAULT_HALF_LIFE_DAYS: f64 = 90.0;
const MIN_DECAY: f64 = 0.05;
const MAX_CONTEXT_TOKENS_DEFAULT: usize = 2_400;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum CognitiveScope {
    Turn,
    Session,
    Project,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CognitiveNodeKind {
    Directive,
    Preference,
    Fact,
    Procedure,
    Outcome,
    Reflection,
    CompressionSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CognitiveEdgeKind {
    Supports,
    Contradicts,
    Refines,
    DerivedFrom,
    Causes,
    UsedBy,
    SameTopic,
    Compresses,
    HasOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveWeights {
    #[serde(default = "default_one")]
    pub reinforcement: f64,
    #[serde(default = "default_half_life")]
    pub half_life_days: f64,
    #[serde(default)]
    pub contradiction: f64,
    #[serde(default)]
    pub graph: f64,
    #[serde(default)]
    pub outcome: f64,
    #[serde(default = "default_one")]
    pub confidence: f64,
}

impl Default for CognitiveWeights {
    fn default() -> Self {
        Self {
            reinforcement: 1.0,
            half_life_days: DEFAULT_HALF_LIFE_DAYS,
            contradiction: 0.0,
            graph: 0.0,
            outcome: 0.0,
            confidence: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveNode {
    pub id: String,
    pub kind: CognitiveNodeKind,
    pub scope: CognitiveScope,
    pub content: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub salient_tokens: Vec<String>,
    #[serde(default)]
    pub token_count_estimate: usize,
    #[serde(default)]
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub weights: CognitiveWeights,
    #[serde(default)]
    pub provenance: BTreeMap<String, String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveEdge {
    pub from: String,
    pub to: String,
    pub kind: CognitiveEdgeKind,
    #[serde(default = "default_one")]
    pub weight: f64,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionSignal {
    pub node_id: String,
    pub recorded_at: DateTime<Utc>,
    pub success: bool,
    pub delta: f64,
    pub source: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalDecision {
    pub recorded_at: DateTime<Utc>,
    pub query: String,
    pub selected_node_ids: Vec<String>,
    pub total_score: f64,
    pub token_budget: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveStore {
    pub version: u32,
    #[serde(default)]
    pub nodes: BTreeMap<String, CognitiveNode>,
    #[serde(default)]
    pub edges: Vec<CognitiveEdge>,
    #[serde(default)]
    pub execution_signals: Vec<ExecutionSignal>,
    #[serde(default)]
    pub retrieval_decisions: VecDeque<RetrievalDecision>,
}

impl Default for CognitiveStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            execution_signals: Vec::new(),
            retrieval_decisions: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredNode {
    pub id: String,
    pub score: f64,
    pub token_count_estimate: usize,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UpsertNode {
    pub id_hint: String,
    pub kind: CognitiveNodeKind,
    pub scope: CognitiveScope,
    pub content: String,
    pub tags: Vec<String>,
    pub source: String,
    pub provenance: BTreeMap<String, String>,
}

pub fn store_path() -> PathBuf {
    kcode_home()
        .join("self_memory")
        .join("adaptive_cognition.json")
}

pub fn kcode_home() -> PathBuf {
    std::env::var_os("KCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".kcode")))
        .unwrap_or_else(|| PathBuf::from(".kcode"))
}

pub fn load_store() -> io::Result<CognitiveStore> {
    load_store_from_path(&store_path())
}

pub fn save_store(store: &CognitiveStore) -> io::Result<()> {
    save_store_to_path(&store_path(), store)
}

pub fn upsert_node(input: UpsertNode) -> io::Result<String> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let id = upsert_node_in_store(&mut store, input);
    evolve_store(&mut store);
    save_store_to_path(&path, &store)?;
    Ok(id)
}

pub fn link_execution_outcome(
    node_id: &str,
    success: bool,
    delta: f64,
    source: impl Into<String>,
    summary: impl Into<String>,
) -> io::Result<()> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    store.execution_signals.push(ExecutionSignal {
        node_id: node_id.to_string(),
        recorded_at: Utc::now(),
        success,
        delta,
        source: source.into(),
        summary: summary.into(),
    });
    evolve_store(&mut store);
    save_store_to_path(&path, &store)
}

pub fn retrieve_for_prompt(query: &str, token_budget: usize) -> io::Result<Vec<ScoredNode>> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    evolve_store(&mut store);
    let selected = retrieve_from_store(&store, query, token_budget);
    let total_score = selected.iter().map(|node| node.score).sum();
    let selected_node_ids = selected.iter().map(|node| node.id.clone()).collect();
    store.retrieval_decisions.push_back(RetrievalDecision {
        recorded_at: Utc::now(),
        query: query.to_string(),
        selected_node_ids,
        total_score,
        token_budget,
        reason: "adaptive retrieval: reinforcement * decay * contradiction * graph * outcome * query relevance".to_string(),
    });
    while store.retrieval_decisions.len() > MAX_DECISIONS {
        store.retrieval_decisions.pop_front();
    }
    save_store_to_path(&path, &store)?;
    Ok(selected)
}

pub fn inspector_markdown(limit: usize) -> io::Result<String> {
    let mut store = load_store()?;
    evolve_store(&mut store);
    let ranked = retrieve_from_store(
        &store,
        ".kcode memory cognition",
        MAX_CONTEXT_TOKENS_DEFAULT,
    );
    let mut lines = vec![
        "# Adaptive cognition inspector".to_string(),
        format!("nodes: {}", store.nodes.len()),
        format!("edges: {}", store.edges.len()),
        format!("execution signals: {}", store.execution_signals.len()),
        "".to_string(),
        "## Top nodes".to_string(),
    ];
    for scored in ranked.into_iter().take(limit) {
        if let Some(node) = store.nodes.get(&scored.id) {
            lines.push(format!(
                "- `{}` score={:.3} kind={:?} scope={:?} reinforce={:.2} contradiction={:.2} graph={:.2} outcome={:.2}: {}",
                node.id,
                scored.score,
                node.kind,
                node.scope,
                node.weights.reinforcement,
                node.weights.contradiction,
                node.weights.graph,
                node.weights.outcome,
                compact(&node.content, 160)
            ));
        }
    }
    Ok(lines.join("\n"))
}

pub fn upsert_node_in_store(store: &mut CognitiveStore, input: UpsertNode) -> String {
    normalize_store(store);
    let now = Utc::now();
    let normalized = normalize(&input.content);
    if let Some((id, node)) = store
        .nodes
        .iter_mut()
        .find(|(_, node)| normalize(&node.content) == normalized)
    {
        node.weights.reinforcement = (node.weights.reinforcement + 0.20).clamp(0.0, 10.0);
        node.updated_at = now;
        node.last_used_at = Some(now);
        node.version += 1;
        return id.clone();
    }

    let id = stable_node_id(&input.id_hint, &input.content);
    let salient_tokens = salient_tokens(&input.content);
    let token_count_estimate = estimate_token_count(&input.content);
    let node = CognitiveNode {
        id: id.clone(),
        kind: input.kind,
        scope: input.scope,
        summary: compact(&input.content, 240),
        content: input.content,
        tags: normalized_tags(input.tags),
        salient_tokens,
        token_count_estimate,
        source: input.source,
        created_at: now,
        updated_at: now,
        last_used_at: Some(now),
        weights: CognitiveWeights::default(),
        provenance: input.provenance,
        active: true,
        version: 1,
    };
    store.nodes.insert(id.clone(), node);
    id
}

pub fn evolve_store(store: &mut CognitiveStore) {
    normalize_store(store);
    recompute_edges(store);
    recompute_weights(store);
}

pub fn retrieve_from_store(
    store: &CognitiveStore,
    query: &str,
    token_budget: usize,
) -> Vec<ScoredNode> {
    let query_tokens = salient_tokens(query);
    let now = Utc::now();
    let mut ranked: Vec<_> = store
        .nodes
        .values()
        .filter(|node| node.active)
        .map(|node| score_node(store, node, &query_tokens, now))
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.token_count_estimate.cmp(&b.token_count_estimate))
    });

    let mut used = 0;
    let mut selected = Vec::new();
    for node in ranked {
        if selected.is_empty() || used + node.token_count_estimate <= token_budget {
            used += node.token_count_estimate;
            selected.push(node);
        }
    }
    selected
}

fn score_node(
    store: &CognitiveStore,
    node: &CognitiveNode,
    query_tokens: &[String],
    now: DateTime<Utc>,
) -> ScoredNode {
    let decay = temporal_decay(node, now);
    let contradiction_factor = (1.0 - node.weights.contradiction.clamp(0.0, 1.0) * 0.45).max(0.0);
    let graph_factor = 1.0 + node.weights.graph.clamp(0.0, 0.75);
    let outcome_factor = 1.0 + node.weights.outcome.clamp(-0.6, 0.6);
    let query_factor = 1.0 + token_overlap_ratio(&node.salient_tokens, query_tokens).min(1.0);
    let scope_factor = match node.scope {
        CognitiveScope::Turn => 0.75,
        CognitiveScope::Session => 0.9,
        CognitiveScope::Project => 1.1,
        CognitiveScope::Global => 1.0,
    };
    let edge_support = store
        .edges
        .iter()
        .filter(|edge| {
            edge.to == node.id
                && matches!(
                    edge.kind,
                    CognitiveEdgeKind::Supports | CognitiveEdgeKind::Refines
                )
        })
        .map(|edge| edge.weight)
        .sum::<f64>()
        .min(0.35);
    let score = node.weights.reinforcement.max(0.0)
        * node.weights.confidence.clamp(0.0, 1.0)
        * decay
        * contradiction_factor
        * graph_factor
        * outcome_factor
        * query_factor
        * scope_factor
        * (1.0 + edge_support);
    ScoredNode {
        id: node.id.clone(),
        score,
        token_count_estimate: node.token_count_estimate,
        reasons: vec![
            format!("decay={decay:.2}"),
            format!("contradiction={:.2}", node.weights.contradiction),
            format!("graph={:.2}", node.weights.graph),
            format!("outcome={:.2}", node.weights.outcome),
            format!("query={query_factor:.2}"),
        ],
    }
}

fn recompute_edges(store: &mut CognitiveStore) {
    let mut edges = Vec::new();
    let nodes: Vec<_> = store.nodes.values().cloned().collect();
    for (idx, left) in nodes.iter().enumerate() {
        for right in nodes.iter().skip(idx + 1) {
            let overlap = token_overlap_ratio(&left.salient_tokens, &right.salient_tokens);
            let shared_tags = shared_tag_ratio(&left.tags, &right.tags);
            if overlap > 0.22 || shared_tags > 0.25 {
                let weight = (overlap * 0.6 + shared_tags * 0.4).clamp(0.05, 1.0);
                let kind = if contradicts(left, right) {
                    CognitiveEdgeKind::Contradicts
                } else if left.created_at <= right.created_at {
                    CognitiveEdgeKind::Supports
                } else {
                    CognitiveEdgeKind::Refines
                };
                edges.push(CognitiveEdge {
                    from: left.id.clone(),
                    to: right.id.clone(),
                    kind: kind.clone(),
                    weight,
                    created_at: Utc::now(),
                    evidence: format!("overlap={overlap:.2}, tags={shared_tags:.2}"),
                });
                edges.push(CognitiveEdge {
                    from: right.id.clone(),
                    to: left.id.clone(),
                    kind,
                    weight,
                    created_at: Utc::now(),
                    evidence: format!("overlap={overlap:.2}, tags={shared_tags:.2}"),
                });
            }
        }
    }
    for signal in &store.execution_signals {
        if store.nodes.contains_key(&signal.node_id) {
            edges.push(CognitiveEdge {
                from: signal.node_id.clone(),
                to: format!("outcome:{}", signal.recorded_at.timestamp_millis()),
                kind: CognitiveEdgeKind::HasOutcome,
                weight: signal.delta.abs().clamp(0.01, 1.0),
                created_at: signal.recorded_at,
                evidence: signal.summary.clone(),
            });
        }
    }
    store.edges = edges;
}

fn recompute_weights(store: &mut CognitiveStore) {
    let mut graph_weights: HashMap<String, f64> = HashMap::new();
    let mut contradiction_weights: HashMap<String, f64> = HashMap::new();
    for edge in &store.edges {
        match edge.kind {
            CognitiveEdgeKind::Contradicts => {
                *contradiction_weights.entry(edge.from.clone()).or_default() += edge.weight * 0.35;
            }
            CognitiveEdgeKind::Supports
            | CognitiveEdgeKind::Refines
            | CognitiveEdgeKind::SameTopic => {
                *graph_weights.entry(edge.from.clone()).or_default() += edge.weight * 0.08;
            }
            _ => {}
        }
    }
    let mut outcome_weights: HashMap<String, f64> = HashMap::new();
    for signal in &store.execution_signals {
        *outcome_weights.entry(signal.node_id.clone()).or_default() += signal.delta;
    }
    for node in store.nodes.values_mut() {
        node.weights.graph = graph_weights
            .get(&node.id)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 0.75);
        node.weights.contradiction = contradiction_weights
            .get(&node.id)
            .copied()
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        node.weights.outcome = outcome_weights
            .get(&node.id)
            .copied()
            .unwrap_or(0.0)
            .clamp(-0.6, 0.6);
    }
}

fn temporal_decay(node: &CognitiveNode, now: DateTime<Utc>) -> f64 {
    let anchor = node.last_used_at.unwrap_or(node.updated_at);
    let age_days = (now - anchor).num_seconds().max(0) as f64 / 86_400.0;
    0.5_f64
        .powf(age_days / node.weights.half_life_days.max(1.0))
        .clamp(MIN_DECAY, 1.0)
}

fn normalize_store(store: &mut CognitiveStore) {
    store.version = STORE_VERSION;
    for node in store.nodes.values_mut() {
        if node.salient_tokens.is_empty() {
            node.salient_tokens = salient_tokens(&node.content);
        }
        if node.token_count_estimate == 0 {
            node.token_count_estimate = estimate_token_count(&node.content);
        }
        if node.summary.is_empty() {
            node.summary = compact(&node.content, 240);
        }
        if node.weights.reinforcement == 0.0 {
            node.weights.reinforcement = 1.0;
        }
        if node.weights.half_life_days == 0.0 {
            node.weights.half_life_days = DEFAULT_HALF_LIFE_DAYS;
        }
        node.tags = normalized_tags(std::mem::take(&mut node.tags));
    }
}

fn contradicts(left: &CognitiveNode, right: &CognitiveNode) -> bool {
    let overlap = token_overlap_ratio(&left.salient_tokens, &right.salient_tokens);
    overlap > 0.25
        && (is_negating(&left.content) != is_negating(&right.content)
            || has_explicit_contradiction_pair(&left.content, &right.content))
}

fn is_negating(text: &str) -> bool {
    let lower = format!(" {} ", text.to_ascii_lowercase());
    [
        " don't ",
        " do not ",
        " never ",
        " stop ",
        " disable ",
        " remove ",
        " not ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn has_explicit_contradiction_pair(left: &str, right: &str) -> bool {
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    [
        ("enable", "disable"),
        ("always", "never"),
        ("remember", "forget"),
        ("increase", "decrease"),
        ("persist", "discard"),
    ]
    .iter()
    .any(|(a, b)| {
        (left.contains(a) && right.contains(b)) || (left.contains(b) && right.contains(a))
    })
}

fn normalized_tags(tags: Vec<String>) -> Vec<String> {
    let mut tags: Vec<_> = tags
        .into_iter()
        .map(|tag| tag.trim().to_ascii_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

fn shared_tag_ratio(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let right: BTreeSet<_> = right.iter().collect();
    let overlap = left.iter().filter(|tag| right.contains(tag)).count();
    overlap as f64 / left.len().max(right.len()) as f64
}

fn token_overlap_ratio(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let right: BTreeSet<_> = right.iter().collect();
    let overlap = left.iter().filter(|token| right.contains(token)).count();
    overlap as f64 / left.len().max(right.len()) as f64
}

pub fn estimate_token_count(text: &str) -> usize {
    text.split_whitespace()
        .map(|w| (w.len().max(1) + 3) / 4)
        .sum::<usize>()
        .max(1)
}

pub fn salient_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '.')
        .map(str::trim)
        .filter(|token| token.len() > 2)
        .map(|token| token.to_ascii_lowercase())
        .take(32)
        .collect()
}

fn stable_node_id(id_hint: &str, content: &str) -> String {
    let slug = salient_tokens(content)
        .into_iter()
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let mut hash: u64 = 1469598103934665603;
    for byte in format!("{id_hint}:{content}").as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!(
        "cog-{}-{hash:016x}",
        if slug.is_empty() { "node" } else { &slug }
    )
}

fn compact(text: &str, max_chars: usize) -> String {
    let mut text = text.replace('\n', " ");
    if text.len() > max_chars {
        text.truncate(max_chars);
        text.push_str("...");
    }
    text
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn load_store_from_path(path: &Path) -> io::Result<CognitiveStore> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(CognitiveStore::default()),
        Err(err) => Err(err),
    }
}

fn save_store_to_path(path: &Path, store: &CognitiveStore) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(path, json)
}

fn default_one() -> f64 {
    1.0
}

fn default_half_life() -> f64 {
    DEFAULT_HALF_LIFE_DAYS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upsert(content: &str) -> UpsertNode {
        UpsertNode {
            id_hint: "test".to_string(),
            kind: CognitiveNodeKind::Directive,
            scope: CognitiveScope::Project,
            content: content.to_string(),
            tags: vec![".kcode".to_string(), "memory".to_string()],
            source: "test".to_string(),
            provenance: BTreeMap::new(),
        }
    }

    #[test]
    fn upsert_reinforces_duplicate_nodes() {
        let mut store = CognitiveStore::default();
        let id = upsert_node_in_store(&mut store, upsert("remember .kcode memory"));
        let id2 = upsert_node_in_store(&mut store, upsert("remember .kcode memory"));
        assert_eq!(id, id2);
        assert_eq!(store.nodes.len(), 1);
        assert!(store.nodes[&id].weights.reinforcement > 1.0);
    }

    #[test]
    fn evolution_links_related_and_contradictory_nodes() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert("always remember .kcode graph memory"));
        upsert_node_in_store(&mut store, upsert("never remember .kcode graph memory"));
        evolve_store(&mut store);
        assert!(
            store
                .edges
                .iter()
                .any(|edge| matches!(edge.kind, CognitiveEdgeKind::Contradicts))
        );
        assert!(
            store
                .nodes
                .values()
                .any(|node| node.weights.contradiction > 0.0)
        );
    }

    #[test]
    fn retrieval_respects_token_budget_and_query_relevance() {
        let mut store = CognitiveStore::default();
        let graph_id =
            upsert_node_in_store(&mut store, upsert(".kcode graph traversal scoring memory"));
        upsert_node_in_store(
            &mut store,
            upsert(".kcode unrelated preference about colors"),
        );
        evolve_store(&mut store);
        let selected = retrieve_from_store(&store, "graph traversal", 30);
        assert!(!selected.is_empty());
        assert_eq!(selected[0].id, graph_id);
    }

    #[test]
    fn execution_outcomes_change_scores() {
        let mut store = CognitiveStore::default();
        let id = upsert_node_in_store(&mut store, upsert(".kcode execution outcome linkage"));
        evolve_store(&mut store);
        let before = retrieve_from_store(&store, "execution", 100)[0].score;
        store.execution_signals.push(ExecutionSignal {
            node_id: id.clone(),
            recorded_at: Utc::now(),
            success: true,
            delta: 0.4,
            source: "test".to_string(),
            summary: "worked".to_string(),
        });
        evolve_store(&mut store);
        let after = retrieve_from_store(&store, "execution", 100)[0].score;
        assert!(after > before);
    }
}
