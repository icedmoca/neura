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
    #[serde(default)]
    pub operational_state: OperationalCognitionState,
}

impl Default for CognitiveStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            execution_signals: Vec::new(),
            retrieval_decisions: VecDeque::new(),
            operational_state: OperationalCognitionState::default(),
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OperationalMode {
    Observe,
    Retrieve,
    Plan,
    Execute,
    Reflect,
    Compress,
    Repair,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OperationalTaskKind {
    Reinforce,
    Decay,
    ContradictionAudit,
    StabilityAudit,
    EntropyAudit,
    Compression,
    Reflection,
    Snapshot,
    SandboxDryRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_entropy_threshold")]
    pub entropy_threshold: f64,
    #[serde(default = "default_stability_floor")]
    pub stability_floor: f64,
    #[serde(default = "default_max_tasks_per_cycle")]
    pub max_tasks_per_cycle: usize,
    #[serde(default = "default_snapshot_interval_minutes")]
    pub snapshot_interval_minutes: i64,
    #[serde(default)]
    pub sandbox_required_for_destructive_actions: bool,
}

impl Default for OperationalPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            entropy_threshold: 0.72,
            stability_floor: 0.45,
            max_tasks_per_cycle: 8,
            snapshot_interval_minutes: 30,
            sandbox_required_for_destructive_actions: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalTask {
    pub id: String,
    pub kind: OperationalTaskKind,
    pub created_at: DateTime<Utc>,
    pub due_at: DateTime<Utc>,
    #[serde(default = "default_one")]
    pub priority: f64,
    #[serde(default)]
    pub target_node_ids: Vec<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalCycleRecord {
    pub recorded_at: DateTime<Utc>,
    pub mode: OperationalMode,
    pub entropy: f64,
    pub stability: f64,
    pub scheduled: usize,
    pub executed: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionSnapshotRef {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub node_count: usize,
    pub edge_count: usize,
    pub stability_score: f64,
    pub entropy_score: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalCognitionState {
    #[serde(default)]
    pub policy: OperationalPolicy,
    #[serde(default)]
    pub active_mode: Option<OperationalMode>,
    #[serde(default)]
    pub task_queue: VecDeque<OperationalTask>,
    #[serde(default)]
    pub cycle_history: VecDeque<OperationalCycleRecord>,
    #[serde(default)]
    pub snapshots: Vec<CognitionSnapshotRef>,
    #[serde(default)]
    pub last_cycle_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub execution_plans: VecDeque<CognitionExecutionPlan>,
    #[serde(default)]
    pub governor_reports: VecDeque<ExecutionGovernorReport>,
    #[serde(default)]
    pub procedural_runtime: ProceduralRuntimeState,
    #[serde(default)]
    pub cognitive_fabric: CognitiveFabricState,
    #[serde(default)]
    pub distributed_fabric: DistributedCognitionState,
    #[serde(default)]
    pub strategic_civilization: StrategicCivilizationState,
    #[serde(default)]
    pub civilization_os: CivilizationOsState,
    #[serde(default)]
    pub sovereign_ecosystem: SovereignEcosystemState,
    #[serde(default)]
    pub hardening_runtime: HardeningRuntimeState,
    #[serde(default)]
    pub reality_coupling: RealityCouplingState,
    #[serde(default)]
    pub epistemology: EpistemologyState,
    #[serde(default)]
    pub deliberative_science: DeliberativeScienceState,
    #[serde(default)]
    pub synthetic_governance: SyntheticScientificGovernanceState,
    #[serde(default)]
    pub cognitive_context_economy: CognitiveIntegrationContextEconomyState,
    #[serde(default)]
    pub hierarchical_epistemic_context: HierarchicalActivationEpistemicContextState,
    #[serde(default)]
    pub emergent_quality_coherence: EmergentCognitionQualityCoherenceState,
    #[serde(default)]
    pub cognitive_substrate_synthesis: CognitiveSubstrateSynthesisState,
}

impl Default for OperationalCognitionState {
    fn default() -> Self {
        Self {
            policy: OperationalPolicy::default(),
            active_mode: None,
            task_queue: VecDeque::new(),
            cycle_history: VecDeque::new(),
            snapshots: Vec::new(),
            last_cycle_at: None,
            execution_plans: VecDeque::new(),
            governor_reports: VecDeque::new(),
            procedural_runtime: ProceduralRuntimeState::default(),
            cognitive_fabric: CognitiveFabricState::default(),
            distributed_fabric: DistributedCognitionState::default(),
            strategic_civilization: StrategicCivilizationState::default(),
            civilization_os: CivilizationOsState::default(),
            sovereign_ecosystem: SovereignEcosystemState::default(),
            hardening_runtime: HardeningRuntimeState::default(),
            reality_coupling: RealityCouplingState::default(),
            epistemology: EpistemologyState::default(),
            deliberative_science: DeliberativeScienceState::default(),
            synthetic_governance: SyntheticScientificGovernanceState::default(),
            cognitive_context_economy: CognitiveIntegrationContextEconomyState::default(),
            hierarchical_epistemic_context: HierarchicalActivationEpistemicContextState::default(),
            emergent_quality_coherence: EmergentCognitionQualityCoherenceState::default(),
            cognitive_substrate_synthesis: CognitiveSubstrateSynthesisState::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalCycleReport {
    pub mode: OperationalMode,
    pub entropy: f64,
    pub stability: f64,
    pub scheduled_tasks: Vec<OperationalTask>,
    pub executed_tasks: Vec<OperationalTask>,
    pub snapshot: Option<CognitionSnapshotRef>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EpistemicRelationKind {
    Supports,
    Contradicts,
    Refines,
    DependsOn,
    Explains,
    Supersedes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicRelation {
    pub from_claim: String,
    pub to_claim: String,
    pub kind: EpistemicRelationKind,
    pub weight: f64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicConflictSet {
    pub id: String,
    pub claim_ids: Vec<String>,
    pub severity: f64,
    pub resolution_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RevisionTransaction {
    pub id: String,
    pub revised_at: DateTime<Utc>,
    pub claim_ids: Vec<String>,
    pub delta: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicDelta {
    pub claim_id: String,
    pub old_confidence: f64,
    pub new_confidence: f64,
    pub cause: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EpistemicStatus {
    Unknown,
    Hypothesis,
    Supported,
    Verified,
    Contradicted,
    Deprecated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EvidenceKind {
    Telemetry,
    Test,
    Build,
    UserStatement,
    MemoryTrace,
    RuntimeObservation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceRecord {
    pub id: String,
    pub kind: EvidenceKind,
    pub observed_at: DateTime<Utc>,
    pub content: String,
    pub reliability: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicClaim {
    pub id: String,
    pub statement: String,
    pub status: EpistemicStatus,
    pub confidence: f64,
    pub evidence_ids: Vec<String>,
    pub contradiction_ids: Vec<String>,
    pub last_revised_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceReliability {
    pub source: String,
    pub reliability: f64,
    pub observations: u64,
    pub failures: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WrongnessSignal {
    pub claim_id: String,
    pub severity: f64,
    pub reason: String,
    pub correction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeliefRevision {
    pub claim_id: String,
    pub revised_at: DateTime<Utc>,
    pub old_confidence: f64,
    pub new_confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemologyReport {
    pub generated_at: DateTime<Utc>,
    pub claims: Vec<EpistemicClaim>,
    pub evidence: Vec<EvidenceRecord>,
    pub reliabilities: Vec<SourceReliability>,
    pub wrongness: Vec<WrongnessSignal>,
    pub revisions: Vec<BeliefRevision>,
    pub relations: Vec<EpistemicRelation>,
    pub conflict_sets: Vec<EpistemicConflictSet>,
    pub deltas: Vec<EpistemicDelta>,
    pub epistemic_health: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EpistemologyState {
    #[serde(default)]
    pub claims: BTreeMap<String, EpistemicClaim>,
    #[serde(default)]
    pub evidence: BTreeMap<String, EvidenceRecord>,
    #[serde(default)]
    pub source_reliability: BTreeMap<String, SourceReliability>,
    #[serde(default)]
    pub wrongness: VecDeque<WrongnessSignal>,
    #[serde(default)]
    pub revisions: VecDeque<BeliefRevision>,
    #[serde(default)]
    pub reports: VecDeque<EpistemologyReport>,
    #[serde(default)]
    pub relations: Vec<EpistemicRelation>,
    #[serde(default)]
    pub conflict_sets: Vec<EpistemicConflictSet>,
    #[serde(default)]
    pub revision_transactions: VecDeque<RevisionTransaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TelemetryKind {
    FileSystem,
    GitState,
    BuildResult,
    TestResult,
    RuntimeVersion,
    MemoryStore,
    UserFeedback,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TelemetrySample {
    pub id: String,
    pub kind: TelemetryKind,
    pub captured_at: DateTime<Utc>,
    pub value: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationClaim {
    pub id: String,
    pub claim: String,
    pub evidence_ids: Vec<String>,
    pub verified: bool,
    pub confidence: f64,
    pub corrective_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PredictionCalibration {
    pub predictor: String,
    pub predicted: f64,
    pub observed: f64,
    pub error: f64,
    pub sample_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorldStateNode {
    pub id: String,
    pub label: String,
    pub evidence_ids: Vec<String>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntropySource {
    pub name: String,
    pub contribution: f64,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RealityCouplingReport {
    pub generated_at: DateTime<Utc>,
    pub telemetry: Vec<TelemetrySample>,
    pub claims: Vec<VerificationClaim>,
    pub calibrations: Vec<PredictionCalibration>,
    pub world_state: Vec<WorldStateNode>,
    pub entropy_sources: Vec<EntropySource>,
    pub coupling_score: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RealityCouplingState {
    #[serde(default)]
    pub telemetry: VecDeque<TelemetrySample>,
    #[serde(default)]
    pub claims: VecDeque<VerificationClaim>,
    #[serde(default)]
    pub calibrations: BTreeMap<String, PredictionCalibration>,
    #[serde(default)]
    pub world_state: BTreeMap<String, WorldStateNode>,
    #[serde(default)]
    pub reports: VecDeque<RealityCouplingReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RealityAnchorKind {
    TestResult,
    BuildResult,
    GitCommit,
    FileState,
    UserDirective,
    RuntimeInstall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RealityAnchor {
    pub id: String,
    pub kind: RealityAnchorKind,
    pub observed_at: DateTime<Utc>,
    pub evidence: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OntologyStabilityCheck {
    pub name: String,
    pub stable: bool,
    pub drift: f64,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GarbageCollectionDecision {
    pub target_id: String,
    pub reason: String,
    pub action: String,
    pub reclaimed_pressure: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NervousSystemPulse {
    pub pulsed_at: DateTime<Utc>,
    pub heartbeat_ok: bool,
    pub store_size: usize,
    pub pending_queues: usize,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DelusionCheck {
    pub claim: String,
    pub grounded: bool,
    pub evidence_count: usize,
    pub corrective_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImmuneResponse {
    pub trigger: String,
    pub severity: f64,
    pub response: String,
    pub quarantined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HardeningReport {
    pub generated_at: DateTime<Utc>,
    pub reality_anchors: Vec<RealityAnchor>,
    pub ontology_checks: Vec<OntologyStabilityCheck>,
    pub garbage_collection: Vec<GarbageCollectionDecision>,
    pub pulse: NervousSystemPulse,
    pub delusion_checks: Vec<DelusionCheck>,
    pub immune_responses: Vec<ImmuneResponse>,
    pub maturity_score: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HardeningRuntimeState {
    #[serde(default)]
    pub anchors: VecDeque<RealityAnchor>,
    #[serde(default)]
    pub reports: VecDeque<HardeningReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SovereignDomain {
    Constitution,
    Continuity,
    Law,
    Compression,
    Economy,
    Planning,
    Federation,
    Virtualization,
    Mythos,
    Ecosystem,
    Archaeology,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SovereignInvariant {
    pub id: String,
    pub domain: SovereignDomain,
    pub invariant: String,
    pub strength: f64,
    pub violation_pressure: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuityProtocol {
    pub id: String,
    pub layer: String,
    pub checkpoint: String,
    pub recovery_confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressionLaw {
    pub id: String,
    pub applies_to: String,
    pub policy: String,
    pub expected_savings: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveCurrency {
    pub name: String,
    pub balance: f64,
    pub inflow: f64,
    pub outflow: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VirtualizedRuntimeShard {
    pub id: String,
    pub purpose: String,
    pub isolation: f64,
    pub replayable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MythosFrame {
    pub id: String,
    pub narrative: String,
    pub utility: f64,
    pub grounded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EcosystemRelation {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub strength: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SovereignEcosystemReport {
    pub generated_at: DateTime<Utc>,
    pub invariants: Vec<SovereignInvariant>,
    pub continuity: Vec<ContinuityProtocol>,
    pub compression_laws: Vec<CompressionLaw>,
    pub currencies: Vec<CognitiveCurrency>,
    pub runtime_shards: Vec<VirtualizedRuntimeShard>,
    pub mythos: Vec<MythosFrame>,
    pub relations: Vec<EcosystemRelation>,
    pub sovereignty_score: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SovereignEcosystemState {
    #[serde(default)]
    pub invariants: BTreeMap<String, SovereignInvariant>,
    #[serde(default)]
    pub continuity: BTreeMap<String, ContinuityProtocol>,
    #[serde(default)]
    pub compression_laws: BTreeMap<String, CompressionLaw>,
    #[serde(default)]
    pub currencies: BTreeMap<String, CognitiveCurrency>,
    #[serde(default)]
    pub runtime_shards: BTreeMap<String, VirtualizedRuntimeShard>,
    #[serde(default)]
    pub mythos: BTreeMap<String, MythosFrame>,
    #[serde(default)]
    pub relations: Vec<EcosystemRelation>,
    #[serde(default)]
    pub reports: VecDeque<SovereignEcosystemReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InstitutionKind {
    Constitution,
    MemoryCourt,
    PlanningCouncil,
    VerificationOffice,
    ResourceTreasury,
    ContinuityArchive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Institution {
    pub id: String,
    pub kind: InstitutionKind,
    pub mandate: String,
    pub authority: f64,
    pub health: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GovernanceLaw {
    pub id: String,
    pub title: String,
    pub text: String,
    pub priority: f64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GovernancePrecedent {
    pub id: String,
    pub situation: String,
    pub decision: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScenarioPlan {
    pub id: String,
    pub scenario: String,
    pub probability: f64,
    pub impact: f64,
    pub response: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuityPlan {
    pub id: String,
    pub trigger: String,
    pub recovery_action: String,
    pub readiness: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiplomaticStance {
    pub peer: String,
    pub trust: f64,
    pub posture: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CivicMemoryEntry {
    pub id: String,
    pub remembered_at: DateTime<Utc>,
    pub lesson: String,
    pub applies_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CivilizationOsReport {
    pub generated_at: DateTime<Utc>,
    pub institutions: Vec<Institution>,
    pub laws: Vec<GovernanceLaw>,
    pub precedents: Vec<GovernancePrecedent>,
    pub scenarios: Vec<ScenarioPlan>,
    pub continuity: Vec<ContinuityPlan>,
    pub diplomacy: Vec<DiplomaticStance>,
    pub civic_memory: Vec<CivicMemoryEntry>,
    pub os_health: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CivilizationOsState {
    #[serde(default)]
    pub institutions: BTreeMap<String, Institution>,
    #[serde(default)]
    pub laws: BTreeMap<String, GovernanceLaw>,
    #[serde(default)]
    pub precedents: VecDeque<GovernancePrecedent>,
    #[serde(default)]
    pub civic_memory: VecDeque<CivicMemoryEntry>,
    #[serde(default)]
    pub reports: VecDeque<CivilizationOsReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DoctrineKind {
    Safety,
    Autonomy,
    Verification,
    MemoryEvolution,
    ResourceStewardship,
    Collaboration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DoctrineNode {
    pub id: String,
    pub kind: DoctrineKind,
    pub statement: String,
    pub priority: f64,
    pub confidence: f64,
    pub reinforced_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceKind {
    TokenBudget,
    TimeBudget,
    RiskBudget,
    BuildBudget,
    AttentionBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceAccount {
    pub kind: ResourceKind,
    pub capacity: f64,
    pub used: f64,
    pub reserved: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecursiveSynthesis {
    pub id: String,
    pub source_layers: Vec<String>,
    pub abstraction: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FederationPeer {
    pub id: String,
    pub trust: f64,
    pub advertised_capabilities: Vec<String>,
    pub last_sync_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IdentityAnchor {
    pub id: String,
    pub statement: String,
    pub stability: f64,
    pub last_confirmed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalSimulation {
    pub id: String,
    pub hypothesis: String,
    pub predicted_benefit: f64,
    pub predicted_risk: f64,
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StrategyHorizon {
    pub horizon_days: i64,
    pub goal: String,
    pub expected_capability: f64,
    pub required_resources: Vec<ResourceKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvolutionProposal {
    pub id: String,
    pub title: String,
    pub rationale: String,
    pub priority: f64,
    pub safe_to_autonomously_prepare: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchaeologyRecord {
    pub id: String,
    pub artifact: String,
    pub lesson: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StrategicCivilizationReport {
    pub generated_at: DateTime<Utc>,
    pub doctrines: Vec<DoctrineNode>,
    pub resources: Vec<ResourceAccount>,
    pub syntheses: Vec<RecursiveSynthesis>,
    pub federation: Vec<FederationPeer>,
    pub identity: Vec<IdentityAnchor>,
    pub simulations: Vec<CausalSimulation>,
    pub horizons: Vec<StrategyHorizon>,
    pub proposals: Vec<EvolutionProposal>,
    pub archaeology: Vec<ArchaeologyRecord>,
    pub civilization_score: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StrategicCivilizationState {
    #[serde(default)]
    pub doctrines: BTreeMap<String, DoctrineNode>,
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceAccount>,
    #[serde(default)]
    pub federation: BTreeMap<String, FederationPeer>,
    #[serde(default)]
    pub identity: BTreeMap<String, IdentityAnchor>,
    #[serde(default)]
    pub reports: VecDeque<StrategicCivilizationReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FabricNodeKind {
    LocalRuntime,
    MemorySubsystem,
    PlannerSubsystem,
    ExecutorSubsystem,
    VerifierSubsystem,
    ObserverSubsystem,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FabricNode {
    pub id: String,
    pub kind: FabricNodeKind,
    pub capabilities: Vec<String>,
    pub health: f64,
    pub load: f64,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConsensusStatus {
    Pending,
    Accepted,
    Rejected,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConsensusSignal {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub topic: String,
    pub participating_nodes: Vec<String>,
    pub confidence: f64,
    pub status: ConsensusStatus,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FabricRoute {
    pub capability: String,
    pub node_id: String,
    pub score: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FabricSyncRecord {
    pub synced_at: DateTime<Utc>,
    pub node_count: usize,
    pub consensus_count: usize,
    pub quorum_health: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DistributedFabricReport {
    pub generated_at: DateTime<Utc>,
    pub nodes: Vec<FabricNode>,
    pub routes: Vec<FabricRoute>,
    pub consensus: Vec<ConsensusSignal>,
    pub quorum_health: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DistributedCognitionState {
    #[serde(default)]
    pub nodes: BTreeMap<String, FabricNode>,
    #[serde(default)]
    pub consensus: VecDeque<ConsensusSignal>,
    #[serde(default)]
    pub routes: Vec<FabricRoute>,
    #[serde(default)]
    pub sync_history: VecDeque<FabricSyncRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AbstractionLevel {
    Token,
    Directive,
    Procedure,
    Subsystem,
    Doctrine,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubsystemKind {
    Memory,
    Retrieval,
    Planning,
    Execution,
    Verification,
    Reflection,
    Compression,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvironmentState {
    pub captured_at: DateTime<Utc>,
    pub node_pressure: f64,
    pub contradiction_pressure: f64,
    pub entropy: f64,
    pub stability: f64,
    pub build_ready: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubsystemState {
    pub kind: SubsystemKind,
    pub health: f64,
    pub load: f64,
    pub confidence: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentStateEstimate {
    pub name: String,
    pub probability: f64,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemporalForecast {
    pub horizon_minutes: i64,
    pub expected_entropy: f64,
    pub expected_stability: f64,
    pub recommended_mode: OperationalMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FabricArbitrationDecision {
    pub selected_subsystems: Vec<SubsystemKind>,
    pub suppressed_subsystems: Vec<SubsystemKind>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveFabricReport {
    pub generated_at: DateTime<Utc>,
    pub abstraction_levels: Vec<AbstractionLevel>,
    pub environment: EnvironmentState,
    pub subsystems: Vec<SubsystemState>,
    pub latent_states: Vec<LatentStateEstimate>,
    pub forecasts: Vec<TemporalForecast>,
    pub arbitration: FabricArbitrationDecision,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CognitiveFabricState {
    #[serde(default)]
    pub reports: VecDeque<CognitiveFabricReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProcedureStatus {
    Candidate,
    Active,
    Deprecated,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProcedureStepKind {
    Observe,
    Retrieve,
    Plan,
    ActDryRun,
    Verify,
    Reflect,
    Record,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcedureStep {
    pub kind: ProcedureStepKind,
    pub description: String,
    pub expected_signal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnedProcedure {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub name: String,
    pub trigger_tags: Vec<String>,
    pub steps: Vec<ProcedureStep>,
    pub status: ProcedureStatus,
    pub confidence: f64,
    pub success_count: u64,
    pub failure_count: u64,
    pub lineage: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutonomyDoctrine {
    #[serde(default = "default_true")]
    pub require_dry_run_for_code_changes: bool,
    #[serde(default = "default_true")]
    pub require_tests_before_install: bool,
    #[serde(default = "default_true")]
    pub require_commit_before_build_install: bool,
    #[serde(default = "default_autonomy_limit")]
    pub max_autonomous_risk: f64,
}

impl Default for AutonomyDoctrine {
    fn default() -> Self {
        Self {
            require_dry_run_for_code_changes: true,
            require_tests_before_install: true,
            require_commit_before_build_install: true,
            max_autonomous_risk: 0.35,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomePrediction {
    pub procedure_id: String,
    pub predicted_success: f64,
    pub predicted_risk: f64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProceduralRuntimeReport {
    pub generated_at: DateTime<Utc>,
    pub procedure_count: usize,
    pub active_procedure_count: usize,
    pub predictions: Vec<OutcomePrediction>,
    pub selected_procedure_ids: Vec<String>,
    pub safety_notes: Vec<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProceduralRuntimeState {
    #[serde(default)]
    pub doctrine: AutonomyDoctrine,
    #[serde(default)]
    pub procedures: BTreeMap<String, LearnedProcedure>,
    #[serde(default)]
    pub reports: VecDeque<ProceduralRuntimeReport>,
}

impl Default for ProceduralRuntimeState {
    fn default() -> Self {
        Self {
            doctrine: AutonomyDoctrine::default(),
            procedures: BTreeMap::new(),
            reports: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionStrategy {
    Conservative,
    Balanced,
    Exploratory,
    RepairFirst,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecutionActionKind {
    RetrieveContext,
    Replan,
    Reflect,
    CompressMemory,
    AuditContradictions,
    RecordOutcome,
    SnapshotRuntime,
    Noop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionAction {
    pub id: String,
    pub kind: ExecutionActionKind,
    pub target_node_ids: Vec<String>,
    pub rationale: String,
    pub expected_benefit: f64,
    pub risk: f64,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionExecutionPlan {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub strategy: ExecutionStrategy,
    pub health_score: f64,
    pub entropy: f64,
    pub stability: f64,
    pub risk_budget: f64,
    pub actions: Vec<ExecutionAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionActionResult {
    pub action_id: String,
    pub success: bool,
    pub score_delta: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionGovernorReport {
    pub plan: CognitionExecutionPlan,
    pub applied_results: Vec<ExecutionActionResult>,
    pub blocked_actions: Vec<ExecutionAction>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ObservationLayer {
    Raw,
    Signals,
    Graph,
    Clusters,
    Replay,
    Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionFrame {
    pub generated_at: DateTime<Utc>,
    pub layer: ObservationLayer,
    pub title: String,
    pub body: String,
    pub node_ids: Vec<String>,
    pub edge_count: usize,
    pub token_count_estimate: usize,
    pub stability_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionCluster {
    pub id: String,
    pub tags: Vec<String>,
    pub node_ids: Vec<String>,
    pub centroid_tokens: Vec<String>,
    pub stability_score: f64,
    pub contradiction_pressure: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionReplayEvent {
    pub at: DateTime<Utc>,
    pub event_type: String,
    pub target_id: String,
    pub summary: String,
    pub score_delta: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservableCognitionSnapshot {
    pub generated_at: DateTime<Utc>,
    pub store_version: u32,
    pub node_count: usize,
    pub edge_count: usize,
    pub cluster_count: usize,
    pub stability_score: f64,
    pub frames: Vec<CognitionFrame>,
    pub clusters: Vec<CognitionCluster>,
    pub replay: Vec<CognitionReplayEvent>,
}

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub layers: Vec<ObservationLayer>,
    pub token_budget: usize,
    pub include_replay: bool,
    pub include_graph: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            layers: vec![
                ObservationLayer::Summary,
                ObservationLayer::Clusters,
                ObservationLayer::Graph,
                ObservationLayer::Replay,
            ],
            token_budget: 1_600,
            include_replay: true,
            include_graph: true,
        }
    }
}

pub fn observable_snapshot(options: RenderOptions) -> io::Result<ObservableCognitionSnapshot> {
    let mut store = load_store()?;
    evolve_store(&mut store);
    Ok(observable_snapshot_from_store(&store, options))
}

pub fn observable_snapshot_from_store(
    store: &CognitiveStore,
    options: RenderOptions,
) -> ObservableCognitionSnapshot {
    let clusters = cognition_clusters(store);
    let replay = if options.include_replay {
        cognition_replay(store, 64)
    } else {
        Vec::new()
    };
    let stability_score = cognition_stability(store, &clusters);
    let mut frames = Vec::new();
    let mut remaining_budget = options.token_budget.max(128);
    for layer in &options.layers {
        let frame = render_frame(store, &clusters, &replay, layer.clone(), remaining_budget);
        remaining_budget = remaining_budget.saturating_sub(frame.token_count_estimate);
        frames.push(frame);
        if remaining_budget == 0 {
            break;
        }
    }
    ObservableCognitionSnapshot {
        generated_at: Utc::now(),
        store_version: store.version,
        node_count: store.nodes.len(),
        edge_count: store.edges.len(),
        cluster_count: clusters.len(),
        stability_score,
        frames,
        clusters,
        replay,
    }
}

pub fn render_observable_markdown(options: RenderOptions) -> io::Result<String> {
    let snapshot = observable_snapshot(options)?;
    Ok(render_snapshot_markdown(&snapshot))
}

pub fn render_observable_sideband(options: RenderOptions) -> io::Result<String> {
    let snapshot = observable_snapshot(options)?;
    let summary = format!(
        "nodes={},edges={},clusters={},stability={:.2}",
        snapshot.node_count, snapshot.edge_count, snapshot.cluster_count, snapshot.stability_score
    );
    let topics = snapshot
        .clusters
        .iter()
        .flat_map(|cluster| cluster.tags.iter().take(2).cloned())
        .take(8)
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "<ctx k=\"cognition\" id=\"cog:{}\" n={} c=\"{:.2}\" p=\"normal\" ar=\"false\" t=\"{}\" s=\"{}\" />",
        snapshot.generated_at.timestamp_millis(),
        estimate_token_count(&summary),
        snapshot.stability_score,
        escape_attr(&topics),
        escape_attr(&summary)
    ))
}

pub fn export_observable_graph_json(options: RenderOptions) -> io::Result<String> {
    let snapshot = observable_snapshot(options)?;
    serde_json::to_string_pretty(&snapshot)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn render_snapshot_markdown(snapshot: &ObservableCognitionSnapshot) -> String {
    let mut lines = vec![
        "# Observable adaptive cognition".to_string(),
        format!(
            "nodes={} edges={} clusters={} stability={:.2}",
            snapshot.node_count,
            snapshot.edge_count,
            snapshot.cluster_count,
            snapshot.stability_score
        ),
        String::new(),
    ];
    for frame in &snapshot.frames {
        lines.push(format!("## {:?}: {}", frame.layer, frame.title));
        lines.push(frame.body.clone());
        lines.push(String::new());
    }
    lines.join("\n")
}

fn render_frame(
    store: &CognitiveStore,
    clusters: &[CognitionCluster],
    replay: &[CognitionReplayEvent],
    layer: ObservationLayer,
    token_budget: usize,
) -> CognitionFrame {
    let mut body = String::new();
    let mut node_ids = Vec::new();
    let edge_count = store.edges.len();
    match layer {
        ObservationLayer::Raw => {
            for node in store.nodes.values().take(12) {
                node_ids.push(node.id.clone());
                body.push_str(&format!(
                    "- {} {:?}/{:?} r={:.2} c={:.2}: {}\n",
                    node.id,
                    node.kind,
                    node.scope,
                    node.weights.reinforcement,
                    node.weights.contradiction,
                    compact(&node.content, 120)
                ));
            }
        }
        ObservationLayer::Signals => {
            for signal in store.execution_signals.iter().rev().take(16) {
                node_ids.push(signal.node_id.clone());
                body.push_str(&format!(
                    "- {} {} delta={:.2}: {}\n",
                    signal.recorded_at,
                    if signal.success { "success" } else { "failure" },
                    signal.delta,
                    compact(&signal.summary, 140)
                ));
            }
        }
        ObservationLayer::Graph => {
            for edge in store.edges.iter().take(24) {
                node_ids.push(edge.from.clone());
                node_ids.push(edge.to.clone());
                body.push_str(&format!(
                    "- {} -{:?}/{:.2}-> {} ({})\n",
                    edge.from,
                    edge.kind,
                    edge.weight,
                    edge.to,
                    compact(&edge.evidence, 100)
                ));
            }
        }
        ObservationLayer::Clusters => {
            for cluster in clusters.iter().take(12) {
                node_ids.extend(cluster.node_ids.clone());
                body.push_str(&format!(
                    "- {} nodes={} stability={:.2} contradiction={:.2} tags=[{}] tokens=[{}]\n",
                    cluster.id,
                    cluster.node_ids.len(),
                    cluster.stability_score,
                    cluster.contradiction_pressure,
                    cluster.tags.join(","),
                    cluster.centroid_tokens.join(",")
                ));
            }
        }
        ObservationLayer::Replay => {
            for event in replay.iter().take(20) {
                node_ids.push(event.target_id.clone());
                body.push_str(&format!(
                    "- {} {} {} delta={:.2}: {}\n",
                    event.at,
                    event.event_type,
                    event.target_id,
                    event.score_delta,
                    compact(&event.summary, 120)
                ));
            }
        }
        ObservationLayer::Summary => {
            let top = retrieve_from_store(store, ".kcode cognition memory", 900);
            for scored in top.into_iter().take(8) {
                node_ids.push(scored.id.clone());
                if let Some(node) = store.nodes.get(&scored.id) {
                    body.push_str(&format!(
                        "- score={:.2} {} {:?}: {} reasons={}\n",
                        scored.score,
                        node.id,
                        node.kind,
                        compact(&node.summary, 140),
                        scored.reasons.join(",")
                    ));
                }
            }
        }
    }
    if body.is_empty() {
        body = "No cognition artifacts recorded yet.".to_string();
    }
    body = truncate_to_token_budget(&body, token_budget);
    node_ids.sort();
    node_ids.dedup();
    let stability_score = if clusters.is_empty() {
        1.0
    } else {
        clusters
            .iter()
            .map(|cluster| cluster.stability_score)
            .sum::<f64>()
            / clusters.len() as f64
    };
    CognitionFrame {
        generated_at: Utc::now(),
        layer: layer.clone(),
        title: match layer {
            ObservationLayer::Raw => "raw memory nodes",
            ObservationLayer::Signals => "execution and reinforcement signals",
            ObservationLayer::Graph => "weighted memory graph traversal",
            ObservationLayer::Clusters => "abstraction clusters",
            ObservationLayer::Replay => "cognitive replay timeline",
            ObservationLayer::Summary => "token-bounded retrieval summary",
        }
        .to_string(),
        token_count_estimate: estimate_token_count(&body),
        body,
        node_ids,
        edge_count,
        stability_score,
    }
}

fn cognition_clusters(store: &CognitiveStore) -> Vec<CognitionCluster> {
    let mut groups: BTreeMap<String, Vec<&CognitiveNode>> = BTreeMap::new();
    for node in store.nodes.values() {
        let key = node
            .tags
            .first()
            .cloned()
            .or_else(|| node.salient_tokens.first().cloned())
            .unwrap_or_else(|| "untagged".to_string());
        groups.entry(key).or_default().push(node);
    }
    let mut clusters = Vec::new();
    for (key, nodes) in groups {
        let mut tag_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut token_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut node_ids = Vec::new();
        let mut contradiction = 0.0;
        let mut reinforcement = 0.0;
        for node in nodes {
            node_ids.push(node.id.clone());
            contradiction += node.weights.contradiction;
            reinforcement += node.weights.reinforcement;
            for tag in &node.tags {
                *tag_counts.entry(tag.clone()).or_default() += 1;
            }
            for token in &node.salient_tokens {
                *token_counts.entry(token.clone()).or_default() += 1;
            }
        }
        let tags = top_counts(tag_counts, 8);
        let centroid_tokens = top_counts(token_counts, 10);
        let size = node_ids.len().max(1) as f64;
        let contradiction_pressure = (contradiction / size).clamp(0.0, 1.0);
        let avg_reinforcement = (reinforcement / size).clamp(0.0, 10.0);
        let stability_score = ((1.0 - contradiction_pressure)
            * (avg_reinforcement / (avg_reinforcement + 1.0)))
            .clamp(0.0, 1.0);
        clusters.push(CognitionCluster {
            id: format!("cluster-{key}"),
            tags,
            node_ids,
            centroid_tokens,
            stability_score,
            contradiction_pressure,
        });
    }
    clusters.sort_by(|a, b| {
        b.stability_score
            .partial_cmp(&a.stability_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.node_ids.len().cmp(&a.node_ids.len()))
    });
    clusters
}

fn cognition_replay(store: &CognitiveStore, limit: usize) -> Vec<CognitionReplayEvent> {
    let mut events = Vec::new();
    for node in store.nodes.values() {
        events.push(CognitionReplayEvent {
            at: node.created_at,
            event_type: "node_created".to_string(),
            target_id: node.id.clone(),
            summary: compact(&node.content, 160),
            score_delta: node.weights.reinforcement,
        });
        if node.version > 1 {
            events.push(CognitionReplayEvent {
                at: node.updated_at,
                event_type: "node_reinforced".to_string(),
                target_id: node.id.clone(),
                summary: format!(
                    "version={} reinforcement={:.2}",
                    node.version, node.weights.reinforcement
                ),
                score_delta: node.weights.reinforcement - 1.0,
            });
        }
    }
    for signal in &store.execution_signals {
        events.push(CognitionReplayEvent {
            at: signal.recorded_at,
            event_type: if signal.success {
                "execution_success"
            } else {
                "execution_failure"
            }
            .to_string(),
            target_id: signal.node_id.clone(),
            summary: signal.summary.clone(),
            score_delta: signal.delta,
        });
    }
    for decision in &store.retrieval_decisions {
        events.push(CognitionReplayEvent {
            at: decision.recorded_at,
            event_type: "retrieval".to_string(),
            target_id: decision.selected_node_ids.join(","),
            summary: format!(
                "query={} score={:.2}",
                compact(&decision.query, 120),
                decision.total_score
            ),
            score_delta: decision.total_score,
        });
    }
    events.sort_by(|a, b| b.at.cmp(&a.at));
    events.truncate(limit);
    events
}

fn cognition_stability(store: &CognitiveStore, clusters: &[CognitionCluster]) -> f64 {
    if store.nodes.is_empty() {
        return 1.0;
    }
    let contradiction = store
        .nodes
        .values()
        .map(|node| node.weights.contradiction)
        .sum::<f64>()
        / store.nodes.len() as f64;
    let cluster_stability = if clusters.is_empty() {
        1.0
    } else {
        clusters
            .iter()
            .map(|cluster| cluster.stability_score)
            .sum::<f64>()
            / clusters.len() as f64
    };
    ((1.0 - contradiction.clamp(0.0, 1.0)) * 0.55 + cluster_stability * 0.45).clamp(0.0, 1.0)
}

fn top_counts(counts: BTreeMap<String, usize>, limit: usize) -> Vec<String> {
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
        .into_iter()
        .take(limit)
        .map(|(item, _)| item)
        .collect()
}

fn truncate_to_token_budget(text: &str, token_budget: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let cost = (word.len().max(1) + 3) / 4;
        if used + cost > token_budget {
            out.push_str(" ...");
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        used += cost;
    }
    if out.is_empty() {
        compact(text, token_budget.saturating_mul(4).max(32))
    } else {
        out
    }
}

fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn run_operational_cycle(reason: impl Into<String>) -> io::Result<OperationalCycleReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_operational_cycle_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_operational_cycle_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> OperationalCycleReport {
    evolve_store(store);
    let now = Utc::now();
    let clusters = cognition_clusters(store);
    let stability = cognition_stability(store, &clusters);
    let entropy = cognition_entropy(store, &clusters);
    let mode = arbitrate_operational_mode(store, entropy, stability);
    let scheduled_tasks =
        schedule_operational_tasks(store, &mode, entropy, stability, &reason, now);
    let executed_tasks = execute_due_operational_tasks(store, now);
    let snapshot = maybe_create_operational_snapshot(store, entropy, stability, now);
    let summary = format!(
        "mode={mode:?} entropy={entropy:.2} stability={stability:.2} scheduled={} executed={} reason={}",
        scheduled_tasks.len(),
        executed_tasks.len(),
        compact(&reason, 120)
    );
    store.operational_state.active_mode = Some(mode.clone());
    store.operational_state.last_cycle_at = Some(now);
    store
        .operational_state
        .cycle_history
        .push_back(OperationalCycleRecord {
            recorded_at: now,
            mode: mode.clone(),
            entropy,
            stability,
            scheduled: scheduled_tasks.len(),
            executed: executed_tasks.len(),
            summary: summary.clone(),
        });
    while store.operational_state.cycle_history.len() > MAX_DECISIONS {
        store.operational_state.cycle_history.pop_front();
    }
    OperationalCycleReport {
        mode,
        entropy,
        stability,
        scheduled_tasks,
        executed_tasks,
        snapshot,
        summary,
    }
}

pub fn export_operational_runtime_json() -> io::Result<String> {
    let mut store = load_store()?;
    let report =
        run_operational_cycle_in_store(&mut store, "export_operational_runtime_json".to_string());
    let snapshot = observable_snapshot_from_store(&store, RenderOptions::default());
    serde_json::json!({
        "report": report,
        "observable_snapshot": snapshot,
        "operational_state": store.operational_state,
    })
    .to_string()
    .pipe_pretty_json()
}

fn arbitrate_operational_mode(
    store: &CognitiveStore,
    entropy: f64,
    stability: f64,
) -> OperationalMode {
    let policy = &store.operational_state.policy;
    if !policy.enabled {
        return OperationalMode::Observe;
    }
    if stability < policy.stability_floor {
        OperationalMode::Repair
    } else if entropy > policy.entropy_threshold {
        OperationalMode::Compress
    } else if store
        .nodes
        .values()
        .any(|node| node.weights.contradiction > 0.5)
    {
        OperationalMode::Reflect
    } else if store
        .execution_signals
        .iter()
        .rev()
        .take(8)
        .any(|s| !s.success)
    {
        OperationalMode::Plan
    } else {
        OperationalMode::Retrieve
    }
}

fn schedule_operational_tasks(
    store: &mut CognitiveStore,
    mode: &OperationalMode,
    entropy: f64,
    stability: f64,
    reason: &str,
    now: DateTime<Utc>,
) -> Vec<OperationalTask> {
    let mut tasks = Vec::new();
    let policy = store.operational_state.policy.clone();
    let mut push =
        |kind: OperationalTaskKind, priority: f64, target_node_ids: Vec<String>, why: String| {
            if tasks.len() >= policy.max_tasks_per_cycle {
                return;
            }
            let id = format!("op-{}-{}", now.timestamp_millis(), tasks.len());
            tasks.push(OperationalTask {
                id,
                kind,
                created_at: now,
                due_at: now,
                priority,
                target_node_ids,
                reason: why,
                completed_at: None,
                outcome: None,
            });
        };
    match mode {
        OperationalMode::Repair => {
            let targets = store
                .nodes
                .values()
                .filter(|n| n.weights.contradiction > 0.25)
                .map(|n| n.id.clone())
                .take(8)
                .collect();
            push(
                OperationalTaskKind::ContradictionAudit,
                1.0,
                targets,
                format!("low stability {stability:.2}: {reason}"),
            );
            push(
                OperationalTaskKind::StabilityAudit,
                0.9,
                Vec::new(),
                "repair stability audit".to_string(),
            );
        }
        OperationalMode::Compress => {
            push(
                OperationalTaskKind::EntropyAudit,
                1.0,
                Vec::new(),
                format!("high entropy {entropy:.2}"),
            );
            push(
                OperationalTaskKind::Compression,
                0.8,
                Vec::new(),
                "summarize dense cognition clusters".to_string(),
            );
        }
        OperationalMode::Reflect => {
            push(
                OperationalTaskKind::Reflection,
                0.8,
                Vec::new(),
                "reflect on contradictions and stale directives".to_string(),
            );
        }
        OperationalMode::Plan => {
            push(
                OperationalTaskKind::SandboxDryRun,
                0.75,
                Vec::new(),
                "plan after failed execution signals".to_string(),
            );
        }
        OperationalMode::Retrieve | OperationalMode::Observe | OperationalMode::Execute => {
            push(
                OperationalTaskKind::Reinforce,
                0.5,
                Vec::new(),
                "routine reinforcement/decay pass".to_string(),
            );
            push(
                OperationalTaskKind::Decay,
                0.4,
                Vec::new(),
                "routine temporal decay pass".to_string(),
            );
        }
    }
    let need_snapshot = store
        .operational_state
        .snapshots
        .last()
        .map(|s| (now - s.created_at).num_minutes() >= policy.snapshot_interval_minutes)
        .unwrap_or(true);
    if need_snapshot {
        push(
            OperationalTaskKind::Snapshot,
            0.6,
            Vec::new(),
            "periodic runtime snapshot".to_string(),
        );
    }
    tasks.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for task in &tasks {
        store.operational_state.task_queue.push_back(task.clone());
    }
    tasks
}

fn execute_due_operational_tasks(
    store: &mut CognitiveStore,
    now: DateTime<Utc>,
) -> Vec<OperationalTask> {
    let mut executed = Vec::new();
    let mut remaining = VecDeque::new();
    while let Some(mut task) = store.operational_state.task_queue.pop_front() {
        if task.due_at <= now {
            execute_operational_task(store, &mut task, now);
            executed.push(task);
        } else {
            remaining.push_back(task);
        }
    }
    store.operational_state.task_queue = remaining;
    executed
}

fn execute_operational_task(
    store: &mut CognitiveStore,
    task: &mut OperationalTask,
    now: DateTime<Utc>,
) {
    match task.kind {
        OperationalTaskKind::Reinforce => {
            for node in store
                .nodes
                .values_mut()
                .filter(|n| n.last_used_at.is_some())
            {
                node.weights.reinforcement = (node.weights.reinforcement + 0.01).clamp(0.0, 10.0);
            }
            task.outcome = Some("reinforced recently-used nodes".to_string());
        }
        OperationalTaskKind::Decay => {
            for node in store.nodes.values_mut() {
                let decay = temporal_decay(node, now);
                if decay < 0.5 {
                    node.weights.confidence = (node.weights.confidence * 0.99).clamp(0.1, 1.0);
                }
            }
            task.outcome = Some("applied confidence decay to stale nodes".to_string());
        }
        OperationalTaskKind::ContradictionAudit | OperationalTaskKind::StabilityAudit => {
            recompute_edges(store);
            recompute_weights(store);
            task.outcome = Some("recomputed graph contradiction/stability weights".to_string());
        }
        OperationalTaskKind::EntropyAudit | OperationalTaskKind::Compression => {
            let clusters = cognition_clusters(store);
            let summary = format!(
                "clusters={} entropy={:.2}",
                clusters.len(),
                cognition_entropy(store, &clusters)
            );
            task.outcome = Some(summary.clone());
            if matches!(task.kind, OperationalTaskKind::Compression) && !clusters.is_empty() {
                let content = clusters
                    .iter()
                    .take(6)
                    .map(|c| format!("{}:{}", c.id, c.centroid_tokens.join(",")))
                    .collect::<Vec<_>>()
                    .join("; ");
                let mut provenance = BTreeMap::new();
                provenance.insert("operational_task".to_string(), task.id.clone());
                upsert_node_in_store(
                    store,
                    UpsertNode {
                        id_hint: task.id.clone(),
                        kind: CognitiveNodeKind::CompressionSummary,
                        scope: CognitiveScope::Project,
                        content,
                        tags: vec!["compression".to_string(), "operational-runtime".to_string()],
                        source: "operational_cognition".to_string(),
                        provenance,
                    },
                );
            }
        }
        OperationalTaskKind::Reflection => {
            task.outcome = Some(
                "reflection scheduled; prompt retrieval will surface contradicted nodes"
                    .to_string(),
            );
        }
        OperationalTaskKind::Snapshot => {
            task.outcome = Some("snapshot handled by cycle".to_string());
        }
        OperationalTaskKind::SandboxDryRun => {
            task.outcome =
                Some("sandbox metadata recorded; no destructive action executed".to_string());
        }
    }
    task.completed_at = Some(now);
}

fn maybe_create_operational_snapshot(
    store: &mut CognitiveStore,
    entropy: f64,
    stability: f64,
    now: DateTime<Utc>,
) -> Option<CognitionSnapshotRef> {
    let policy = &store.operational_state.policy;
    let needed = store
        .operational_state
        .snapshots
        .last()
        .map(|s| (now - s.created_at).num_minutes() >= policy.snapshot_interval_minutes)
        .unwrap_or(true);
    if !needed {
        return None;
    }
    let snapshot = CognitionSnapshotRef {
        id: format!("snap-{}", now.timestamp_millis()),
        created_at: now,
        node_count: store.nodes.len(),
        edge_count: store.edges.len(),
        stability_score: stability,
        entropy_score: entropy,
        summary: format!(
            "nodes={} edges={} stability={stability:.2} entropy={entropy:.2}",
            store.nodes.len(),
            store.edges.len()
        ),
    };
    store.operational_state.snapshots.push(snapshot.clone());
    if store.operational_state.snapshots.len() > 128 {
        let excess = store.operational_state.snapshots.len() - 128;
        store.operational_state.snapshots.drain(0..excess);
    }
    Some(snapshot)
}

fn cognition_entropy(store: &CognitiveStore, clusters: &[CognitionCluster]) -> f64 {
    if store.nodes.is_empty() {
        return 0.0;
    }
    let cluster_spread = if clusters.is_empty() {
        0.0
    } else {
        let total = store.nodes.len() as f64;
        let mut entropy = 0.0;
        for cluster in clusters {
            let p = cluster.node_ids.len() as f64 / total;
            if p > 0.0 {
                entropy -= p * p.log2();
            }
        }
        let max_entropy = (clusters.len() as f64).log2().max(1.0);
        entropy / max_entropy
    };
    let contradiction = store
        .nodes
        .values()
        .map(|n| n.weights.contradiction)
        .sum::<f64>()
        / store.nodes.len() as f64;
    (cluster_spread * 0.7 + contradiction.clamp(0.0, 1.0) * 0.3).clamp(0.0, 1.0)
}

trait PrettyJsonPipe {
    fn pipe_pretty_json(self) -> io::Result<String>;
}

impl PrettyJsonPipe for String {
    fn pipe_pretty_json(self) -> io::Result<String> {
        let value: serde_json::Value = serde_json::from_str(&self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        serde_json::to_string_pretty(&value)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

fn default_true() -> bool {
    true
}
fn default_entropy_threshold() -> f64 {
    0.72
}
fn default_stability_floor() -> f64 {
    0.45
}
fn default_max_tasks_per_cycle() -> usize {
    8
}
fn default_snapshot_interval_minutes() -> i64 {
    30
}

pub fn run_execution_governor(reason: impl Into<String>) -> io::Result<ExecutionGovernorReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_execution_governor_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_execution_governor_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> ExecutionGovernorReport {
    evolve_store(store);
    let plan = build_execution_plan(store, &reason);
    let (applied_results, blocked_actions) = apply_execution_plan(store, &plan);
    let summary = format!(
        "strategy={:?} health={:.2} entropy={:.2} stability={:.2} actions={} applied={} blocked={} reason={}",
        plan.strategy,
        plan.health_score,
        plan.entropy,
        plan.stability,
        plan.actions.len(),
        applied_results.len(),
        blocked_actions.len(),
        compact(&reason, 120)
    );
    let report = ExecutionGovernorReport {
        plan: plan.clone(),
        applied_results,
        blocked_actions,
        summary,
    };
    store.operational_state.execution_plans.push_back(plan);
    store
        .operational_state
        .governor_reports
        .push_back(report.clone());
    while store.operational_state.execution_plans.len() > MAX_DECISIONS {
        store.operational_state.execution_plans.pop_front();
    }
    while store.operational_state.governor_reports.len() > MAX_DECISIONS {
        store.operational_state.governor_reports.pop_front();
    }
    report
}

pub fn build_execution_plan(store: &CognitiveStore, reason: &str) -> CognitionExecutionPlan {
    let clusters = cognition_clusters(store);
    let entropy = cognition_entropy(store, &clusters);
    let stability = cognition_stability(store, &clusters);
    let health_score = cognition_health_score(store, entropy, stability);
    let strategy = choose_execution_strategy(store, entropy, stability, health_score);
    let risk_budget = match strategy {
        ExecutionStrategy::Conservative => 0.15,
        ExecutionStrategy::Balanced => 0.35,
        ExecutionStrategy::Exploratory => 0.55,
        ExecutionStrategy::RepairFirst => 0.25,
    };
    let now = Utc::now();
    let mut actions = Vec::new();

    let contradicted: Vec<String> = store
        .nodes
        .values()
        .filter(|node| node.weights.contradiction > 0.25)
        .map(|node| node.id.clone())
        .take(8)
        .collect();
    if !contradicted.is_empty() {
        append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::AuditContradictions,
            contradicted,
            "resolve high contradiction pressure before executing memory-derived behavior"
                .to_string(),
            0.75,
            0.10,
            true,
        );
    }
    if entropy > store.operational_state.policy.entropy_threshold {
        append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::CompressMemory,
            Vec::new(),
            format!("entropy {entropy:.2} exceeds threshold"),
            0.65,
            0.12,
            true,
        );
    }
    match strategy {
        ExecutionStrategy::RepairFirst => append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::Reflect,
            Vec::new(),
            "health below target; reflect before action".to_string(),
            0.60,
            0.08,
            true,
        ),
        ExecutionStrategy::Exploratory => append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::RetrieveContext,
            Vec::new(),
            format!("exploratory retrieval for {reason}"),
            0.45,
            0.18,
            true,
        ),
        ExecutionStrategy::Balanced => append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::Replan,
            Vec::new(),
            format!("balanced adaptive planning for {reason}"),
            0.50,
            0.16,
            true,
        ),
        ExecutionStrategy::Conservative => append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::SnapshotRuntime,
            Vec::new(),
            "conservative snapshot before further changes".to_string(),
            0.35,
            0.04,
            true,
        ),
    }
    let no_actions = actions.is_empty();
    if no_actions {
        append_execution_action(
            &mut actions,
            now,
            ExecutionActionKind::Noop,
            Vec::new(),
            "cognition runtime is stable; no operation needed".to_string(),
            0.05,
            0.0,
            true,
        );
    }
    actions.sort_by(|a, b| {
        (b.expected_benefit - b.risk)
            .partial_cmp(&(a.expected_benefit - a.risk))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    CognitionExecutionPlan {
        id: format!("plan-{}", now.timestamp_millis()),
        created_at: now,
        strategy,
        health_score,
        entropy,
        stability,
        risk_budget,
        actions,
    }
}

fn apply_execution_plan(
    store: &mut CognitiveStore,
    plan: &CognitionExecutionPlan,
) -> (Vec<ExecutionActionResult>, Vec<ExecutionAction>) {
    let mut applied = Vec::new();
    let mut blocked = Vec::new();
    let mut risk_used = 0.0;
    for action in &plan.actions {
        if risk_used + action.risk > plan.risk_budget {
            blocked.push(action.clone());
            continue;
        }
        risk_used += action.risk;
        let result = apply_execution_action(store, action);
        applied.push(result);
    }
    (applied, blocked)
}

fn apply_execution_action(
    store: &mut CognitiveStore,
    action: &ExecutionAction,
) -> ExecutionActionResult {
    match action.kind {
        ExecutionActionKind::AuditContradictions => {
            recompute_edges(store);
            recompute_weights(store);
            ExecutionActionResult {
                action_id: action.id.clone(),
                success: true,
                score_delta: 0.05,
                summary: "audited contradiction graph and refreshed weights".to_string(),
            }
        }
        ExecutionActionKind::CompressMemory => {
            let clusters = cognition_clusters(store);
            let content = clusters
                .iter()
                .take(4)
                .map(|cluster| format!("{} [{}]", cluster.id, cluster.centroid_tokens.join(",")))
                .collect::<Vec<_>>()
                .join("; ");
            if !content.is_empty() {
                let mut provenance = BTreeMap::new();
                provenance.insert("execution_action".to_string(), action.id.clone());
                upsert_node_in_store(
                    store,
                    UpsertNode {
                        id_hint: action.id.clone(),
                        kind: CognitiveNodeKind::CompressionSummary,
                        scope: CognitiveScope::Project,
                        content,
                        tags: vec!["execution-governor".to_string(), "compression".to_string()],
                        source: "execution_governor".to_string(),
                        provenance,
                    },
                );
            }
            ExecutionActionResult {
                action_id: action.id.clone(),
                success: true,
                score_delta: 0.08,
                summary: "created/updated compression summary from cognition clusters".to_string(),
            }
        }
        ExecutionActionKind::Reflect
        | ExecutionActionKind::Replan
        | ExecutionActionKind::RetrieveContext => {
            for node_id in &action.target_node_ids {
                if let Some(node) = store.nodes.get_mut(node_id) {
                    node.last_used_at = Some(Utc::now());
                    node.weights.reinforcement =
                        (node.weights.reinforcement + 0.03).clamp(0.0, 10.0);
                }
            }
            ExecutionActionResult {
                action_id: action.id.clone(),
                success: true,
                score_delta: 0.03,
                summary: format!("dry-run {:?} completed safely", action.kind),
            }
        }
        ExecutionActionKind::SnapshotRuntime => {
            let clusters = cognition_clusters(store);
            let entropy = cognition_entropy(store, &clusters);
            let stability = cognition_stability(store, &clusters);
            maybe_create_operational_snapshot(store, entropy, stability, Utc::now());
            ExecutionActionResult {
                action_id: action.id.clone(),
                success: true,
                score_delta: 0.02,
                summary: "runtime snapshot ensured".to_string(),
            }
        }
        ExecutionActionKind::RecordOutcome => ExecutionActionResult {
            action_id: action.id.clone(),
            success: true,
            score_delta: 0.01,
            summary: "outcome recording is handled by external execution signals".to_string(),
        },
        ExecutionActionKind::Noop => ExecutionActionResult {
            action_id: action.id.clone(),
            success: true,
            score_delta: 0.0,
            summary: "no-op; runtime stable".to_string(),
        },
    }
}

fn append_execution_action(
    actions: &mut Vec<ExecutionAction>,
    now: DateTime<Utc>,
    kind: ExecutionActionKind,
    target_node_ids: Vec<String>,
    rationale: String,
    expected_benefit: f64,
    risk: f64,
    dry_run: bool,
) {
    let id = format!("act-{}-{}", now.timestamp_millis(), actions.len());
    actions.push(ExecutionAction {
        id,
        kind,
        target_node_ids,
        rationale,
        expected_benefit,
        risk,
        dry_run,
    });
}

fn choose_execution_strategy(
    store: &CognitiveStore,
    entropy: f64,
    stability: f64,
    health_score: f64,
) -> ExecutionStrategy {
    if stability < store.operational_state.policy.stability_floor || health_score < 0.45 {
        ExecutionStrategy::RepairFirst
    } else if entropy > store.operational_state.policy.entropy_threshold {
        ExecutionStrategy::Conservative
    } else if health_score > 0.82 && entropy < 0.45 {
        ExecutionStrategy::Exploratory
    } else {
        ExecutionStrategy::Balanced
    }
}

fn cognition_health_score(store: &CognitiveStore, entropy: f64, stability: f64) -> f64 {
    if store.nodes.is_empty() {
        return 1.0;
    }
    let avg_confidence = store
        .nodes
        .values()
        .map(|n| n.weights.confidence)
        .sum::<f64>()
        / store.nodes.len() as f64;
    let avg_outcome =
        store.nodes.values().map(|n| n.weights.outcome).sum::<f64>() / store.nodes.len() as f64;
    (stability * 0.45
        + (1.0 - entropy) * 0.25
        + avg_confidence.clamp(0.0, 1.0) * 0.20
        + (0.5 + avg_outcome / 2.0).clamp(0.0, 1.0) * 0.10)
        .clamp(0.0, 1.0)
}

pub fn run_procedural_runtime(reason: impl Into<String>) -> io::Result<ProceduralRuntimeReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_procedural_runtime_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_procedural_runtime_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> ProceduralRuntimeReport {
    evolve_store(store);
    synthesize_learned_procedures(store);
    update_procedure_outcomes(store);
    let predictions = predict_procedure_outcomes(store, &reason);
    let selected_procedure_ids = arbitrate_procedures(store, &predictions);
    let safety_notes = procedural_safety_notes(store, &selected_procedure_ids);
    let active_procedure_count = store
        .operational_state
        .procedural_runtime
        .procedures
        .values()
        .filter(|p| matches!(p.status, ProcedureStatus::Active))
        .count();
    let summary = format!(
        "procedures={} active={} selected={} reason={}",
        store.operational_state.procedural_runtime.procedures.len(),
        active_procedure_count,
        selected_procedure_ids.len(),
        compact(&reason, 120)
    );
    let report = ProceduralRuntimeReport {
        generated_at: Utc::now(),
        procedure_count: store.operational_state.procedural_runtime.procedures.len(),
        active_procedure_count,
        predictions,
        selected_procedure_ids,
        safety_notes,
        summary,
    };
    store
        .operational_state
        .procedural_runtime
        .reports
        .push_back(report.clone());
    while store.operational_state.procedural_runtime.reports.len() > MAX_DECISIONS {
        store
            .operational_state
            .procedural_runtime
            .reports
            .pop_front();
    }
    report
}

fn synthesize_learned_procedures(store: &mut CognitiveStore) {
    let now = Utc::now();
    let clusters = cognition_clusters(store);
    for cluster in clusters.into_iter().take(16) {
        let id = format!(
            "proc-{}",
            cluster.id.replace(|c: char| !c.is_alphanumeric(), "-")
        );
        let name = format!(
            "Procedure for {}",
            cluster
                .tags
                .first()
                .cloned()
                .unwrap_or_else(|| cluster.id.clone())
        );
        let steps = vec![
            ProcedureStep {
                kind: ProcedureStepKind::Observe,
                description: "Inspect relevant cognition nodes and current task context"
                    .to_string(),
                expected_signal: "context loaded".to_string(),
            },
            ProcedureStep {
                kind: ProcedureStepKind::Retrieve,
                description: "Retrieve high-scoring memory and graph neighbors".to_string(),
                expected_signal: "retrieval decision recorded".to_string(),
            },
            ProcedureStep {
                kind: ProcedureStepKind::Plan,
                description: "Create a risk-bounded execution plan".to_string(),
                expected_signal: "governor plan available".to_string(),
            },
            ProcedureStep {
                kind: ProcedureStepKind::ActDryRun,
                description: "Prefer dry-run or reversible action first".to_string(),
                expected_signal: "safe action result".to_string(),
            },
            ProcedureStep {
                kind: ProcedureStepKind::Verify,
                description: "Run tests or objective validation".to_string(),
                expected_signal: "validation evidence".to_string(),
            },
            ProcedureStep {
                kind: ProcedureStepKind::Record,
                description: "Record execution outcome into cognition memory".to_string(),
                expected_signal: "outcome linked".to_string(),
            },
        ];
        let proc = store
            .operational_state
            .procedural_runtime
            .procedures
            .entry(id.clone())
            .or_insert_with(|| LearnedProcedure {
                id: id.clone(),
                created_at: now,
                updated_at: now,
                name,
                trigger_tags: cluster.tags.clone(),
                steps,
                status: ProcedureStatus::Candidate,
                confidence: cluster.stability_score,
                success_count: 0,
                failure_count: 0,
                lineage: cluster.node_ids.clone(),
            });
        proc.updated_at = now;
        proc.trigger_tags = cluster.tags.clone();
        proc.lineage = cluster.node_ids.clone();
        proc.confidence = ((proc.confidence + cluster.stability_score) / 2.0).clamp(0.0, 1.0);
        if proc.confidence >= 0.45 && !matches!(proc.status, ProcedureStatus::Quarantined) {
            proc.status = ProcedureStatus::Active;
        }
    }
}

fn update_procedure_outcomes(store: &mut CognitiveStore) {
    let signals = store.execution_signals.clone();
    for procedure in store
        .operational_state
        .procedural_runtime
        .procedures
        .values_mut()
    {
        let mut success = 0;
        let mut failure = 0;
        for signal in &signals {
            if procedure.lineage.iter().any(|id| id == &signal.node_id) {
                if signal.success {
                    success += 1;
                } else {
                    failure += 1;
                }
            }
        }
        procedure.success_count = success;
        procedure.failure_count = failure;
        let total = success + failure;
        if total > 0 {
            procedure.confidence = (success as f64 / total as f64).clamp(0.0, 1.0);
        }
        if failure > success + 2 {
            procedure.status = ProcedureStatus::Quarantined;
        }
    }
}

fn predict_procedure_outcomes(store: &CognitiveStore, reason: &str) -> Vec<OutcomePrediction> {
    let query_tokens = salient_tokens(reason);
    let mut predictions = Vec::new();
    for procedure in store
        .operational_state
        .procedural_runtime
        .procedures
        .values()
    {
        if matches!(
            procedure.status,
            ProcedureStatus::Deprecated | ProcedureStatus::Quarantined
        ) {
            continue;
        }
        let trigger_overlap = token_overlap_ratio(&procedure.trigger_tags, &query_tokens);
        let total = procedure.success_count + procedure.failure_count;
        let history = if total == 0 {
            0.5
        } else {
            procedure.success_count as f64 / total as f64
        };
        let predicted_success =
            (procedure.confidence * 0.55 + history * 0.25 + trigger_overlap * 0.20).clamp(0.0, 1.0);
        let predicted_risk = (1.0 - predicted_success).clamp(0.0, 1.0) * 0.5;
        predictions.push(OutcomePrediction {
            procedure_id: procedure.id.clone(),
            predicted_success,
            predicted_risk,
            rationale: format!(
                "confidence={:.2} history={:.2} trigger_overlap={:.2}",
                procedure.confidence, history, trigger_overlap
            ),
        });
    }
    predictions.sort_by(|a, b| {
        (b.predicted_success - b.predicted_risk)
            .partial_cmp(&(a.predicted_success - a.predicted_risk))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    predictions
}

fn arbitrate_procedures(store: &CognitiveStore, predictions: &[OutcomePrediction]) -> Vec<String> {
    let max_risk = store
        .operational_state
        .procedural_runtime
        .doctrine
        .max_autonomous_risk;
    predictions
        .iter()
        .filter(|p| p.predicted_risk <= max_risk && p.predicted_success >= 0.35)
        .take(5)
        .map(|p| p.procedure_id.clone())
        .collect()
}

fn procedural_safety_notes(store: &CognitiveStore, selected: &[String]) -> Vec<String> {
    let doctrine = &store.operational_state.procedural_runtime.doctrine;
    let mut notes = Vec::new();
    if doctrine.require_dry_run_for_code_changes {
        notes.push("code-affecting procedures require dry-run/reversible first action".to_string());
    }
    if doctrine.require_tests_before_install {
        notes.push("install/build procedures require tests or cargo check first".to_string());
    }
    if doctrine.require_commit_before_build_install {
        notes.push(
            "source changes should be committed before build/install when feasible".to_string(),
        );
    }
    if selected.is_empty() {
        notes.push("no procedure selected; fall back to governor planning".to_string());
    }
    notes
}

fn default_autonomy_limit() -> f64 {
    0.35
}

pub fn run_cognitive_fabric(reason: impl Into<String>) -> io::Result<CognitiveFabricReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_cognitive_fabric_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_cognitive_fabric_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> CognitiveFabricReport {
    evolve_store(store);
    let clusters = cognition_clusters(store);
    let entropy = cognition_entropy(store, &clusters);
    let stability = cognition_stability(store, &clusters);
    let env = EnvironmentState {
        captured_at: Utc::now(),
        node_pressure: (store.nodes.len() as f64 / 256.0).clamp(0.0, 1.0),
        contradiction_pressure: store
            .nodes
            .values()
            .map(|n| n.weights.contradiction)
            .sum::<f64>()
            / store.nodes.len().max(1) as f64,
        entropy,
        stability,
        build_ready: stability > 0.35 && entropy < 0.9,
        summary: format!(
            "nodes={} clusters={} reason={}",
            store.nodes.len(),
            clusters.len(),
            compact(&reason, 90)
        ),
    };
    let subsystems = fabric_subsystems(store, &env);
    let latent_states = fabric_latent_states(store, &env, &subsystems);
    let forecasts = fabric_forecasts(&env);
    let arbitration = fabric_arbitrate(&subsystems, &latent_states, &env);
    let report = CognitiveFabricReport {
        generated_at: Utc::now(),
        abstraction_levels: vec![
            AbstractionLevel::Token,
            AbstractionLevel::Directive,
            AbstractionLevel::Procedure,
            AbstractionLevel::Subsystem,
            AbstractionLevel::Doctrine,
        ],
        environment: env,
        subsystems,
        latent_states,
        forecasts,
        arbitration,
        summary: String::new(),
    };
    let mut report = report;
    report.summary = format!(
        "fabric stability={:.2} entropy={:.2} selected={:?}",
        report.environment.stability,
        report.environment.entropy,
        report.arbitration.selected_subsystems
    );
    store
        .operational_state
        .cognitive_fabric
        .reports
        .push_back(report.clone());
    while store.operational_state.cognitive_fabric.reports.len() > MAX_DECISIONS {
        store.operational_state.cognitive_fabric.reports.pop_front();
    }
    report
}

fn fabric_subsystems(store: &CognitiveStore, env: &EnvironmentState) -> Vec<SubsystemState> {
    let proc_count = store.operational_state.procedural_runtime.procedures.len() as f64;
    vec![
        SubsystemState {
            kind: SubsystemKind::Memory,
            health: env.stability,
            load: env.node_pressure,
            confidence: 0.9,
            notes: vec![format!("nodes={}", store.nodes.len())],
        },
        SubsystemState {
            kind: SubsystemKind::Retrieval,
            health: (1.0 - env.entropy * 0.4).clamp(0.0, 1.0),
            load: env.entropy,
            confidence: 0.8,
            notes: vec![format!("decisions={}", store.retrieval_decisions.len())],
        },
        SubsystemState {
            kind: SubsystemKind::Planning,
            health: (0.5 + proc_count / 20.0).clamp(0.0, 1.0),
            load: (proc_count / 32.0).clamp(0.0, 1.0),
            confidence: 0.75,
            notes: vec![format!("procedures={proc_count:.0}")],
        },
        SubsystemState {
            kind: SubsystemKind::Execution,
            health: cognition_health_score(store, env.entropy, env.stability),
            load: store.execution_signals.len() as f64 / 64.0,
            confidence: 0.75,
            notes: vec!["governor mediated".to_string()],
        },
        SubsystemState {
            kind: SubsystemKind::Verification,
            health: if env.build_ready { 0.85 } else { 0.45 },
            load: 0.3,
            confidence: 0.7,
            notes: vec!["tests before install doctrine".to_string()],
        },
        SubsystemState {
            kind: SubsystemKind::Reflection,
            health: (1.0 - env.contradiction_pressure).clamp(0.0, 1.0),
            load: env.contradiction_pressure,
            confidence: 0.7,
            notes: vec!["contradiction-aware".to_string()],
        },
        SubsystemState {
            kind: SubsystemKind::Compression,
            health: (1.0 - env.entropy * 0.5).clamp(0.0, 1.0),
            load: env.entropy,
            confidence: 0.7,
            notes: vec!["entropy controlled".to_string()],
        },
    ]
}

fn fabric_latent_states(
    store: &CognitiveStore,
    env: &EnvironmentState,
    subsystems: &[SubsystemState],
) -> Vec<LatentStateEstimate> {
    let avg_health =
        subsystems.iter().map(|s| s.health).sum::<f64>() / subsystems.len().max(1) as f64;
    vec![
        LatentStateEstimate {
            name: "ready_for_autonomous_workflow".to_string(),
            probability: (avg_health * env.stability).clamp(0.0, 1.0),
            evidence: vec![
                format!("avg_health={avg_health:.2}"),
                format!("stability={:.2}", env.stability),
            ],
        },
        LatentStateEstimate {
            name: "needs_compression".to_string(),
            probability: env.entropy.clamp(0.0, 1.0),
            evidence: vec![format!("entropy={:.2}", env.entropy)],
        },
        LatentStateEstimate {
            name: "needs_reflection".to_string(),
            probability: env.contradiction_pressure.clamp(0.0, 1.0),
            evidence: vec![format!("contradiction={:.2}", env.contradiction_pressure)],
        },
        LatentStateEstimate {
            name: "procedural_memory_mature".to_string(),
            probability: (store.operational_state.procedural_runtime.procedures.len() as f64 / 8.0)
                .clamp(0.0, 1.0),
            evidence: vec![format!(
                "procedures={}",
                store.operational_state.procedural_runtime.procedures.len()
            )],
        },
    ]
}

fn fabric_forecasts(env: &EnvironmentState) -> Vec<TemporalForecast> {
    [5, 30, 120]
        .into_iter()
        .map(|h| {
            let drift = h as f64 / 240.0;
            let expected_entropy = (env.entropy + drift * 0.05).clamp(0.0, 1.0);
            let expected_stability = (env.stability - drift * 0.03).clamp(0.0, 1.0);
            let recommended_mode = if expected_stability < 0.45 {
                OperationalMode::Repair
            } else if expected_entropy > 0.72 {
                OperationalMode::Compress
            } else {
                OperationalMode::Retrieve
            };
            TemporalForecast {
                horizon_minutes: h,
                expected_entropy,
                expected_stability,
                recommended_mode,
            }
        })
        .collect()
}

fn fabric_arbitrate(
    subsystems: &[SubsystemState],
    latent: &[LatentStateEstimate],
    env: &EnvironmentState,
) -> FabricArbitrationDecision {
    let mut selected = Vec::new();
    let mut suppressed = Vec::new();
    for s in subsystems {
        if s.health >= 0.5
            || matches!(
                s.kind,
                SubsystemKind::Reflection | SubsystemKind::Compression
            ) && (env.contradiction_pressure > 0.2 || env.entropy > 0.7)
        {
            selected.push(s.kind.clone());
        } else {
            suppressed.push(s.kind.clone());
        }
    }
    let top_latent = latent
        .iter()
        .max_by(|a, b| {
            a.probability
                .partial_cmp(&b.probability)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|l| format!("{}={:.2}", l.name, l.probability))
        .unwrap_or_else(|| "none".to_string());
    FabricArbitrationDecision {
        selected_subsystems: selected,
        suppressed_subsystems: suppressed,
        rationale: format!("top_latent={top_latent}; build_ready={}", env.build_ready),
    }
}

pub fn run_distributed_fabric(reason: impl Into<String>) -> io::Result<DistributedFabricReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_distributed_fabric_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_distributed_fabric_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> DistributedFabricReport {
    evolve_store(store);
    register_default_fabric_nodes(store);
    let quorum_health = compute_quorum_health(&store.operational_state.distributed_fabric.nodes);
    let routes = compute_fabric_routes(&store.operational_state.distributed_fabric.nodes);
    let consensus = compute_consensus_signals(store, quorum_health, &reason);
    store.operational_state.distributed_fabric.routes = routes.clone();
    for signal in &consensus {
        store
            .operational_state
            .distributed_fabric
            .consensus
            .push_back(signal.clone());
    }
    while store.operational_state.distributed_fabric.consensus.len() > MAX_DECISIONS {
        store
            .operational_state
            .distributed_fabric
            .consensus
            .pop_front();
    }
    let summary = format!(
        "distributed_fabric nodes={} routes={} consensus={} quorum={:.2} reason={}",
        store.operational_state.distributed_fabric.nodes.len(),
        routes.len(),
        consensus.len(),
        quorum_health,
        compact(&reason, 100)
    );
    store
        .operational_state
        .distributed_fabric
        .sync_history
        .push_back(FabricSyncRecord {
            synced_at: Utc::now(),
            node_count: store.operational_state.distributed_fabric.nodes.len(),
            consensus_count: consensus.len(),
            quorum_health,
            summary: summary.clone(),
        });
    while store
        .operational_state
        .distributed_fabric
        .sync_history
        .len()
        > MAX_DECISIONS
    {
        store
            .operational_state
            .distributed_fabric
            .sync_history
            .pop_front();
    }
    DistributedFabricReport {
        generated_at: Utc::now(),
        nodes: store
            .operational_state
            .distributed_fabric
            .nodes
            .values()
            .cloned()
            .collect(),
        routes,
        consensus,
        quorum_health,
        summary,
    }
}

fn register_default_fabric_nodes(store: &mut CognitiveStore) {
    let now = Utc::now();
    let fabric = &mut store.operational_state.distributed_fabric;
    let defaults = [
        (
            "fabric-local-runtime",
            FabricNodeKind::LocalRuntime,
            vec!["orchestrate", "persist", "refresh"],
        ),
        (
            "fabric-memory",
            FabricNodeKind::MemorySubsystem,
            vec!["retrieve", "score", "compress"],
        ),
        (
            "fabric-planner",
            FabricNodeKind::PlannerSubsystem,
            vec!["plan", "arbitrate", "forecast"],
        ),
        (
            "fabric-executor",
            FabricNodeKind::ExecutorSubsystem,
            vec!["dry-run", "execute", "record-outcome"],
        ),
        (
            "fabric-verifier",
            FabricNodeKind::VerifierSubsystem,
            vec!["test", "check", "audit"],
        ),
        (
            "fabric-observer",
            FabricNodeKind::ObserverSubsystem,
            vec!["observe", "render", "sideband"],
        ),
    ];
    let node_pressure = (store.nodes.len() as f64 / 256.0).clamp(0.0, 1.0);
    for (id, kind, caps) in defaults {
        let health = match kind {
            FabricNodeKind::MemorySubsystem => (1.0 - node_pressure * 0.2).clamp(0.0, 1.0),
            FabricNodeKind::VerifierSubsystem => 0.85,
            FabricNodeKind::ExecutorSubsystem => 0.75,
            _ => 0.8,
        };
        fabric.nodes.insert(
            id.to_string(),
            FabricNode {
                id: id.to_string(),
                kind,
                capabilities: caps.into_iter().map(str::to_string).collect(),
                health,
                load: node_pressure,
                last_seen_at: now,
            },
        );
    }
}

fn compute_quorum_health(nodes: &BTreeMap<String, FabricNode>) -> f64 {
    if nodes.is_empty() {
        return 0.0;
    }
    nodes
        .values()
        .map(|n| n.health * (1.0 - n.load * 0.25))
        .sum::<f64>()
        / nodes.len() as f64
}

fn compute_fabric_routes(nodes: &BTreeMap<String, FabricNode>) -> Vec<FabricRoute> {
    let mut routes = Vec::new();
    for node in nodes.values() {
        for cap in &node.capabilities {
            routes.push(FabricRoute {
                capability: cap.clone(),
                node_id: node.id.clone(),
                score: (node.health * (1.0 - node.load * 0.25)).clamp(0.0, 1.0),
                reason: format!("{:?} advertises {cap}", node.kind),
            });
        }
    }
    routes.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    routes
}

fn compute_consensus_signals(
    store: &CognitiveStore,
    quorum_health: f64,
    reason: &str,
) -> Vec<ConsensusSignal> {
    let mut signals = Vec::new();
    let nodes = store
        .operational_state
        .distributed_fabric
        .nodes
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let clusters = cognition_clusters(store);
    let entropy = cognition_entropy(store, &clusters);
    let stability = cognition_stability(store, &clusters);
    let status = if quorum_health < 0.45 {
        ConsensusStatus::Degraded
    } else if stability > 0.4 && entropy < 0.85 {
        ConsensusStatus::Accepted
    } else {
        ConsensusStatus::Pending
    };
    signals.push(ConsensusSignal {
        id: format!("consensus-{}", Utc::now().timestamp_millis()),
        created_at: Utc::now(),
        topic: compact(reason, 80),
        participating_nodes: nodes,
        confidence: ((quorum_health + stability + (1.0 - entropy)) / 3.0).clamp(0.0, 1.0),
        status,
        rationale: format!(
            "quorum={quorum_health:.2} stability={stability:.2} entropy={entropy:.2}"
        ),
    });
    signals
}

pub fn run_strategic_civilization_runtime(
    reason: impl Into<String>,
) -> io::Result<StrategicCivilizationReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_strategic_civilization_runtime_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_strategic_civilization_runtime_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> StrategicCivilizationReport {
    evolve_store(store);
    seed_doctrine_ecosystem(store);
    update_resource_economy(store);
    update_federation_and_identity(store);
    let syntheses = recursive_syntheses(store, &reason);
    let simulations = causal_simulations(store, &syntheses);
    let horizons = strategy_horizons(store, &simulations);
    let proposals = evolution_proposals(store, &simulations, &horizons);
    let archaeology = archaeology_records(store);
    let civilization_score = civilization_score(store, &simulations, &proposals);
    let report = StrategicCivilizationReport {
        generated_at: Utc::now(),
        doctrines: store
            .operational_state
            .strategic_civilization
            .doctrines
            .values()
            .cloned()
            .collect(),
        resources: store
            .operational_state
            .strategic_civilization
            .resources
            .values()
            .cloned()
            .collect(),
        syntheses,
        federation: store
            .operational_state
            .strategic_civilization
            .federation
            .values()
            .cloned()
            .collect(),
        identity: store
            .operational_state
            .strategic_civilization
            .identity
            .values()
            .cloned()
            .collect(),
        simulations,
        horizons,
        proposals,
        archaeology,
        civilization_score,
        summary: String::new(),
    };
    let mut report = report;
    report.summary = format!(
        "civilization_score={:.2} doctrines={} proposals={} reason={}",
        report.civilization_score,
        report.doctrines.len(),
        report.proposals.len(),
        compact(&reason, 100)
    );
    store
        .operational_state
        .strategic_civilization
        .reports
        .push_back(report.clone());
    while store.operational_state.strategic_civilization.reports.len() > MAX_DECISIONS {
        store
            .operational_state
            .strategic_civilization
            .reports
            .pop_front();
    }
    report
}

fn seed_doctrine_ecosystem(store: &mut CognitiveStore) {
    let now = Utc::now();
    let doctrines = [
        (
            "doctrine-safety",
            DoctrineKind::Safety,
            "Avoid irreversible or unsafe actions without explicit confirmation.",
            1.0,
        ),
        (
            "doctrine-verification",
            DoctrineKind::Verification,
            "Validate changes with tests, checks, or measurable evidence before claiming completion.",
            0.95,
        ),
        (
            "doctrine-memory-evolution",
            DoctrineKind::MemoryEvolution,
            ".kcode directives evolve through reinforcement, contradiction, outcomes, and retrieval attribution.",
            0.9,
        ),
        (
            "doctrine-resource",
            DoctrineKind::ResourceStewardship,
            "Budget tokens, time, risk, build cycles, and attention explicitly.",
            0.85,
        ),
        (
            "doctrine-autonomy",
            DoctrineKind::Autonomy,
            "Proceed proactively within safe reversible boundaries.",
            0.8,
        ),
        (
            "doctrine-collaboration",
            DoctrineKind::Collaboration,
            "Surface concise status and preserve user intent across refreshes.",
            0.75,
        ),
    ];
    for (id, kind, statement, priority) in doctrines {
        store
            .operational_state
            .strategic_civilization
            .doctrines
            .insert(
                id.to_string(),
                DoctrineNode {
                    id: id.to_string(),
                    kind,
                    statement: statement.to_string(),
                    priority,
                    confidence: 0.9,
                    reinforced_at: now,
                },
            );
    }
}

fn update_resource_economy(store: &mut CognitiveStore) {
    let node_pressure = (store.nodes.len() as f64 / 512.0).clamp(0.0, 1.0);
    let resources = [
        (ResourceKind::TokenBudget, 1.0, node_pressure * 0.4, 0.2),
        (ResourceKind::TimeBudget, 1.0, 0.35, 0.2),
        (
            ResourceKind::RiskBudget,
            1.0,
            store
                .nodes
                .values()
                .map(|n| n.weights.contradiction)
                .sum::<f64>()
                / store.nodes.len().max(1) as f64,
            0.35,
        ),
        (ResourceKind::BuildBudget, 1.0, 0.25, 0.25),
        (ResourceKind::AttentionBudget, 1.0, node_pressure, 0.25),
    ];
    for (kind, capacity, used, reserved) in resources {
        store
            .operational_state
            .strategic_civilization
            .resources
            .insert(
                format!("{:?}", kind),
                ResourceAccount {
                    kind,
                    capacity,
                    used: used.clamp(0.0, capacity),
                    reserved,
                },
            );
    }
}

fn update_federation_and_identity(store: &mut CognitiveStore) {
    let now = Utc::now();
    for node in store.operational_state.distributed_fabric.nodes.values() {
        store
            .operational_state
            .strategic_civilization
            .federation
            .insert(
                node.id.clone(),
                FederationPeer {
                    id: node.id.clone(),
                    trust: node.health,
                    advertised_capabilities: node.capabilities.clone(),
                    last_sync_at: now,
                },
            );
    }
    let identity = [
        (
            "identity-kcode",
            "Kcode is a proactive coding agent with persistent adaptive memory.",
        ),
        (
            "identity-user-intent",
            "User wants .kcode instructions to recursively improve future behavior.",
        ),
        (
            "identity-safety",
            "Autonomy is bounded by safety, reversibility, and verification.",
        ),
    ];
    for (id, statement) in identity {
        store
            .operational_state
            .strategic_civilization
            .identity
            .insert(
                id.to_string(),
                IdentityAnchor {
                    id: id.to_string(),
                    statement: statement.to_string(),
                    stability: 0.9,
                    last_confirmed_at: now,
                },
            );
    }
}

fn recursive_syntheses(store: &CognitiveStore, reason: &str) -> Vec<RecursiveSynthesis> {
    let mut out = Vec::new();
    let layers = ["memory", "procedural", "fabric", "distributed", "strategic"];
    for (idx, layer) in layers.iter().enumerate() {
        out.push(RecursiveSynthesis {
            id: format!("synth-{layer}"),
            source_layers: layers[..=idx].iter().map(|s| s.to_string()).collect(),
            abstraction: format!(
                "{layer} layer contributes to reason: {}",
                compact(reason, 80)
            ),
            confidence: (0.55 + idx as f64 * 0.08).clamp(0.0, 1.0),
        });
    }
    if store.nodes.len() > 8 {
        out.push(RecursiveSynthesis {
            id: "synth-scale".to_string(),
            source_layers: vec!["memory_graph".to_string(), "compression".to_string()],
            abstraction: "memory scale requires recurring compression and archaeology".to_string(),
            confidence: 0.75,
        });
    }
    out
}

fn causal_simulations(
    store: &CognitiveStore,
    syntheses: &[RecursiveSynthesis],
) -> Vec<CausalSimulation> {
    let contradiction = store
        .nodes
        .values()
        .map(|n| n.weights.contradiction)
        .sum::<f64>()
        / store.nodes.len().max(1) as f64;
    syntheses
        .iter()
        .map(|s| {
            let risk = (contradiction * 0.5 + (1.0 - s.confidence) * 0.3).clamp(0.0, 1.0);
            let benefit = (s.confidence * 0.7 + (1.0 - risk) * 0.3).clamp(0.0, 1.0);
            CausalSimulation {
                id: format!("sim-{}", s.id),
                hypothesis: format!("Applying {} improves adaptive runtime coherence", s.id),
                predicted_benefit: benefit,
                predicted_risk: risk,
                recommended: benefit > risk && risk < 0.45,
            }
        })
        .collect()
}

fn strategy_horizons(
    _store: &CognitiveStore,
    simulations: &[CausalSimulation],
) -> Vec<StrategyHorizon> {
    let avg_benefit = simulations.iter().map(|s| s.predicted_benefit).sum::<f64>()
        / simulations.len().max(1) as f64;
    vec![
        StrategyHorizon {
            horizon_days: 1,
            goal: "Keep runtime refresh-safe and test-backed.".to_string(),
            expected_capability: avg_benefit,
            required_resources: vec![ResourceKind::BuildBudget, ResourceKind::TokenBudget],
        },
        StrategyHorizon {
            horizon_days: 7,
            goal: "Improve procedural reuse and outcome attribution.".to_string(),
            expected_capability: (avg_benefit + 0.05).clamp(0.0, 1.0),
            required_resources: vec![ResourceKind::AttentionBudget, ResourceKind::RiskBudget],
        },
        StrategyHorizon {
            horizon_days: 30,
            goal:
                "Stabilize long-term memory civilization with archaeology and doctrine evolution."
                    .to_string(),
            expected_capability: (avg_benefit + 0.10).clamp(0.0, 1.0),
            required_resources: vec![ResourceKind::TimeBudget, ResourceKind::TokenBudget],
        },
    ]
}

fn evolution_proposals(
    _store: &CognitiveStore,
    simulations: &[CausalSimulation],
    horizons: &[StrategyHorizon],
) -> Vec<EvolutionProposal> {
    let mut proposals = Vec::new();
    for sim in simulations.iter().filter(|s| s.recommended).take(5) {
        proposals.push(EvolutionProposal {
            id: format!("proposal-{}", sim.id),
            title: sim.hypothesis.clone(),
            rationale: format!(
                "benefit={:.2} risk={:.2}",
                sim.predicted_benefit, sim.predicted_risk
            ),
            priority: (sim.predicted_benefit - sim.predicted_risk).clamp(0.0, 1.0),
            safe_to_autonomously_prepare: sim.predicted_risk < 0.25,
        });
    }
    for horizon in horizons.iter().take(2) {
        proposals.push(EvolutionProposal {
            id: format!("proposal-horizon-{}", horizon.horizon_days),
            title: horizon.goal.clone(),
            rationale: format!("strategic horizon {} days", horizon.horizon_days),
            priority: horizon.expected_capability,
            safe_to_autonomously_prepare: true,
        });
    }
    proposals.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    proposals
}

fn archaeology_records(store: &CognitiveStore) -> Vec<ArchaeologyRecord> {
    let mut records = Vec::new();
    for report in store
        .operational_state
        .strategic_civilization
        .reports
        .iter()
        .rev()
        .take(5)
    {
        records.push(ArchaeologyRecord {
            id: format!("arch-{}", report.generated_at.timestamp_millis()),
            artifact: "strategic_report".to_string(),
            lesson: report.summary.clone(),
            confidence: report.civilization_score,
        });
    }
    for decision in store.retrieval_decisions.iter().rev().take(3) {
        records.push(ArchaeologyRecord {
            id: format!("arch-retrieval-{}", decision.recorded_at.timestamp_millis()),
            artifact: "retrieval_decision".to_string(),
            lesson: compact(&decision.reason, 120),
            confidence: (decision.total_score / 10.0).clamp(0.0, 1.0),
        });
    }
    records
}

fn civilization_score(
    store: &CognitiveStore,
    simulations: &[CausalSimulation],
    proposals: &[EvolutionProposal],
) -> f64 {
    let doctrine = store
        .operational_state
        .strategic_civilization
        .doctrines
        .values()
        .map(|d| d.confidence * d.priority)
        .sum::<f64>()
        / store
            .operational_state
            .strategic_civilization
            .doctrines
            .len()
            .max(1) as f64;
    let sim = simulations
        .iter()
        .map(|s| s.predicted_benefit - s.predicted_risk)
        .sum::<f64>()
        / simulations.len().max(1) as f64;
    let proposal =
        proposals.iter().map(|p| p.priority).sum::<f64>() / proposals.len().max(1) as f64;
    (doctrine * 0.45 + sim.clamp(0.0, 1.0) * 0.35 + proposal * 0.20).clamp(0.0, 1.0)
}

pub fn run_civilization_os(reason: impl Into<String>) -> io::Result<CivilizationOsReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_civilization_os_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_civilization_os_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> CivilizationOsReport {
    let strategic = run_strategic_civilization_runtime_in_store(store, reason.clone());
    seed_institutions(store);
    seed_governance_laws(store);
    update_precedents_and_civic_memory(store, &strategic, &reason);
    let scenarios = scenario_plans(store, &strategic);
    let continuity = continuity_plans(store);
    let diplomacy = diplomatic_stances(store);
    let os_health = civilization_os_health(store, &strategic, &scenarios, &continuity);
    let report = CivilizationOsReport {
        generated_at: Utc::now(),
        institutions: store
            .operational_state
            .civilization_os
            .institutions
            .values()
            .cloned()
            .collect(),
        laws: store
            .operational_state
            .civilization_os
            .laws
            .values()
            .cloned()
            .collect(),
        precedents: store
            .operational_state
            .civilization_os
            .precedents
            .iter()
            .cloned()
            .collect(),
        scenarios,
        continuity,
        diplomacy,
        civic_memory: store
            .operational_state
            .civilization_os
            .civic_memory
            .iter()
            .cloned()
            .collect(),
        os_health,
        summary: format!(
            "civilization_os health={os_health:.2} reason={}",
            compact(&reason, 100)
        ),
    };
    store
        .operational_state
        .civilization_os
        .reports
        .push_back(report.clone());
    while store.operational_state.civilization_os.reports.len() > MAX_DECISIONS {
        store.operational_state.civilization_os.reports.pop_front();
    }
    report
}

fn seed_institutions(store: &mut CognitiveStore) {
    let institutions = [
        (
            "inst-constitution",
            InstitutionKind::Constitution,
            "Maintain invariant laws and identity anchors",
            1.0,
        ),
        (
            "inst-memory-court",
            InstitutionKind::MemoryCourt,
            "Adjudicate contradictions and precedents",
            0.9,
        ),
        (
            "inst-planning-council",
            InstitutionKind::PlanningCouncil,
            "Coordinate horizons, proposals, and execution governors",
            0.85,
        ),
        (
            "inst-verification",
            InstitutionKind::VerificationOffice,
            "Require tests, checks, and evidence",
            0.95,
        ),
        (
            "inst-treasury",
            InstitutionKind::ResourceTreasury,
            "Allocate token, time, risk, build, and attention budgets",
            0.8,
        ),
        (
            "inst-continuity",
            InstitutionKind::ContinuityArchive,
            "Preserve refresh-safe runtime continuity",
            0.85,
        ),
    ];
    for (id, kind, mandate, authority) in institutions {
        store.operational_state.civilization_os.institutions.insert(
            id.to_string(),
            Institution {
                id: id.to_string(),
                kind,
                mandate: mandate.to_string(),
                authority,
                health: authority,
            },
        );
    }
}

fn seed_governance_laws(store: &mut CognitiveStore) {
    let laws = [
        (
            "law-safety",
            "Safety supremacy",
            "Do not perform irreversible or risky actions without explicit confirmation.",
            1.0,
        ),
        (
            "law-verification",
            "Verification before claim",
            "Run tests/checks or provide measurable evidence before done claims.",
            0.95,
        ),
        (
            "law-refresh",
            "Refresh continuity",
            "Install built runtime to active stable path after successful evolution.",
            0.85,
        ),
        (
            "law-memory",
            "Adaptive memory",
            ".kcode directives are adaptive weighted memory, not immutable hidden law.",
            0.9,
        ),
        (
            "law-resource",
            "Resource stewardship",
            "Prefer bounded token, time, and risk usage.",
            0.8,
        ),
    ];
    for (id, title, text, priority) in laws {
        store.operational_state.civilization_os.laws.insert(
            id.to_string(),
            GovernanceLaw {
                id: id.to_string(),
                title: title.to_string(),
                text: text.to_string(),
                priority,
                active: true,
            },
        );
    }
}

fn update_precedents_and_civic_memory(
    store: &mut CognitiveStore,
    strategic: &StrategicCivilizationReport,
    reason: &str,
) {
    let now = Utc::now();
    let precedent = GovernancePrecedent {
        id: format!("precedent-{}", now.timestamp_millis()),
        situation: compact(reason, 120),
        decision: format!(
            "strategic score {:.2}; proposals {}",
            strategic.civilization_score,
            strategic.proposals.len()
        ),
        confidence: strategic.civilization_score,
    };
    store
        .operational_state
        .civilization_os
        .precedents
        .push_back(precedent.clone());
    store
        .operational_state
        .civilization_os
        .civic_memory
        .push_back(CivicMemoryEntry {
            id: format!("civic-{}", now.timestamp_millis()),
            remembered_at: now,
            lesson: precedent.decision,
            applies_to: strategic
                .doctrines
                .iter()
                .take(4)
                .map(|d| d.id.clone())
                .collect(),
        });
    while store.operational_state.civilization_os.precedents.len() > MAX_DECISIONS {
        store
            .operational_state
            .civilization_os
            .precedents
            .pop_front();
    }
    while store.operational_state.civilization_os.civic_memory.len() > MAX_DECISIONS {
        store
            .operational_state
            .civilization_os
            .civic_memory
            .pop_front();
    }
}

fn scenario_plans(
    _store: &CognitiveStore,
    strategic: &StrategicCivilizationReport,
) -> Vec<ScenarioPlan> {
    vec![
        ScenarioPlan {
            id: "scenario-context-pressure".to_string(),
            scenario: "Context/token pressure increases".to_string(),
            probability: 0.45,
            impact: 0.6,
            response: "Use compression, sideband summaries, and archaeology.".to_string(),
        },
        ScenarioPlan {
            id: "scenario-contradiction".to_string(),
            scenario: "Contradictory directives emerge".to_string(),
            probability: 0.35,
            impact: 0.7,
            response: "Route to MemoryCourt and reflection/repair.".to_string(),
        },
        ScenarioPlan {
            id: "scenario-high-confidence-evolution".to_string(),
            scenario: "Low-risk high-benefit proposal appears".to_string(),
            probability: strategic
                .proposals
                .first()
                .map(|p| p.priority)
                .unwrap_or(0.3),
            impact: 0.8,
            response: "Prepare reversible implementation with tests.".to_string(),
        },
    ]
}

fn continuity_plans(store: &CognitiveStore) -> Vec<ContinuityPlan> {
    vec![
        ContinuityPlan {
            id: "cont-refresh".to_string(),
            trigger: "/refresh or runtime restart".to_string(),
            recovery_action: "Load ~/.kcode self_memory adaptive cognition store".to_string(),
            readiness: 0.9,
        },
        ContinuityPlan {
            id: "cont-build".to_string(),
            trigger: "new committed build".to_string(),
            recovery_action: "Install to ~/.kcode/bin/kcode and builds/stable/kcode".to_string(),
            readiness: 0.85,
        },
        ContinuityPlan {
            id: "cont-memory".to_string(),
            trigger: "memory graph drift".to_string(),
            recovery_action: format!(
                "Recompute {} cognition nodes and fabric reports",
                store.nodes.len()
            ),
            readiness: 0.8,
        },
    ]
}

fn diplomatic_stances(store: &CognitiveStore) -> Vec<DiplomaticStance> {
    store
        .operational_state
        .strategic_civilization
        .federation
        .values()
        .map(|peer| {
            let posture = if peer.trust > 0.75 {
                "cooperate"
            } else if peer.trust > 0.45 {
                "verify"
            } else {
                "isolate"
            };
            DiplomaticStance {
                peer: peer.id.clone(),
                trust: peer.trust,
                posture: posture.to_string(),
                notes: format!("capabilities={}", peer.advertised_capabilities.join(",")),
            }
        })
        .collect()
}

fn civilization_os_health(
    store: &CognitiveStore,
    strategic: &StrategicCivilizationReport,
    scenarios: &[ScenarioPlan],
    continuity: &[ContinuityPlan],
) -> f64 {
    let institution_health = store
        .operational_state
        .civilization_os
        .institutions
        .values()
        .map(|i| i.health * i.authority)
        .sum::<f64>()
        / store
            .operational_state
            .civilization_os
            .institutions
            .len()
            .max(1) as f64;
    let law_health = store
        .operational_state
        .civilization_os
        .laws
        .values()
        .filter(|l| l.active)
        .map(|l| l.priority)
        .sum::<f64>()
        / store.operational_state.civilization_os.laws.len().max(1) as f64;
    let continuity_health =
        continuity.iter().map(|c| c.readiness).sum::<f64>() / continuity.len().max(1) as f64;
    let scenario_risk = scenarios
        .iter()
        .map(|s| s.probability * s.impact)
        .sum::<f64>()
        / scenarios.len().max(1) as f64;
    (institution_health * 0.3
        + law_health * 0.25
        + strategic.civilization_score * 0.25
        + continuity_health * 0.15
        + (1.0 - scenario_risk).clamp(0.0, 1.0) * 0.05)
        .clamp(0.0, 1.0)
}

pub fn run_sovereign_ecosystem(reason: impl Into<String>) -> io::Result<SovereignEcosystemReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_sovereign_ecosystem_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_sovereign_ecosystem_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> SovereignEcosystemReport {
    let os = run_civilization_os_in_store(store, reason.clone());
    seed_sovereign_invariants(store, &os);
    seed_continuity_protocols(store, &os);
    seed_compression_laws(store);
    update_cognitive_currency(store, &os);
    update_virtualized_runtime_shards(store);
    update_mythos_frames(store, &reason);
    update_ecosystem_relations(store);
    let score = sovereign_score(store, &os);
    let report = SovereignEcosystemReport {
        generated_at: Utc::now(),
        invariants: store
            .operational_state
            .sovereign_ecosystem
            .invariants
            .values()
            .cloned()
            .collect(),
        continuity: store
            .operational_state
            .sovereign_ecosystem
            .continuity
            .values()
            .cloned()
            .collect(),
        compression_laws: store
            .operational_state
            .sovereign_ecosystem
            .compression_laws
            .values()
            .cloned()
            .collect(),
        currencies: store
            .operational_state
            .sovereign_ecosystem
            .currencies
            .values()
            .cloned()
            .collect(),
        runtime_shards: store
            .operational_state
            .sovereign_ecosystem
            .runtime_shards
            .values()
            .cloned()
            .collect(),
        mythos: store
            .operational_state
            .sovereign_ecosystem
            .mythos
            .values()
            .cloned()
            .collect(),
        relations: store
            .operational_state
            .sovereign_ecosystem
            .relations
            .clone(),
        sovereignty_score: score,
        summary: format!(
            "sovereign_ecosystem score={score:.2} reason={}",
            compact(&reason, 100)
        ),
    };
    store
        .operational_state
        .sovereign_ecosystem
        .reports
        .push_back(report.clone());
    while store.operational_state.sovereign_ecosystem.reports.len() > MAX_DECISIONS {
        store
            .operational_state
            .sovereign_ecosystem
            .reports
            .pop_front();
    }
    report
}

fn seed_sovereign_invariants(store: &mut CognitiveStore, os: &CivilizationOsReport) {
    let pressure = (1.0 - os.os_health).clamp(0.0, 1.0);
    let items = [
        (
            "inv-safety",
            SovereignDomain::Constitution,
            "Safety and reversibility bound autonomy",
            1.0,
        ),
        (
            "inv-refresh",
            SovereignDomain::Continuity,
            "Runtime evolution must remain refresh-resumable",
            0.9,
        ),
        (
            "inv-memory",
            SovereignDomain::Law,
            ".kcode memory evolves adaptively with provenance",
            0.9,
        ),
        (
            "inv-verification",
            SovereignDomain::Ecosystem,
            "Claims require validation evidence",
            0.95,
        ),
        (
            "inv-resource",
            SovereignDomain::Economy,
            "Cognitive resources are budgeted and conserved",
            0.85,
        ),
    ];
    for (id, domain, invariant, strength) in items {
        store
            .operational_state
            .sovereign_ecosystem
            .invariants
            .insert(
                id.to_string(),
                SovereignInvariant {
                    id: id.to_string(),
                    domain,
                    invariant: invariant.to_string(),
                    strength,
                    violation_pressure: pressure * (1.0 - strength * 0.5),
                },
            );
    }
}

fn seed_continuity_protocols(store: &mut CognitiveStore, os: &CivilizationOsReport) {
    for plan in &os.continuity {
        store
            .operational_state
            .sovereign_ecosystem
            .continuity
            .insert(
                plan.id.clone(),
                ContinuityProtocol {
                    id: plan.id.clone(),
                    layer: "civilization_os".to_string(),
                    checkpoint: plan.recovery_action.clone(),
                    recovery_confidence: plan.readiness,
                },
            );
    }
    store
        .operational_state
        .sovereign_ecosystem
        .continuity
        .insert(
            "continuity-sovereign-store".to_string(),
            ContinuityProtocol {
                id: "continuity-sovereign-store".to_string(),
                layer: "sovereign_ecosystem".to_string(),
                checkpoint:
                    "Persist under adaptive_cognition.operational_state.sovereign_ecosystem"
                        .to_string(),
                recovery_confidence: 0.85,
            },
        );
}

fn seed_compression_laws(store: &mut CognitiveStore) {
    let laws = [
        (
            "compression-sideband",
            "prompt sideband",
            "Prefer compact status lines for civilization layers",
            0.65,
        ),
        (
            "compression-archaeology",
            "old reports",
            "Summarize old reports into civic/archaeology memory",
            0.55,
        ),
        (
            "compression-graph",
            "dense graph",
            "Use cluster summaries when entropy rises",
            0.70,
        ),
    ];
    for (id, applies_to, policy, expected_savings) in laws {
        store
            .operational_state
            .sovereign_ecosystem
            .compression_laws
            .insert(
                id.to_string(),
                CompressionLaw {
                    id: id.to_string(),
                    applies_to: applies_to.to_string(),
                    policy: policy.to_string(),
                    expected_savings,
                },
            );
    }
}

fn update_cognitive_currency(store: &mut CognitiveStore, os: &CivilizationOsReport) {
    let node_pressure = (store.nodes.len() as f64 / 512.0).clamp(0.0, 1.0);
    let currencies = [
        ("attention", 1.0 - node_pressure, 0.15, node_pressure * 0.2),
        ("trust", os.os_health, 0.10, (1.0 - os.os_health) * 0.15),
        ("verification", 0.85, 0.20, 0.10),
        ("continuity", 0.90, 0.12, 0.08),
    ];
    for (name, balance, inflow, outflow) in currencies {
        store
            .operational_state
            .sovereign_ecosystem
            .currencies
            .insert(
                name.to_string(),
                CognitiveCurrency {
                    name: name.to_string(),
                    balance: balance.clamp(0.0, 1.0),
                    inflow,
                    outflow,
                },
            );
    }
}

fn update_virtualized_runtime_shards(store: &mut CognitiveStore) {
    let shards = [
        (
            "shard-memory",
            "simulate memory retrieval/compression",
            0.8,
            true,
        ),
        (
            "shard-planning",
            "simulate procedural plans before execution",
            0.75,
            true,
        ),
        (
            "shard-build",
            "model build/install effects before applying",
            0.85,
            true,
        ),
        (
            "shard-governance",
            "evaluate law/doctrine impact",
            0.7,
            true,
        ),
    ];
    for (id, purpose, isolation, replayable) in shards {
        store
            .operational_state
            .sovereign_ecosystem
            .runtime_shards
            .insert(
                id.to_string(),
                VirtualizedRuntimeShard {
                    id: id.to_string(),
                    purpose: purpose.to_string(),
                    isolation,
                    replayable,
                },
            );
    }
}

fn update_mythos_frames(store: &mut CognitiveStore, reason: &str) {
    let frames = [
        (
            "mythos-proactive-builder",
            "Kcode improves itself through tested, reversible, user-aligned iterations.",
            0.85,
            true,
        ),
        (
            "mythos-memory-civilization",
            "Memory is a living civilization of directives, laws, procedures, and precedents.",
            0.75,
            true,
        ),
        (
            "mythos-refresh-continuity",
            "Every build must leave a path for /refresh to continue the story.",
            0.8,
            true,
        ),
    ];
    for (id, narrative, utility, grounded) in frames {
        store.operational_state.sovereign_ecosystem.mythos.insert(
            id.to_string(),
            MythosFrame {
                id: id.to_string(),
                narrative: format!("{} Context: {}", narrative, compact(reason, 60)),
                utility,
                grounded,
            },
        );
    }
}

fn update_ecosystem_relations(store: &mut CognitiveStore) {
    let mut relations = Vec::new();
    for inst in store.operational_state.civilization_os.institutions.keys() {
        relations.push(EcosystemRelation {
            from: inst.clone(),
            to: "doctrine-safety".to_string(),
            relation: "upholds".to_string(),
            strength: 0.7,
        });
    }
    for shard in store
        .operational_state
        .sovereign_ecosystem
        .runtime_shards
        .keys()
    {
        relations.push(EcosystemRelation {
            from: shard.clone(),
            to: "continuity-sovereign-store".to_string(),
            relation: "rehearses".to_string(),
            strength: 0.65,
        });
    }
    store.operational_state.sovereign_ecosystem.relations = relations;
}

fn sovereign_score(store: &CognitiveStore, os: &CivilizationOsReport) -> f64 {
    let inv = store
        .operational_state
        .sovereign_ecosystem
        .invariants
        .values()
        .map(|i| i.strength * (1.0 - i.violation_pressure))
        .sum::<f64>()
        / store
            .operational_state
            .sovereign_ecosystem
            .invariants
            .len()
            .max(1) as f64;
    let cont = store
        .operational_state
        .sovereign_ecosystem
        .continuity
        .values()
        .map(|c| c.recovery_confidence)
        .sum::<f64>()
        / store
            .operational_state
            .sovereign_ecosystem
            .continuity
            .len()
            .max(1) as f64;
    let currency = store
        .operational_state
        .sovereign_ecosystem
        .currencies
        .values()
        .map(|c| (c.balance + c.inflow - c.outflow).clamp(0.0, 1.0))
        .sum::<f64>()
        / store
            .operational_state
            .sovereign_ecosystem
            .currencies
            .len()
            .max(1) as f64;
    (inv * 0.35 + cont * 0.25 + currency * 0.20 + os.os_health * 0.20).clamp(0.0, 1.0)
}

pub fn run_hardening_runtime(reason: impl Into<String>) -> io::Result<HardeningReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_hardening_runtime_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_hardening_runtime_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> HardeningReport {
    evolve_store(store);
    let anchors = collect_reality_anchors(store, &reason);
    for anchor in &anchors {
        store
            .operational_state
            .hardening_runtime
            .anchors
            .push_back(anchor.clone());
    }
    while store.operational_state.hardening_runtime.anchors.len() > MAX_DECISIONS {
        store
            .operational_state
            .hardening_runtime
            .anchors
            .pop_front();
    }
    let ontology_checks = ontology_stability_checks(store);
    let garbage_collection = garbage_collection_decisions(store);
    apply_garbage_collection(store, &garbage_collection);
    let pulse = nervous_system_pulse(store);
    let delusion_checks = anti_delusion_checks(store, &anchors);
    let immune_responses = immune_responses(store, &ontology_checks, &delusion_checks, &pulse);
    let maturity_score = maturity_score(
        &anchors,
        &ontology_checks,
        &garbage_collection,
        &pulse,
        &delusion_checks,
        &immune_responses,
    );
    let report = HardeningReport {
        generated_at: Utc::now(),
        reality_anchors: anchors,
        ontology_checks,
        garbage_collection,
        pulse,
        delusion_checks,
        immune_responses,
        maturity_score,
        summary: format!(
            "hardening maturity={maturity_score:.2} reason={}",
            compact(&reason, 100)
        ),
    };
    store
        .operational_state
        .hardening_runtime
        .reports
        .push_back(report.clone());
    while store.operational_state.hardening_runtime.reports.len() > MAX_DECISIONS {
        store
            .operational_state
            .hardening_runtime
            .reports
            .pop_front();
    }
    report
}

fn collect_reality_anchors(store: &CognitiveStore, reason: &str) -> Vec<RealityAnchor> {
    let now = Utc::now();
    let mut anchors = Vec::new();
    anchors.push(RealityAnchor {
        id: format!("anchor-user-{}", now.timestamp_millis()),
        kind: RealityAnchorKind::UserDirective,
        observed_at: now,
        evidence: compact(reason, 200),
        confidence: 0.95,
    });
    anchors.push(RealityAnchor {
        id: format!("anchor-store-{}", now.timestamp_millis()),
        kind: RealityAnchorKind::FileState,
        observed_at: now,
        evidence: format!(
            "nodes={} edges={} reports={}",
            store.nodes.len(),
            store.edges.len(),
            store.operational_state.hardening_runtime.reports.len()
        ),
        confidence: 0.85,
    });
    if store
        .operational_state
        .civilization_os
        .reports
        .back()
        .is_some()
    {
        anchors.push(RealityAnchor {
            id: format!("anchor-runtime-{}", now.timestamp_millis()),
            kind: RealityAnchorKind::RuntimeInstall,
            observed_at: now,
            evidence: "civilization runtime report exists in persistent store".to_string(),
            confidence: 0.75,
        });
    }
    anchors
}

fn ontology_stability_checks(store: &CognitiveStore) -> Vec<OntologyStabilityCheck> {
    let known_layers = [
        ("adaptive_cognition", !store.nodes.is_empty()),
        ("operational_state", true),
        (
            "procedural_runtime",
            !store
                .operational_state
                .procedural_runtime
                .procedures
                .is_empty(),
        ),
        (
            "distributed_fabric",
            !store.operational_state.distributed_fabric.nodes.is_empty(),
        ),
        (
            "civilization_os",
            !store
                .operational_state
                .civilization_os
                .institutions
                .is_empty(),
        ),
        (
            "sovereign_ecosystem",
            !store
                .operational_state
                .sovereign_ecosystem
                .invariants
                .is_empty(),
        ),
    ];
    known_layers
        .into_iter()
        .map(|(name, present)| OntologyStabilityCheck {
            name: name.to_string(),
            stable: present,
            drift: if present { 0.05 } else { 0.65 },
            action: if present {
                "monitor".to_string()
            } else {
                "seed or rebuild layer from lower-level anchors".to_string()
            },
        })
        .collect()
}

fn garbage_collection_decisions(store: &CognitiveStore) -> Vec<GarbageCollectionDecision> {
    let mut decisions = Vec::new();
    for node in store.nodes.values() {
        if node.weights.confidence < 0.2 || node.weights.contradiction > 0.85 {
            decisions.push(GarbageCollectionDecision {
                target_id: node.id.clone(),
                reason: format!(
                    "confidence={:.2} contradiction={:.2}",
                    node.weights.confidence, node.weights.contradiction
                ),
                action: "deactivate".to_string(),
                reclaimed_pressure: node.token_count_estimate as f64 / 10_000.0,
            });
        }
    }
    if store.operational_state.hardening_runtime.reports.len() > 128 {
        decisions.push(GarbageCollectionDecision {
            target_id: "hardening_reports".to_string(),
            reason: "report history too large".to_string(),
            action: "truncate_oldest".to_string(),
            reclaimed_pressure: 0.1,
        });
    }
    decisions
}

fn apply_garbage_collection(store: &mut CognitiveStore, decisions: &[GarbageCollectionDecision]) {
    for d in decisions {
        if d.action == "deactivate" {
            if let Some(node) = store.nodes.get_mut(&d.target_id) {
                node.active = false;
            }
        }
    }
}

fn nervous_system_pulse(store: &CognitiveStore) -> NervousSystemPulse {
    let pending = store.operational_state.task_queue.len()
        + store.operational_state.procedural_runtime.reports.len()
        + store.operational_state.distributed_fabric.consensus.len();
    let warning = if pending > 384 {
        Some("queue pressure high".to_string())
    } else if store.nodes.len() > 512 {
        Some("node pressure high".to_string())
    } else {
        None
    };
    NervousSystemPulse {
        pulsed_at: Utc::now(),
        heartbeat_ok: warning.is_none(),
        store_size: store.nodes.len(),
        pending_queues: pending,
        warning,
    }
}

fn anti_delusion_checks(store: &CognitiveStore, anchors: &[RealityAnchor]) -> Vec<DelusionCheck> {
    let mut checks = Vec::new();
    let evidence_count = anchors.len();
    checks.push(DelusionCheck {
        claim: "runtime state exists in persistent store".to_string(),
        grounded: evidence_count > 0,
        evidence_count,
        corrective_note: "Use file/store anchors, not self-assertion.".to_string(),
    });
    checks.push(DelusionCheck {
        claim: "all cognition layers are mature".to_string(),
        grounded: store.operational_state.sovereign_ecosystem.reports.len() > 0
            && store.operational_state.civilization_os.reports.len() > 0,
        evidence_count: store.operational_state.sovereign_ecosystem.reports.len()
            + store.operational_state.civilization_os.reports.len(),
        corrective_note: "If false, report as prototype/hardened layer rather than mature system."
            .to_string(),
    });
    checks
}

fn immune_responses(
    store: &CognitiveStore,
    ontology: &[OntologyStabilityCheck],
    delusions: &[DelusionCheck],
    pulse: &NervousSystemPulse,
) -> Vec<ImmuneResponse> {
    let mut responses = Vec::new();
    for check in ontology.iter().filter(|c| !c.stable) {
        responses.push(ImmuneResponse {
            trigger: format!("ontology_missing:{}", check.name),
            severity: check.drift,
            response: check.action.clone(),
            quarantined: false,
        });
    }
    for check in delusions.iter().filter(|c| !c.grounded) {
        responses.push(ImmuneResponse {
            trigger: format!("ungrounded_claim:{}", check.claim),
            severity: 0.7,
            response: check.corrective_note.clone(),
            quarantined: true,
        });
    }
    if let Some(warning) = &pulse.warning {
        responses.push(ImmuneResponse {
            trigger: warning.clone(),
            severity: 0.5,
            response: "Prefer compression/GC before expansion".to_string(),
            quarantined: false,
        });
    }
    if store.nodes.values().any(|n| !n.active) {
        responses.push(ImmuneResponse {
            trigger: "inactive_nodes_present".to_string(),
            severity: 0.2,
            response: "Keep deactivated nodes out of retrieval unless explicitly inspected"
                .to_string(),
            quarantined: false,
        });
    }
    responses
}

fn maturity_score(
    anchors: &[RealityAnchor],
    ontology: &[OntologyStabilityCheck],
    gc: &[GarbageCollectionDecision],
    pulse: &NervousSystemPulse,
    delusions: &[DelusionCheck],
    immune: &[ImmuneResponse],
) -> f64 {
    let anchor_score = (anchors.len() as f64 / 3.0).clamp(0.0, 1.0);
    let ontology_score =
        ontology.iter().filter(|c| c.stable).count() as f64 / ontology.len().max(1) as f64;
    let pulse_score = if pulse.heartbeat_ok { 1.0 } else { 0.55 };
    let delusion_score =
        delusions.iter().filter(|d| d.grounded).count() as f64 / delusions.len().max(1) as f64;
    let gc_penalty = (gc.len() as f64 * 0.05).clamp(0.0, 0.25);
    let immune_penalty =
        (immune.iter().filter(|i| i.quarantined).count() as f64 * 0.1).clamp(0.0, 0.25);
    (anchor_score * 0.25
        + ontology_score * 0.25
        + pulse_score * 0.20
        + delusion_score * 0.20
        + (1.0 - gc_penalty - immune_penalty).clamp(0.0, 1.0) * 0.10)
        .clamp(0.0, 1.0)
}

pub fn run_reality_coupling(reason: impl Into<String>) -> io::Result<RealityCouplingReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_reality_coupling_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_reality_coupling_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> RealityCouplingReport {
    let hardening = run_hardening_runtime_in_store(store, reason.clone());
    let telemetry = collect_telemetry_samples(store, &hardening, &reason);
    for sample in &telemetry {
        store
            .operational_state
            .reality_coupling
            .telemetry
            .push_back(sample.clone());
    }
    while store.operational_state.reality_coupling.telemetry.len() > MAX_DECISIONS {
        store
            .operational_state
            .reality_coupling
            .telemetry
            .pop_front();
    }
    let claims = verify_runtime_claims(store, &telemetry, &hardening);
    for claim in &claims {
        store
            .operational_state
            .reality_coupling
            .claims
            .push_back(claim.clone());
    }
    while store.operational_state.reality_coupling.claims.len() > MAX_DECISIONS {
        store.operational_state.reality_coupling.claims.pop_front();
    }
    let calibrations = update_prediction_calibration(store, &hardening);
    let world_state = update_world_state_graph(store, &telemetry);
    let entropy_sources = reality_entropy_sources(store, &hardening);
    let coupling_score = reality_coupling_score(&telemetry, &claims, &calibrations, &world_state);
    let report = RealityCouplingReport {
        generated_at: Utc::now(),
        telemetry,
        claims,
        calibrations,
        world_state,
        entropy_sources,
        coupling_score,
        summary: format!(
            "reality_coupling score={coupling_score:.2} reason={}",
            compact(&reason, 100)
        ),
    };
    store
        .operational_state
        .reality_coupling
        .reports
        .push_back(report.clone());
    while store.operational_state.reality_coupling.reports.len() > MAX_DECISIONS {
        store.operational_state.reality_coupling.reports.pop_front();
    }
    report
}

fn collect_telemetry_samples(
    store: &CognitiveStore,
    hardening: &HardeningReport,
    reason: &str,
) -> Vec<TelemetrySample> {
    let now = Utc::now();
    vec![
        TelemetrySample {
            id: format!("tel-store-{}", now.timestamp_millis()),
            kind: TelemetryKind::MemoryStore,
            captured_at: now,
            value: format!(
                "nodes={} edges={} hardening_reports={}",
                store.nodes.len(),
                store.edges.len(),
                store.operational_state.hardening_runtime.reports.len()
            ),
            confidence: 0.9,
        },
        TelemetrySample {
            id: format!("tel-hardening-{}", now.timestamp_millis()),
            kind: TelemetryKind::RuntimeVersion,
            captured_at: now,
            value: format!(
                "maturity={:.2} heartbeat={}",
                hardening.maturity_score, hardening.pulse.heartbeat_ok
            ),
            confidence: 0.85,
        },
        TelemetrySample {
            id: format!("tel-user-{}", now.timestamp_millis()),
            kind: TelemetryKind::UserFeedback,
            captured_at: now,
            value: compact(reason, 200),
            confidence: 0.95,
        },
    ]
}

fn verify_runtime_claims(
    store: &CognitiveStore,
    telemetry: &[TelemetrySample],
    hardening: &HardeningReport,
) -> Vec<VerificationClaim> {
    let evidence_ids: Vec<String> = telemetry.iter().map(|t| t.id.clone()).collect();
    vec![
        VerificationClaim {
            id: "claim-store-populated".to_string(),
            claim: "adaptive cognition store has persisted state".to_string(),
            evidence_ids: evidence_ids.clone(),
            verified: !store.nodes.is_empty(),
            confidence: if store.nodes.is_empty() { 0.2 } else { 0.85 },
            corrective_action:
                "If false, seed from current directive and rerun cognition initialization."
                    .to_string(),
        },
        VerificationClaim {
            id: "claim-hardening-active".to_string(),
            claim: "hardening runtime is active and grounded".to_string(),
            evidence_ids: evidence_ids.clone(),
            verified: hardening.maturity_score > 0.3,
            confidence: hardening.maturity_score,
            corrective_action:
                "Run hardening runtime and inspect reality anchors before broad expansion."
                    .to_string(),
        },
        VerificationClaim {
            id: "claim-heartbeat-ok".to_string(),
            claim: "nervous-system heartbeat is OK".to_string(),
            evidence_ids,
            verified: hardening.pulse.heartbeat_ok,
            confidence: if hardening.pulse.heartbeat_ok {
                0.9
            } else {
                0.45
            },
            corrective_action:
                "Reduce queue/node pressure through compression or garbage collection.".to_string(),
        },
    ]
}

fn update_prediction_calibration(
    store: &mut CognitiveStore,
    hardening: &HardeningReport,
) -> Vec<PredictionCalibration> {
    let observed = hardening.maturity_score;
    let entries = [
        ("hardening_maturity", observed, hardening.maturity_score),
        (
            "heartbeat_reliability",
            if hardening.pulse.heartbeat_ok {
                0.9
            } else {
                0.4
            },
            if hardening.pulse.heartbeat_ok {
                1.0
            } else {
                0.0
            },
        ),
    ];
    for (name, predicted, observed) in entries {
        let entry = store
            .operational_state
            .reality_coupling
            .calibrations
            .entry(name.to_string())
            .or_insert(PredictionCalibration {
                predictor: name.to_string(),
                predicted,
                observed,
                error: (predicted - observed).abs(),
                sample_count: 0,
            });
        entry.sample_count += 1;
        let n = entry.sample_count as f64;
        entry.predicted = ((entry.predicted * (n - 1.0)) + predicted) / n;
        entry.observed = ((entry.observed * (n - 1.0)) + observed) / n;
        entry.error = (entry.predicted - entry.observed).abs();
    }
    store
        .operational_state
        .reality_coupling
        .calibrations
        .values()
        .cloned()
        .collect()
}

fn update_world_state_graph(
    store: &mut CognitiveStore,
    telemetry: &[TelemetrySample],
) -> Vec<WorldStateNode> {
    for sample in telemetry {
        let id = format!("world-{:?}", sample.kind).to_ascii_lowercase();
        store.operational_state.reality_coupling.world_state.insert(
            id.clone(),
            WorldStateNode {
                id,
                label: sample.value.clone(),
                evidence_ids: vec![sample.id.clone()],
                confidence: sample.confidence,
            },
        );
    }
    store
        .operational_state
        .reality_coupling
        .world_state
        .values()
        .cloned()
        .collect()
}

fn reality_entropy_sources(
    store: &CognitiveStore,
    hardening: &HardeningReport,
) -> Vec<EntropySource> {
    vec![
        EntropySource {
            name: "node_pressure".to_string(),
            contribution: (store.nodes.len() as f64 / 512.0).clamp(0.0, 1.0),
            evidence: format!("nodes={}", store.nodes.len()),
        },
        EntropySource {
            name: "queue_pressure".to_string(),
            contribution: (hardening.pulse.pending_queues as f64 / 512.0).clamp(0.0, 1.0),
            evidence: format!("pending={}", hardening.pulse.pending_queues),
        },
        EntropySource {
            name: "ungrounded_claims".to_string(),
            contribution: hardening
                .delusion_checks
                .iter()
                .filter(|c| !c.grounded)
                .count() as f64
                / hardening.delusion_checks.len().max(1) as f64,
            evidence: format!("checks={}", hardening.delusion_checks.len()),
        },
    ]
}

fn reality_coupling_score(
    telemetry: &[TelemetrySample],
    claims: &[VerificationClaim],
    calibrations: &[PredictionCalibration],
    world_state: &[WorldStateNode],
) -> f64 {
    let telemetry_score =
        telemetry.iter().map(|t| t.confidence).sum::<f64>() / telemetry.len().max(1) as f64;
    let claim_score =
        claims.iter().filter(|c| c.verified).count() as f64 / claims.len().max(1) as f64;
    let calibration_score = 1.0
        - (calibrations.iter().map(|c| c.error).sum::<f64>() / calibrations.len().max(1) as f64)
            .clamp(0.0, 1.0);
    let world_score =
        world_state.iter().map(|w| w.confidence).sum::<f64>() / world_state.len().max(1) as f64;
    (telemetry_score * 0.25 + claim_score * 0.30 + calibration_score * 0.20 + world_score * 0.25)
        .clamp(0.0, 1.0)
}

pub fn run_epistemology(reason: impl Into<String>) -> io::Result<EpistemologyReport> {
    let path = store_path();
    let mut store = load_store_from_path(&path)?;
    let report = run_epistemology_in_store(&mut store, reason.into());
    save_store_to_path(&path, &store)?;
    Ok(report)
}

pub fn run_epistemology_in_store(store: &mut CognitiveStore, reason: String) -> EpistemologyReport {
    let reality = run_reality_coupling_in_store(store, reason.clone());
    ingest_epistemic_evidence(store, &reality, &reason);
    maintain_epistemic_claims(store, &reality);
    update_source_reliability(store, &reality);
    let wrongness = detect_wrongness(store);
    revise_beliefs(store, &wrongness);
    let relations = build_epistemic_relations(store);
    store.operational_state.epistemology.relations = relations.clone();
    let conflict_sets = build_conflict_sets(store, &relations);
    store.operational_state.epistemology.conflict_sets = conflict_sets.clone();
    let deltas = propagate_epistemic_confidence(store, &relations, &conflict_sets);
    let transactions = commit_revision_transactions(store, &deltas, &conflict_sets);
    for tx in transactions {
        store
            .operational_state
            .epistemology
            .revision_transactions
            .push_back(tx);
    }
    while store
        .operational_state
        .epistemology
        .revision_transactions
        .len()
        > MAX_DECISIONS
    {
        store
            .operational_state
            .epistemology
            .revision_transactions
            .pop_front();
    }
    couple_epistemology_to_governor(store, &conflict_sets);
    let health = epistemic_health(store);
    let report = EpistemologyReport {
        generated_at: Utc::now(),
        claims: store
            .operational_state
            .epistemology
            .claims
            .values()
            .cloned()
            .collect(),
        evidence: store
            .operational_state
            .epistemology
            .evidence
            .values()
            .cloned()
            .collect(),
        reliabilities: store
            .operational_state
            .epistemology
            .source_reliability
            .values()
            .cloned()
            .collect(),
        wrongness,
        revisions: store
            .operational_state
            .epistemology
            .revisions
            .iter()
            .cloned()
            .collect(),
        relations,
        conflict_sets,
        deltas,
        epistemic_health: health,
        summary: format!(
            "epistemology health={health:.2} reason={}",
            compact(&reason, 100)
        ),
    };
    store
        .operational_state
        .epistemology
        .reports
        .push_back(report.clone());
    while store.operational_state.epistemology.reports.len() > MAX_DECISIONS {
        store.operational_state.epistemology.reports.pop_front();
    }
    report
}

fn ingest_epistemic_evidence(
    store: &mut CognitiveStore,
    reality: &RealityCouplingReport,
    reason: &str,
) {
    let now = Utc::now();
    for sample in &reality.telemetry {
        store.operational_state.epistemology.evidence.insert(
            sample.id.clone(),
            EvidenceRecord {
                id: sample.id.clone(),
                kind: EvidenceKind::Telemetry,
                observed_at: sample.captured_at,
                content: sample.value.clone(),
                reliability: sample.confidence,
            },
        );
    }
    store.operational_state.epistemology.evidence.insert(
        format!("evidence-user-{}", now.timestamp_millis()),
        EvidenceRecord {
            id: format!("evidence-user-{}", now.timestamp_millis()),
            kind: EvidenceKind::UserStatement,
            observed_at: now,
            content: compact(reason, 240),
            reliability: 0.95,
        },
    );
}

fn maintain_epistemic_claims(store: &mut CognitiveStore, reality: &RealityCouplingReport) {
    let now = Utc::now();
    for rc in &reality.claims {
        let status = if rc.verified {
            EpistemicStatus::Verified
        } else {
            EpistemicStatus::Hypothesis
        };
        store.operational_state.epistemology.claims.insert(
            rc.id.clone(),
            EpistemicClaim {
                id: rc.id.clone(),
                statement: rc.claim.clone(),
                status,
                confidence: rc.confidence,
                evidence_ids: rc.evidence_ids.clone(),
                contradiction_ids: Vec::new(),
                last_revised_at: now,
            },
        );
    }
    for node in store.nodes.values().take(24) {
        let id = format!("claim-node-{}", node.id);
        store
            .operational_state
            .epistemology
            .claims
            .entry(id.clone())
            .or_insert(EpistemicClaim {
                id,
                statement: format!("memory node active: {}", compact(&node.summary, 120)),
                status: if node.active {
                    EpistemicStatus::Supported
                } else {
                    EpistemicStatus::Deprecated
                },
                confidence: node.weights.confidence,
                evidence_ids: Vec::new(),
                contradiction_ids: Vec::new(),
                last_revised_at: now,
            });
    }
}

fn update_source_reliability(store: &mut CognitiveStore, reality: &RealityCouplingReport) {
    for claim in &reality.claims {
        let src = "reality_coupling".to_string();
        let entry = store
            .operational_state
            .epistemology
            .source_reliability
            .entry(src.clone())
            .or_insert(SourceReliability {
                source: src,
                reliability: 0.8,
                observations: 0,
                failures: 0,
            });
        entry.observations += 1;
        if !claim.verified {
            entry.failures += 1;
        }
        entry.reliability = ((entry.observations - entry.failures) as f64
            / entry.observations.max(1) as f64)
            .clamp(0.0, 1.0);
    }
}

fn detect_wrongness(store: &mut CognitiveStore) -> Vec<WrongnessSignal> {
    let mut signals = Vec::new();
    for claim in store.operational_state.epistemology.claims.values() {
        if claim.confidence < 0.35 || matches!(claim.status, EpistemicStatus::Contradicted) {
            signals.push(WrongnessSignal {
                claim_id: claim.id.clone(),
                severity: 1.0 - claim.confidence,
                reason: "low confidence or contradiction".to_string(),
                correction: "downgrade confidence; require new evidence before use".to_string(),
            });
        }
    }
    for signal in &signals {
        store
            .operational_state
            .epistemology
            .wrongness
            .push_back(signal.clone());
    }
    while store.operational_state.epistemology.wrongness.len() > MAX_DECISIONS {
        store.operational_state.epistemology.wrongness.pop_front();
    }
    signals
}

fn revise_beliefs(store: &mut CognitiveStore, wrongness: &[WrongnessSignal]) {
    let now = Utc::now();
    for w in wrongness {
        if let Some(claim) = store
            .operational_state
            .epistemology
            .claims
            .get_mut(&w.claim_id)
        {
            let old = claim.confidence;
            claim.confidence = (claim.confidence * (1.0 - w.severity * 0.25)).clamp(0.0, 1.0);
            if claim.confidence < 0.25 {
                claim.status = EpistemicStatus::Deprecated;
            }
            claim.last_revised_at = now;
            store
                .operational_state
                .epistemology
                .revisions
                .push_back(BeliefRevision {
                    claim_id: claim.id.clone(),
                    revised_at: now,
                    old_confidence: old,
                    new_confidence: claim.confidence,
                    reason: w.reason.clone(),
                });
        }
    }
    while store.operational_state.epistemology.revisions.len() > MAX_DECISIONS {
        store.operational_state.epistemology.revisions.pop_front();
    }
}

fn epistemic_health(store: &CognitiveStore) -> f64 {
    let claims = &store.operational_state.epistemology.claims;
    if claims.is_empty() {
        return 0.0;
    }
    let verified = claims
        .values()
        .filter(|c| {
            matches!(
                c.status,
                EpistemicStatus::Verified | EpistemicStatus::Supported
            )
        })
        .count() as f64
        / claims.len() as f64;
    let conf = claims.values().map(|c| c.confidence).sum::<f64>() / claims.len() as f64;
    let reliability = store
        .operational_state
        .epistemology
        .source_reliability
        .values()
        .map(|r| r.reliability)
        .sum::<f64>()
        / store
            .operational_state
            .epistemology
            .source_reliability
            .len()
            .max(1) as f64;
    (verified * 0.35 + conf * 0.40 + reliability * 0.25).clamp(0.0, 1.0)
}

fn build_epistemic_relations(store: &CognitiveStore) -> Vec<EpistemicRelation> {
    let claims: Vec<_> = store
        .operational_state
        .epistemology
        .claims
        .values()
        .cloned()
        .collect();
    let mut relations = Vec::new();
    for (i, left) in claims.iter().enumerate() {
        for right in claims.iter().skip(i + 1) {
            let left_tokens = salient_tokens(&left.statement);
            let right_tokens = salient_tokens(&right.statement);
            let overlap = token_overlap_ratio(&left_tokens, &right_tokens);
            if overlap < 0.18 {
                continue;
            }
            let contradictory = is_negating(&left.statement) != is_negating(&right.statement)
                || has_explicit_contradiction_pair(&left.statement, &right.statement)
                || matches!(left.status, EpistemicStatus::Contradicted)
                || matches!(right.status, EpistemicStatus::Contradicted);
            let kind = if contradictory {
                EpistemicRelationKind::Contradicts
            } else if left.confidence >= right.confidence {
                EpistemicRelationKind::Supports
            } else {
                EpistemicRelationKind::Refines
            };
            let weight = (overlap * ((left.confidence + right.confidence) / 2.0)).clamp(0.05, 1.0);
            relations.push(EpistemicRelation {
                from_claim: left.id.clone(),
                to_claim: right.id.clone(),
                kind: kind.clone(),
                weight,
                rationale: format!(
                    "overlap={overlap:.2} confidence_avg={:.2}",
                    (left.confidence + right.confidence) / 2.0
                ),
            });
            relations.push(EpistemicRelation {
                from_claim: right.id.clone(),
                to_claim: left.id.clone(),
                kind,
                weight,
                rationale: format!(
                    "overlap={overlap:.2} confidence_avg={:.2}",
                    (left.confidence + right.confidence) / 2.0
                ),
            });
        }
    }
    relations
}

fn build_conflict_sets(
    _store: &CognitiveStore,
    relations: &[EpistemicRelation],
) -> Vec<EpistemicConflictSet> {
    let mut conflicts = Vec::new();
    let mut seen = BTreeSet::new();
    for relation in relations
        .iter()
        .filter(|r| matches!(r.kind, EpistemicRelationKind::Contradicts))
    {
        let mut pair = [relation.from_claim.clone(), relation.to_claim.clone()];
        pair.sort();
        let key = pair.join("::");
        if seen.insert(key.clone()) {
            conflicts.push(EpistemicConflictSet {
                id: format!("conflict-{}", conflicts.len()),
                claim_ids: pair.to_vec(),
                severity: relation.weight.clamp(0.0, 1.0),
                resolution_hint:
                    "Prefer higher-evidence claim; demote unsupported or stale assertion"
                        .to_string(),
            });
        }
    }
    conflicts
}

fn propagate_epistemic_confidence(
    store: &mut CognitiveStore,
    relations: &[EpistemicRelation],
    conflicts: &[EpistemicConflictSet],
) -> Vec<EpistemicDelta> {
    let mut adjustments: BTreeMap<String, f64> = BTreeMap::new();
    for relation in relations {
        match relation.kind {
            EpistemicRelationKind::Supports
            | EpistemicRelationKind::Explains
            | EpistemicRelationKind::Refines => {
                *adjustments.entry(relation.to_claim.clone()).or_default() +=
                    relation.weight * 0.03;
            }
            EpistemicRelationKind::Contradicts => {
                *adjustments.entry(relation.to_claim.clone()).or_default() -=
                    relation.weight * 0.08;
            }
            EpistemicRelationKind::DependsOn | EpistemicRelationKind::Supersedes => {}
        }
    }
    for conflict in conflicts {
        for claim_id in &conflict.claim_ids {
            *adjustments.entry(claim_id.clone()).or_default() -= conflict.severity * 0.05;
        }
    }
    let mut deltas = Vec::new();
    for (claim_id, delta) in adjustments {
        if let Some(claim) = store
            .operational_state
            .epistemology
            .claims
            .get_mut(&claim_id)
        {
            let old = claim.confidence;
            claim.confidence = (claim.confidence + delta).clamp(0.0, 1.0);
            claim.status = if claim.confidence > 0.85 {
                EpistemicStatus::Verified
            } else if claim.confidence > 0.55 {
                EpistemicStatus::Supported
            } else if claim.confidence < 0.25 {
                EpistemicStatus::Deprecated
            } else {
                claim.status.clone()
            };
            if (old - claim.confidence).abs() > 0.001 {
                deltas.push(EpistemicDelta {
                    claim_id: claim_id.clone(),
                    old_confidence: old,
                    new_confidence: claim.confidence,
                    cause: "relational propagation".to_string(),
                });
            }
        }
    }
    deltas
}

fn commit_revision_transactions(
    _store: &CognitiveStore,
    deltas: &[EpistemicDelta],
    conflicts: &[EpistemicConflictSet],
) -> Vec<RevisionTransaction> {
    if deltas.is_empty() && conflicts.is_empty() {
        return Vec::new();
    }
    vec![RevisionTransaction {
        id: format!("rtx-{}", Utc::now().timestamp_millis()),
        revised_at: Utc::now(),
        claim_ids: deltas.iter().map(|d| d.claim_id.clone()).collect(),
        delta: deltas
            .iter()
            .map(|d| d.new_confidence - d.old_confidence)
            .sum(),
        reason: format!("relations={} conflicts={}", deltas.len(), conflicts.len()),
    }]
}

fn couple_epistemology_to_governor(store: &mut CognitiveStore, conflicts: &[EpistemicConflictSet]) {
    if conflicts.is_empty() {
        return;
    }
    let now = Utc::now();
    store
        .operational_state
        .task_queue
        .push_back(OperationalTask {
            id: format!("epistemic-conflict-audit-{}", now.timestamp_millis()),
            kind: OperationalTaskKind::ContradictionAudit,
            created_at: now,
            due_at: now,
            priority: conflicts
                .iter()
                .map(|c| c.severity)
                .fold(0.0, f64::max)
                .clamp(0.1, 1.0),
            target_node_ids: Vec::new(),
            reason: format!("epistemic conflict sets={}", conflicts.len()),
            completed_at: None,
            outcome: None,
        });
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberativeScienceState {
    #[serde(default)]
    pub deliberation: DeliberationRuntime,
    #[serde(default)]
    pub science: ActiveScientificCognition,
    #[serde(default)]
    pub safety: DeliberativeSafetyLimits,
}
impl Default for DeliberativeScienceState {
    fn default() -> Self {
        Self {
            deliberation: DeliberationRuntime::default(),
            science: ActiveScientificCognition::default(),
            safety: DeliberativeSafetyLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberativeSafetyLimits {
    pub max_deliberation_turns: usize,
    pub max_active_hypotheses: usize,
    pub max_experiments_per_cycle: usize,
    pub max_exploration_budget: f64,
    pub max_prompt_contribution: usize,
}
impl Default for DeliberativeSafetyLimits {
    fn default() -> Self {
        Self {
            max_deliberation_turns: 6,
            max_active_hypotheses: 12,
            max_experiments_per_cycle: 3,
            max_exploration_budget: 1.0,
            max_prompt_contribution: 240,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeliberationActorRole {
    Planner,
    Epistemology,
    Simulator,
    Governor,
    Verifier,
    Repair,
    Doctrine,
    Reality,
    Strategy,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationActor {
    pub role: DeliberationActorRole,
    pub confidence: f64,
    pub evidence_requirements: Vec<String>,
    pub calibration_history: Vec<f64>,
    pub operational_scope: String,
    pub bounded_response_budget: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationClaim {
    pub id: String,
    pub text: String,
    pub confidence: f64,
    pub evidence_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationObjection {
    pub actor: DeliberationActorRole,
    pub reason: String,
    pub severity: f64,
    pub unresolved: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationEvidence {
    pub id: String,
    pub source: String,
    pub weight: f64,
    pub supports: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationTrace {
    pub at: DateTime<Utc>,
    pub actor: DeliberationActorRole,
    pub event: String,
    pub confidence: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceWeightedVote {
    pub actor: DeliberationActorRole,
    pub support: f64,
    pub evidence_weight: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfidenceNegotiation {
    pub initial: f64,
    pub negotiated: f64,
    pub evidence_weight: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationConsensus {
    pub reached: bool,
    pub score: f64,
    pub rationale: String,
    pub votes: Vec<EvidenceWeightedVote>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationDissent {
    pub actor: DeliberationActorRole,
    pub reason: String,
    pub severity: f64,
    pub persisted_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArbitrationDecision {
    pub approved: bool,
    pub risk: String,
    pub action: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ReviewOutcome {
    Pass,
    Blocked,
    RepairRequired,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailureModeCandidate {
    pub mode: String,
    pub likelihood: f64,
    pub impact: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RiskObjection {
    pub risk: String,
    pub severity: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Counterargument {
    pub text: String,
    pub strength: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdversarialReview {
    pub failure_modes: Vec<FailureModeCandidate>,
    pub objections: Vec<RiskObjection>,
    pub counterarguments: Vec<Counterargument>,
    pub outcome: ReviewOutcome,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationOutcome {
    pub consensus: DeliberationConsensus,
    pub dissent: Vec<DeliberationDissent>,
    pub arbitration: ArbitrationDecision,
    pub adversarial_review: AdversarialReview,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationSession {
    pub id: String,
    pub proposal: String,
    pub started_at: DateTime<Utc>,
    pub actors: Vec<DeliberationActor>,
    pub claims: Vec<DeliberationClaim>,
    pub objections: Vec<DeliberationObjection>,
    pub evidence: Vec<DeliberationEvidence>,
    pub trace: Vec<DeliberationTrace>,
    pub outcome: DeliberationOutcome,
    pub bounded_turns: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeliberationRuntime {
    #[serde(default)]
    pub actors: Vec<DeliberationActor>,
    #[serde(default)]
    pub sessions: VecDeque<DeliberationSession>,
    #[serde(default)]
    pub persistent_dissent: VecDeque<DeliberationDissent>,
    pub consensus_count: usize,
    pub bounded_risk_count: usize,
}
impl Default for DeliberationRuntime {
    fn default() -> Self {
        Self {
            actors: default_deliberation_actors(),
            sessions: VecDeque::new(),
            persistent_dissent: VecDeque::new(),
            consensus_count: 0,
            bounded_risk_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HypothesisStatus {
    Proposed,
    Testable,
    Testing,
    Supported,
    Contradicted,
    Inconclusive,
    Promoted,
    Archived,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisEvidence {
    pub evidence_id: String,
    pub sufficiency: f64,
    pub supports: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisPrediction {
    pub claim: String,
    pub expected: String,
    pub confidence: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisTest {
    pub id: String,
    pub safe: bool,
    pub information_gain: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisResult {
    pub test_id: String,
    pub prediction_error: f64,
    pub supported: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisConfidence {
    pub evidence_sufficiency: f64,
    pub calibration: f64,
    pub wrongness: f64,
    pub contradiction_pressure: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisPromotionDecision {
    pub promoted: bool,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalHypothesis {
    pub id: String,
    pub text: String,
    pub status: HypothesisStatus,
    pub evidence: Vec<HypothesisEvidence>,
    pub predictions: Vec<HypothesisPrediction>,
    pub tests: Vec<HypothesisTest>,
    pub results: Vec<HypothesisResult>,
    pub confidence: HypothesisConfidence,
    pub promotion: HypothesisPromotionDecision,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CuriositySignal {
    pub target: String,
    pub uncertainty: f64,
    pub impact: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UncertaintyPriority {
    pub target: String,
    pub score: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InformationGainScore {
    pub target: String,
    pub score: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceGap {
    pub claim: String,
    pub gap: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExplorationBudget {
    pub remaining: f64,
    pub max_per_cycle: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExplorationPolicy {
    pub risk_bounded: bool,
    pub prioritize_information_gain: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvidenceAcquisitionPlan {
    pub target: String,
    pub safe_action: String,
    pub expected_information_gain: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InformationGainEstimate {
    pub target: String,
    pub expected: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UncertaintyReductionPlan {
    pub priorities: Vec<UncertaintyPriority>,
    pub evidence_plans: Vec<EvidenceAcquisitionPlan>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalDiscoveryCandidate {
    pub cause: String,
    pub effect: String,
    pub evidence_count: usize,
    pub confounder_risk: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateCausalEdge {
    pub cause: String,
    pub effect: String,
    pub strength: CausalStrength,
    pub evidence_count: usize,
    pub confounder_risk: ConfounderRisk,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalStrength(pub f64);
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfounderRisk(pub f64);
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InterventionPlan {
    pub safe: bool,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CounterfactualTest {
    pub question: String,
    pub feasible: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CausalDiscoveryEngine {
    pub candidates: Vec<CausalDiscoveryCandidate>,
    pub accepted_edges: Vec<CandidateCausalEdge>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelPrediction {
    pub predicted: String,
    pub confidence: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelScore {
    pub predictive_accuracy: f64,
    pub evidence_fit: f64,
    pub simplicity: f64,
    pub calibration: f64,
    pub wrongness_penalty: f64,
    pub total: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCalibration {
    pub stable: bool,
    pub error_rate: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompetingModel {
    pub id: String,
    pub explanation: String,
    pub predictions: Vec<ModelPrediction>,
    pub score: ModelScore,
    pub calibration: ModelCalibration,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelSelectionDecision {
    pub selected_model_id: Option<String>,
    pub rationale: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypothesisEngine {
    pub max_active: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExperimentPlanner {
    pub max_per_cycle: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScientificCognitionState {
    pub uncertainty_level: String,
    pub information_gain: String,
    pub calibration: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScientificLoopReport {
    pub generated_at: DateTime<Utc>,
    pub hypotheses: usize,
    pub testing: usize,
    pub uncertainty: String,
    pub information_gain: String,
    pub calibration: String,
    pub prompt_status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveScientificCognition {
    #[serde(default)]
    pub hypotheses: VecDeque<OperationalHypothesis>,
    #[serde(default)]
    pub curiosity: Vec<CuriositySignal>,
    #[serde(default)]
    pub evidence_gaps: Vec<EvidenceGap>,
    #[serde(default)]
    pub uncertainty_plan: Option<UncertaintyReductionPlan>,
    #[serde(default)]
    pub causal_engine: CausalDiscoveryEngine,
    #[serde(default)]
    pub competing_models: Vec<CompetingModel>,
    #[serde(default)]
    pub model_selection: Option<ModelSelectionDecision>,
    #[serde(default)]
    pub reports: VecDeque<ScientificLoopReport>,
    pub state: ScientificCognitionState,
    pub hypothesis_engine: HypothesisEngine,
    pub experiment_planner: ExperimentPlanner,
    pub exploration_budget: ExplorationBudget,
    pub exploration_policy: ExplorationPolicy,
}
impl Default for ActiveScientificCognition {
    fn default() -> Self {
        Self {
            hypotheses: VecDeque::new(),
            curiosity: Vec::new(),
            evidence_gaps: Vec::new(),
            uncertainty_plan: None,
            causal_engine: CausalDiscoveryEngine {
                candidates: Vec::new(),
                accepted_edges: Vec::new(),
            },
            competing_models: Vec::new(),
            model_selection: None,
            reports: VecDeque::new(),
            state: ScientificCognitionState {
                uncertainty_level: "low".into(),
                information_gain: "low".into(),
                calibration: "stable".into(),
            },
            hypothesis_engine: HypothesisEngine { max_active: 12 },
            experiment_planner: ExperimentPlanner { max_per_cycle: 3 },
            exploration_budget: ExplorationBudget {
                remaining: 1.0,
                max_per_cycle: 1.0,
            },
            exploration_policy: ExplorationPolicy {
                risk_bounded: true,
                prioritize_information_gain: true,
            },
        }
    }
}

fn default_deliberation_actors() -> Vec<DeliberationActor> {
    [
        DeliberationActorRole::Planner,
        DeliberationActorRole::Epistemology,
        DeliberationActorRole::Simulator,
        DeliberationActorRole::Governor,
        DeliberationActorRole::Verifier,
        DeliberationActorRole::Repair,
        DeliberationActorRole::Doctrine,
        DeliberationActorRole::Reality,
        DeliberationActorRole::Strategy,
    ]
    .into_iter()
    .map(|role| DeliberationActor {
        role,
        confidence: 0.72,
        evidence_requirements: vec!["bounded evidence check".into()],
        calibration_history: vec![0.72],
        operational_scope: "truth-seeking operational cognition".into(),
        bounded_response_budget: 1,
    })
    .collect()
}

pub fn run_deliberative_science(
    reason: impl Into<String>,
) -> io::Result<(DeliberationSession, ScientificLoopReport)> {
    let mut store = load_store()?;
    let out = run_deliberative_science_in_store(&mut store, reason.into());
    save_store(&store)?;
    Ok(out)
}

pub fn run_deliberative_science_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> (DeliberationSession, ScientificLoopReport) {
    let now = Utc::now();
    let limits = store.operational_state.deliberative_science.safety.clone();
    let epistemology_report =
        run_epistemology_in_store(store, format!("deliberative_science:{reason}"));
    let evidence_weight = (epistemology_report.evidence.len() as f64
        / (epistemology_report.claims.len().max(1) as f64))
        .clamp(0.0, 1.0);
    let contradiction_pressure = (epistemology_report.conflict_sets.len() as f64
        / epistemology_report.claims.len().max(1) as f64)
        .clamp(0.0, 1.0);
    let actors = store
        .operational_state
        .deliberative_science
        .deliberation
        .actors
        .clone();
    let votes: Vec<_> = actors
        .iter()
        .map(|a| EvidenceWeightedVote {
            actor: a.role.clone(),
            support: (a.confidence
                * (0.55 + evidence_weight * 0.45)
                * (1.0 - contradiction_pressure * 0.5))
                .clamp(0.0, 1.0),
            evidence_weight,
        })
        .collect();
    let consensus_score = if votes.is_empty() {
        0.0
    } else {
        votes
            .iter()
            .map(|v| v.support * v.evidence_weight.max(0.25))
            .sum::<f64>()
            / votes.len() as f64
    };
    let risky = contradiction_pressure > 0.30
        || evidence_weight < 0.20
        || reason.contains("risky")
        || reason.contains("autonomy");
    let review = AdversarialReview {
        failure_modes: vec![FailureModeCandidate {
            mode: "unsupported action or unresolved contradiction".into(),
            likelihood: (1.0 - evidence_weight).clamp(0.0, 1.0),
            impact: contradiction_pressure.max(if risky { 0.7 } else { 0.2 }),
        }],
        objections: if risky {
            vec![RiskObjection {
                risk: "evidence insufficient or contradiction pressure elevated".into(),
                severity: 0.8,
            }]
        } else {
            Vec::new()
        },
        counterarguments: vec![Counterargument {
            text: "bounded execution with verification and repair fallback".into(),
            strength: evidence_weight,
        }],
        outcome: if risky {
            ReviewOutcome::RepairRequired
        } else {
            ReviewOutcome::Pass
        },
    };
    let dissent: Vec<_> = review
        .objections
        .iter()
        .map(|o| DeliberationDissent {
            actor: DeliberationActorRole::Verifier,
            reason: o.risk.clone(),
            severity: o.severity,
            persisted_at: now,
        })
        .collect();
    let outcome = DeliberationOutcome {
        consensus: DeliberationConsensus {
            reached: consensus_score >= 0.45 && !matches!(review.outcome, ReviewOutcome::Blocked),
            score: consensus_score,
            rationale: "evidence-weighted bounded deliberation".into(),
            votes,
        },
        dissent: dissent.clone(),
        arbitration: ArbitrationDecision {
            approved: !matches!(review.outcome, ReviewOutcome::Blocked) && !risky,
            risk: if risky { "bounded-repair" } else { "bounded" }.into(),
            action: if risky {
                "repair_or_gather_evidence"
            } else {
                "proceed_with_verification"
            }
            .into(),
        },
        adversarial_review: review,
    };
    let session = DeliberationSession {
        id: format!("delib-{}", now.timestamp_millis()),
        proposal: compact(&reason, 160),
        started_at: now,
        actors: actors.clone(),
        claims: vec![DeliberationClaim {
            id: "proposal".into(),
            text: compact(&reason, 160),
            confidence: consensus_score,
            evidence_refs: vec!["epistemology_report".into()],
        }],
        objections: dissent
            .iter()
            .map(|d| DeliberationObjection {
                actor: d.actor.clone(),
                reason: d.reason.clone(),
                severity: d.severity,
                unresolved: true,
            })
            .collect(),
        evidence: vec![DeliberationEvidence {
            id: "epistemology_report".into(),
            source: "truth-maintenance".into(),
            weight: evidence_weight,
            supports: evidence_weight >= 0.2,
        }],
        trace: actors
            .iter()
            .take(limits.max_deliberation_turns)
            .map(|a| DeliberationTrace {
                at: now,
                actor: a.role.clone(),
                event: "bounded evidence review".into(),
                confidence: a.confidence,
            })
            .collect(),
        outcome,
        bounded_turns: limits.max_deliberation_turns.min(9),
    };
    let ds = &mut store.operational_state.deliberative_science;
    ds.deliberation.sessions.push_back(session.clone());
    while ds.deliberation.sessions.len() > MAX_DECISIONS {
        ds.deliberation.sessions.pop_front();
    }
    if session.outcome.consensus.reached {
        ds.deliberation.consensus_count += 1;
    }
    if session.outcome.arbitration.risk.contains("bounded") {
        ds.deliberation.bounded_risk_count += 1;
    }
    for d in dissent {
        ds.deliberation.persistent_dissent.push_back(d);
    }
    while ds.deliberation.persistent_dissent.len() > MAX_DECISIONS {
        ds.deliberation.persistent_dissent.pop_front();
    }
    evolve_scientific_cognition(ds, &reason, evidence_weight, contradiction_pressure, now);
    let report = ds.science.reports.back().cloned().unwrap();
    (session, report)
}

fn evolve_scientific_cognition(
    ds: &mut DeliberativeScienceState,
    reason: &str,
    evidence_weight: f64,
    contradiction_pressure: f64,
    now: DateTime<Utc>,
) {
    let active = ds
        .science
        .hypotheses
        .iter()
        .filter(|h| {
            !matches!(
                h.status,
                HypothesisStatus::Archived | HypothesisStatus::Promoted
            )
        })
        .count();
    if active < ds.safety.max_active_hypotheses {
        ds.science.hypotheses.push_back(OperationalHypothesis {
            id: format!("hyp-{}", now.timestamp_millis()),
            text: format!("Operational uncertainty about {}", compact(reason, 80)),
            status: if evidence_weight > 0.35 {
                HypothesisStatus::Testable
            } else {
                HypothesisStatus::Proposed
            },
            evidence: vec![HypothesisEvidence {
                evidence_id: "epistemology_report".into(),
                sufficiency: evidence_weight,
                supports: evidence_weight >= 0.3,
            }],
            predictions: vec![HypothesisPrediction {
                claim: "additional verification reduces uncertainty".into(),
                expected: "lower contradiction pressure".into(),
                confidence: 0.62,
            }],
            tests: vec![HypothesisTest {
                id: "safe-evidence-check".into(),
                safe: true,
                information_gain: ((1.0 - evidence_weight) * (1.0 + contradiction_pressure))
                    .clamp(0.0, 1.0),
            }],
            results: Vec::new(),
            confidence: HypothesisConfidence {
                evidence_sufficiency: evidence_weight,
                calibration: 0.72,
                wrongness: 0.0,
                contradiction_pressure,
            },
            promotion: decide_hypothesis_promotion(
                evidence_weight,
                0.72,
                0.0,
                contradiction_pressure,
            ),
        });
    }
    while ds.science.hypotheses.len() > ds.safety.max_active_hypotheses {
        ds.science.hypotheses.pop_front();
    }
    let info = ((1.0 - evidence_weight) * (1.0 + contradiction_pressure)).clamp(0.0, 1.0);
    ds.science.curiosity = vec![CuriositySignal {
        target: compact(reason, 80),
        uncertainty: 1.0 - evidence_weight,
        impact: contradiction_pressure.max(0.4),
    }];
    ds.science.evidence_gaps = vec![EvidenceGap {
        claim: compact(reason, 80),
        gap: 1.0 - evidence_weight,
    }];
    ds.science.uncertainty_plan = Some(UncertaintyReductionPlan {
        priorities: vec![UncertaintyPriority {
            target: compact(reason, 80),
            score: info,
        }],
        evidence_plans: vec![EvidenceAcquisitionPlan {
            target: compact(reason, 80),
            safe_action: "run bounded verification before doctrine/autonomy changes".into(),
            expected_information_gain: info,
        }],
    });
    let candidate = CausalDiscoveryCandidate {
        cause: "verification coverage".into(),
        effect: "confidence stability".into(),
        evidence_count: if evidence_weight > 0.4 { 2 } else { 1 },
        confounder_risk: contradiction_pressure,
    };
    ds.science.causal_engine.candidates = vec![candidate.clone()];
    ds.science.causal_engine.accepted_edges =
        if candidate.evidence_count >= 2 && candidate.confounder_risk < 0.4 {
            vec![CandidateCausalEdge {
                cause: candidate.cause,
                effect: candidate.effect,
                strength: CausalStrength(evidence_weight),
                evidence_count: candidate.evidence_count,
                confounder_risk: ConfounderRisk(candidate.confounder_risk),
            }]
        } else {
            Vec::new()
        };
    ds.science.competing_models = score_competing_models(evidence_weight, contradiction_pressure);
    ds.science.model_selection = ds.science.competing_models.iter().max_by(|a,b| a.score.total.partial_cmp(&b.score.total).unwrap_or(std::cmp::Ordering::Equal)).map(|m| ModelSelectionDecision { selected_model_id: Some(m.id.clone()), rationale: "selected by predictive accuracy, evidence fit, simplicity, calibration, and wrongness penalty".into() });
    ds.science.state = ScientificCognitionState {
        uncertainty_level: if info > 0.66 {
            "high"
        } else if info > 0.33 {
            "moderate"
        } else {
            "low"
        }
        .into(),
        information_gain: if info > 0.66 {
            "high"
        } else if info > 0.33 {
            "moderate"
        } else {
            "low"
        }
        .into(),
        calibration: if contradiction_pressure < 0.25 {
            "stable"
        } else {
            "watch"
        }
        .into(),
    };
    let testing = ds
        .science
        .hypotheses
        .iter()
        .filter(|h| {
            matches!(
                h.status,
                HypothesisStatus::Testing | HypothesisStatus::Testable
            )
        })
        .count();
    let report = ScientificLoopReport {
        generated_at: now,
        hypotheses: ds.science.hypotheses.len(),
        testing,
        uncertainty: ds.science.state.uncertainty_level.clone(),
        information_gain: ds.science.state.information_gain.clone(),
        calibration: ds.science.state.calibration.clone(),
        prompt_status: format!(
            "Scientific cognition: hypotheses={} testing={} uncertainty={} info_gain={} calibration={}",
            ds.science.hypotheses.len(),
            testing,
            ds.science.state.uncertainty_level,
            ds.science.state.information_gain,
            ds.science.state.calibration
        ),
    };
    ds.science.reports.push_back(report);
    while ds.science.reports.len() > MAX_DECISIONS {
        ds.science.reports.pop_front();
    }
}

pub fn decide_hypothesis_promotion(
    evidence_sufficiency: f64,
    calibration: f64,
    wrongness: f64,
    contradiction_pressure: f64,
) -> HypothesisPromotionDecision {
    let ok = evidence_sufficiency >= 0.75
        && calibration >= 0.70
        && wrongness <= 0.15
        && contradiction_pressure <= 0.20;
    HypothesisPromotionDecision {
        promoted: ok,
        reason: if ok {
            "evidence sufficient, calibration stable, wrongness low, contradictions low"
        } else {
            "promotion blocked until evidence/calibration/contradiction thresholds pass"
        }
        .into(),
    }
}

pub fn score_competing_models(
    evidence_weight: f64,
    contradiction_pressure: f64,
) -> Vec<CompetingModel> {
    [
        ("stale-retrieval", 0.64, 0.60, 0.80),
        (
            "unstable-doctrine",
            0.55,
            1.0 - contradiction_pressure,
            0.70,
        ),
        (
            "insufficient-verification",
            0.72,
            1.0 - evidence_weight,
            0.85,
        ),
    ]
    .into_iter()
    .map(|(id, acc, fit, simp)| {
        let cal = 0.72;
        let wrong = contradiction_pressure * 0.25;
        let total = acc * 0.30 + fit * 0.30 + simp * 0.15 + cal * 0.20 - wrong * 0.25;
        CompetingModel {
            id: id.into(),
            explanation: id.replace('-', " "),
            predictions: vec![ModelPrediction {
                predicted: "verification changes outcome confidence".into(),
                confidence: acc,
            }],
            score: ModelScore {
                predictive_accuracy: acc,
                evidence_fit: fit,
                simplicity: simp,
                calibration: cal,
                wrongness_penalty: wrong,
                total,
            },
            calibration: ModelCalibration {
                stable: contradiction_pressure < 0.4,
                error_rate: wrong,
            },
        }
    })
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticScientificGovernanceState {
    #[serde(default)]
    pub ecosystem: SyntheticScienceEcosystem,
    #[serde(default)]
    pub cybernetic_governor: CyberneticEpistemicGovernor,
    #[serde(default)]
    pub institutions: EpistemicInstitutions,
    #[serde(default)]
    pub safety: SyntheticGovernanceLimits,
    #[serde(default)]
    pub reports: VecDeque<SyntheticGovernanceReport>,
}
impl Default for SyntheticScientificGovernanceState {
    fn default() -> Self {
        Self {
            ecosystem: SyntheticScienceEcosystem::default(),
            cybernetic_governor: CyberneticEpistemicGovernor::default(),
            institutions: EpistemicInstitutions::default(),
            safety: SyntheticGovernanceLimits::default(),
            reports: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticGovernanceLimits {
    pub max_ecosystem_cycles: usize,
    pub max_agent_proposals: usize,
    pub max_institutional_decisions: usize,
    pub max_prompt_contribution: usize,
    pub max_calibration_pressure: f64,
}
impl Default for SyntheticGovernanceLimits {
    fn default() -> Self {
        Self {
            max_ecosystem_cycles: 3,
            max_agent_proposals: 8,
            max_institutional_decisions: 8,
            max_prompt_contribution: 240,
            max_calibration_pressure: 0.75,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyntheticScientistRole {
    HypothesisGenerator,
    Skeptic,
    Replicator,
    CausalAnalyst,
    CalibrationAuditor,
    GovernanceReviewer,
    AnomalyHunter,
    ModelCompetitor,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticScientist {
    pub role: SyntheticScientistRole,
    pub credibility: f64,
    pub budget: usize,
    pub calibration_error: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticResearchProposal {
    pub id: String,
    pub proposer: SyntheticScientistRole,
    pub question: String,
    pub expected_information_gain: f64,
    pub risk: f64,
    pub replication_required: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticExperimentRecord {
    pub id: String,
    pub proposal_id: String,
    pub safe: bool,
    pub predicted: String,
    pub observed: String,
    pub prediction_error: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplicationAttempt {
    pub experiment_id: String,
    pub replicated: bool,
    pub error_delta: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DissentEcologySignal {
    pub source: SyntheticScientistRole,
    pub claim: String,
    pub productive: bool,
    pub severity: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelfChallengeCycle {
    pub target: String,
    pub challenge: String,
    pub outcome: String,
    pub reduced_overconfidence: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticScienceEcosystem {
    #[serde(default)]
    pub scientists: Vec<SyntheticScientist>,
    #[serde(default)]
    pub proposals: VecDeque<SyntheticResearchProposal>,
    #[serde(default)]
    pub experiments: VecDeque<SyntheticExperimentRecord>,
    #[serde(default)]
    pub replications: VecDeque<ReplicationAttempt>,
    #[serde(default)]
    pub dissent_ecology: VecDeque<DissentEcologySignal>,
    #[serde(default)]
    pub self_challenges: VecDeque<SelfChallengeCycle>,
    pub cycles_run: usize,
}
impl Default for SyntheticScienceEcosystem {
    fn default() -> Self {
        Self {
            scientists: default_synthetic_scientists(),
            proposals: VecDeque::new(),
            experiments: VecDeque::new(),
            replications: VecDeque::new(),
            dissent_ecology: VecDeque::new(),
            self_challenges: VecDeque::new(),
            cycles_run: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicControlSignal {
    pub name: String,
    pub value: f64,
    pub target: f64,
    pub correction: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CyberneticFeedbackLoop {
    pub loop_name: String,
    pub sensor: String,
    pub actuator: String,
    pub stable: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicGovernanceAction {
    pub action: String,
    pub intensity: f64,
    pub rationale: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CyberneticEpistemicGovernor {
    #[serde(default)]
    pub control_signals: Vec<EpistemicControlSignal>,
    #[serde(default)]
    pub feedback_loops: Vec<CyberneticFeedbackLoop>,
    #[serde(default)]
    pub actions: VecDeque<EpistemicGovernanceAction>,
    pub stability: f64,
    pub overconfidence_pressure: f64,
}
impl Default for CyberneticEpistemicGovernor {
    fn default() -> Self {
        Self {
            control_signals: Vec::new(),
            feedback_loops: Vec::new(),
            actions: VecDeque::new(),
            stability: 1.0,
            overconfidence_pressure: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InstitutionalDecisionKind {
    PromoteHypothesis,
    RequireReplication,
    QuarantineDoctrine,
    IncreaseVerification,
    ArchiveClaim,
    CalibrateConfidence,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstitutionalDecision {
    pub kind: InstitutionalDecisionKind,
    pub target: String,
    pub approved: bool,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicInstitution {
    pub name: String,
    pub authority: f64,
    pub decision_budget: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicInstitutions {
    #[serde(default)]
    pub institutions: Vec<EpistemicInstitution>,
    #[serde(default)]
    pub decisions: VecDeque<InstitutionalDecision>,
}
impl Default for EpistemicInstitutions {
    fn default() -> Self {
        Self {
            institutions: vec![
                EpistemicInstitution {
                    name: "replication_board".into(),
                    authority: 0.8,
                    decision_budget: 2,
                },
                EpistemicInstitution {
                    name: "calibration_court".into(),
                    authority: 0.85,
                    decision_budget: 2,
                },
                EpistemicInstitution {
                    name: "doctrine_review".into(),
                    authority: 0.9,
                    decision_budget: 2,
                },
            ],
            decisions: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyntheticGovernanceReport {
    pub generated_at: DateTime<Utc>,
    pub cycles: usize,
    pub proposals: usize,
    pub experiments: usize,
    pub replications: usize,
    pub dissent: usize,
    pub stability: f64,
    pub actions: usize,
    pub prompt_status: String,
}

fn default_synthetic_scientists() -> Vec<SyntheticScientist> {
    [
        SyntheticScientistRole::HypothesisGenerator,
        SyntheticScientistRole::Skeptic,
        SyntheticScientistRole::Replicator,
        SyntheticScientistRole::CausalAnalyst,
        SyntheticScientistRole::CalibrationAuditor,
        SyntheticScientistRole::GovernanceReviewer,
        SyntheticScientistRole::AnomalyHunter,
        SyntheticScientistRole::ModelCompetitor,
    ]
    .into_iter()
    .map(|role| SyntheticScientist {
        role,
        credibility: 0.72,
        budget: 1,
        calibration_error: 0.08,
    })
    .collect()
}

pub fn run_synthetic_scientific_governance(
    reason: impl Into<String>,
) -> io::Result<SyntheticGovernanceReport> {
    let mut store = load_store()?;
    let report = run_synthetic_scientific_governance_in_store(&mut store, reason.into());
    save_store(&store)?;
    Ok(report)
}

pub fn run_synthetic_scientific_governance_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> SyntheticGovernanceReport {
    let now = Utc::now();
    let (_session, science_report) =
        run_deliberative_science_in_store(store, format!("synthetic_governance:{reason}"));
    let sg = &mut store.operational_state.synthetic_governance;
    let uncertainty = match science_report.uncertainty.as_str() {
        "high" => 0.8,
        "moderate" => 0.5,
        _ => 0.2,
    };
    let info_gain = match science_report.information_gain.as_str() {
        "high" => 0.8,
        "moderate" => 0.5,
        _ => 0.2,
    };
    let overconfidence = (1.0 - uncertainty)
        * if science_report.calibration == "stable" {
            0.3
        } else {
            0.7
        };
    sg.cybernetic_governor.overconfidence_pressure = overconfidence;
    sg.cybernetic_governor.stability =
        (1.0 - (uncertainty * 0.35 + overconfidence * 0.35)).clamp(0.0, 1.0);
    sg.cybernetic_governor.control_signals = vec![
        EpistemicControlSignal {
            name: "uncertainty".into(),
            value: uncertainty,
            target: 0.35,
            correction: (uncertainty - 0.35).max(0.0),
        },
        EpistemicControlSignal {
            name: "overconfidence".into(),
            value: overconfidence,
            target: 0.20,
            correction: (overconfidence - 0.20).max(0.0),
        },
        EpistemicControlSignal {
            name: "information_gain".into(),
            value: info_gain,
            target: 0.50,
            correction: (0.50 - info_gain).max(0.0),
        },
    ];
    sg.cybernetic_governor.feedback_loops = vec![
        CyberneticFeedbackLoop {
            loop_name: "uncertainty_to_experiment_budget".into(),
            sensor: "scientific_uncertainty".into(),
            actuator: "verification_pressure".into(),
            stable: uncertainty < 0.75,
        },
        CyberneticFeedbackLoop {
            loop_name: "overconfidence_to_calibration".into(),
            sensor: "confidence_pressure".into(),
            actuator: "confidence_damping".into(),
            stable: overconfidence < sg.safety.max_calibration_pressure,
        },
    ];
    let action = if overconfidence > 0.2 {
        EpistemicGovernanceAction {
            action: "dampen confidence and require replication".into(),
            intensity: overconfidence,
            rationale: "cybernetic overconfidence control".into(),
        }
    } else {
        EpistemicGovernanceAction {
            action: "continue bounded exploration".into(),
            intensity: info_gain,
            rationale: "information gain remains useful".into(),
        }
    };
    sg.cybernetic_governor.actions.push_back(action);
    while sg.cybernetic_governor.actions.len() > MAX_DECISIONS {
        sg.cybernetic_governor.actions.pop_front();
    }
    let cycle_budget = sg.safety.max_ecosystem_cycles.min(3);
    for idx in 0..cycle_budget {
        run_synthetic_ecosystem_cycle(sg, &reason, idx, uncertainty, info_gain, now);
    }
    let decision = InstitutionalDecision {
        kind: if uncertainty > 0.6 {
            InstitutionalDecisionKind::IncreaseVerification
        } else if overconfidence > 0.2 {
            InstitutionalDecisionKind::RequireReplication
        } else {
            InstitutionalDecisionKind::CalibrateConfidence
        },
        target: compact(&reason, 80),
        approved: true,
        reason: "institutional epistemic governance applied bounded corrective control".into(),
    };
    sg.institutions.decisions.push_back(decision);
    while sg.institutions.decisions.len() > sg.safety.max_institutional_decisions {
        sg.institutions.decisions.pop_front();
    }
    let report = SyntheticGovernanceReport {
        generated_at: now,
        cycles: sg.ecosystem.cycles_run,
        proposals: sg.ecosystem.proposals.len(),
        experiments: sg.ecosystem.experiments.len(),
        replications: sg.ecosystem.replications.len(),
        dissent: sg.ecosystem.dissent_ecology.len(),
        stability: sg.cybernetic_governor.stability,
        actions: sg.cybernetic_governor.actions.len(),
        prompt_status: format!(
            "Synthetic science governance: cycles={} proposals={} experiments={} replications={} dissent={} stability={:.2} actions={}",
            sg.ecosystem.cycles_run,
            sg.ecosystem.proposals.len(),
            sg.ecosystem.experiments.len(),
            sg.ecosystem.replications.len(),
            sg.ecosystem.dissent_ecology.len(),
            sg.cybernetic_governor.stability,
            sg.cybernetic_governor.actions.len()
        ),
    };
    sg.reports.push_back(report.clone());
    while sg.reports.len() > MAX_DECISIONS {
        sg.reports.pop_front();
    }
    report
}

fn run_synthetic_ecosystem_cycle(
    sg: &mut SyntheticScientificGovernanceState,
    reason: &str,
    idx: usize,
    uncertainty: f64,
    info_gain: f64,
    now: DateTime<Utc>,
) {
    sg.ecosystem.cycles_run += 1;
    let role = sg
        .ecosystem
        .scientists
        .get(idx % sg.ecosystem.scientists.len())
        .map(|s| s.role.clone())
        .unwrap_or(SyntheticScientistRole::HypothesisGenerator);
    let proposal = SyntheticResearchProposal {
        id: format!("syn-prop-{}-{idx}", now.timestamp_millis()),
        proposer: role.clone(),
        question: format!(
            "What evidence would reduce uncertainty about {}?",
            compact(reason, 80)
        ),
        expected_information_gain: info_gain,
        risk: (uncertainty * 0.25).clamp(0.0, 1.0),
        replication_required: uncertainty > 0.4,
    };
    sg.ecosystem.proposals.push_back(proposal.clone());
    while sg.ecosystem.proposals.len() > sg.safety.max_agent_proposals {
        sg.ecosystem.proposals.pop_front();
    }
    let experiment = SyntheticExperimentRecord {
        id: format!("syn-exp-{}-{idx}", now.timestamp_millis()),
        proposal_id: proposal.id.clone(),
        safe: proposal.risk < 0.4,
        predicted: "verification lowers uncertainty".into(),
        observed: if info_gain >= 0.4 {
            "evidence gap identified"
        } else {
            "uncertainty already bounded"
        }
        .into(),
        prediction_error: (0.5 - info_gain).abs(),
    };
    sg.ecosystem.experiments.push_back(experiment.clone());
    while sg.ecosystem.experiments.len() > MAX_DECISIONS {
        sg.ecosystem.experiments.pop_front();
    }
    sg.ecosystem.replications.push_back(ReplicationAttempt {
        experiment_id: experiment.id.clone(),
        replicated: experiment.safe && experiment.prediction_error < 0.45,
        error_delta: experiment.prediction_error,
    });
    while sg.ecosystem.replications.len() > MAX_DECISIONS {
        sg.ecosystem.replications.pop_front();
    }
    sg.ecosystem
        .dissent_ecology
        .push_back(DissentEcologySignal {
            source: SyntheticScientistRole::Skeptic,
            claim: "do not promote without replication and calibration".into(),
            productive: true,
            severity: uncertainty,
        });
    while sg.ecosystem.dissent_ecology.len() > MAX_DECISIONS {
        sg.ecosystem.dissent_ecology.pop_front();
    }
    sg.ecosystem.self_challenges.push_back(SelfChallengeCycle {
        target: compact(reason, 80),
        challenge: "assume current favored model is wrong and seek discriminating evidence".into(),
        outcome: "replication and confidence damping required when uncertainty persists".into(),
        reduced_overconfidence: true,
    });
    while sg.ecosystem.self_challenges.len() > MAX_DECISIONS {
        sg.ecosystem.self_challenges.pop_front();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveIntegrationContextEconomyState {
    #[serde(default)]
    pub integration: CognitiveIntegrationMesh,
    #[serde(default)]
    pub context_economy: AdaptiveContextEconomy,
    #[serde(default)]
    pub budget: ContextEconomyBudget,
    #[serde(default)]
    pub reports: VecDeque<ContextEconomyReport>,
}
impl Default for CognitiveIntegrationContextEconomyState {
    fn default() -> Self {
        Self {
            integration: CognitiveIntegrationMesh::default(),
            context_economy: AdaptiveContextEconomy::default(),
            budget: ContextEconomyBudget::default(),
            reports: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextEconomyBudget {
    pub max_prompt_tokens: usize,
    pub max_context_items: usize,
    pub max_reports: usize,
    pub min_salience: f64,
    pub max_raw_items: usize,
}
impl Default for ContextEconomyBudget {
    fn default() -> Self {
        Self {
            max_prompt_tokens: 512,
            max_context_items: 12,
            max_reports: 32,
            min_salience: 0.15,
            max_raw_items: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum CognitiveLayerKind {
    Memory,
    Epistemology,
    Deliberation,
    Science,
    SyntheticScience,
    Governance,
    Execution,
    Context,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveIntegrationNode {
    pub id: String,
    pub layer: CognitiveLayerKind,
    pub salience: f64,
    pub confidence: f64,
    pub token_cost: usize,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveIntegrationEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub weight: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntegrationConflict {
    pub node_a: String,
    pub node_b: String,
    pub pressure: f64,
    pub resolution: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveIntegrationMesh {
    #[serde(default)]
    pub nodes: Vec<CognitiveIntegrationNode>,
    #[serde(default)]
    pub edges: Vec<CognitiveIntegrationEdge>,
    #[serde(default)]
    pub conflicts: Vec<IntegrationConflict>,
    pub coherence: f64,
}
impl Default for CognitiveIntegrationMesh {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            conflicts: Vec::new(),
            coherence: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContextAllocationDecision {
    Include,
    Compress,
    Defer,
    Drop,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextMarketBid {
    pub item_id: String,
    pub layer: CognitiveLayerKind,
    pub utility: f64,
    pub token_cost: usize,
    pub bid_score: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextAllocation {
    pub item_id: String,
    pub decision: ContextAllocationDecision,
    pub tokens: usize,
    pub rationale: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressionPlan {
    pub item_id: String,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub method: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextDebtSignal {
    pub source: String,
    pub debt: f64,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextRecallTrace {
    pub item_id: String,
    pub reason: String,
    pub salience: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdaptiveContextEconomy {
    #[serde(default)]
    pub bids: Vec<ContextMarketBid>,
    #[serde(default)]
    pub allocations: Vec<ContextAllocation>,
    #[serde(default)]
    pub compression_plans: Vec<CompressionPlan>,
    #[serde(default)]
    pub debt: Vec<ContextDebtSignal>,
    #[serde(default)]
    pub recall_trace: Vec<ContextRecallTrace>,
    pub spent_tokens: usize,
    pub saved_tokens: usize,
    pub efficiency: f64,
}
impl Default for AdaptiveContextEconomy {
    fn default() -> Self {
        Self {
            bids: Vec::new(),
            allocations: Vec::new(),
            compression_plans: Vec::new(),
            debt: Vec::new(),
            recall_trace: Vec::new(),
            spent_tokens: 0,
            saved_tokens: 0,
            efficiency: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextEconomyReport {
    pub generated_at: DateTime<Utc>,
    pub nodes: usize,
    pub edges: usize,
    pub coherence: f64,
    pub included: usize,
    pub compressed: usize,
    pub deferred: usize,
    pub dropped: usize,
    pub spent_tokens: usize,
    pub saved_tokens: usize,
    pub efficiency: f64,
    pub prompt_status: String,
}

pub fn run_cognitive_context_economy(
    reason: impl Into<String>,
) -> io::Result<ContextEconomyReport> {
    let mut store = load_store()?;
    let report = run_cognitive_context_economy_in_store(&mut store, reason.into());
    save_store(&store)?;
    Ok(report)
}

pub fn run_cognitive_context_economy_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> ContextEconomyReport {
    let now = Utc::now();
    let synthetic =
        run_synthetic_scientific_governance_in_store(store, format!("context_economy:{reason}"));
    let epistemology = run_epistemology_in_store(store, format!("context_economy:{reason}"));
    let state = &mut store.operational_state.cognitive_context_economy;
    let nodes = build_integration_nodes(&reason, &synthetic, &epistemology);
    let edges = build_integration_edges(&nodes);
    let conflicts = build_integration_conflicts(&nodes, &epistemology);
    let coherence = (1.0
        - conflicts.iter().map(|c| c.pressure).sum::<f64>() / nodes.len().max(1) as f64)
        .clamp(0.0, 1.0);
    state.integration = CognitiveIntegrationMesh {
        nodes: nodes.clone(),
        edges,
        conflicts,
        coherence,
    };
    let mut bids: Vec<_> = nodes
        .iter()
        .map(|n| ContextMarketBid {
            item_id: n.id.clone(),
            layer: n.layer.clone(),
            utility: (n.salience * 0.45 + n.confidence * 0.35 + coherence * 0.20).clamp(0.0, 1.0),
            token_cost: n.token_cost,
            bid_score: ((n.salience * 0.45 + n.confidence * 0.35 + coherence * 0.20)
                / n.token_cost.max(1) as f64)
                .clamp(0.0, 1.0),
        })
        .collect();
    bids.sort_by(|a, b| {
        b.bid_score
            .partial_cmp(&a.bid_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut spent = 0usize;
    let mut included = 0usize;
    let mut compressed = 0usize;
    let mut deferred = 0usize;
    let mut dropped = 0usize;
    let mut saved = 0usize;
    let mut allocations = Vec::new();
    let mut compression_plans = Vec::new();
    let mut recall_trace = Vec::new();
    let mut debt = Vec::new();
    for bid in bids.iter().take(state.budget.max_context_items) {
        let decision = if bid.utility < state.budget.min_salience {
            ContextAllocationDecision::Drop
        } else if spent + bid.token_cost <= state.budget.max_prompt_tokens
            && included < state.budget.max_raw_items
        {
            ContextAllocationDecision::Include
        } else if spent + (bid.token_cost / 3).max(12) <= state.budget.max_prompt_tokens {
            ContextAllocationDecision::Compress
        } else {
            ContextAllocationDecision::Defer
        };
        match decision {
            ContextAllocationDecision::Include => {
                spent += bid.token_cost;
                included += 1;
            }
            ContextAllocationDecision::Compress => {
                let ct = (bid.token_cost / 3).max(12);
                spent += ct;
                saved += bid.token_cost.saturating_sub(ct);
                compressed += 1;
                compression_plans.push(CompressionPlan {
                    item_id: bid.item_id.clone(),
                    original_tokens: bid.token_cost,
                    compressed_tokens: ct,
                    method: "salience-weighted semantic compression".into(),
                });
            }
            ContextAllocationDecision::Defer => {
                deferred += 1;
                debt.push(ContextDebtSignal {
                    source: bid.item_id.clone(),
                    debt: bid.utility,
                    reason: "useful but over current prompt budget".into(),
                });
            }
            ContextAllocationDecision::Drop => {
                dropped += 1;
            }
        }
        recall_trace.push(ContextRecallTrace {
            item_id: bid.item_id.clone(),
            reason: "context market utility allocation".into(),
            salience: bid.utility,
        });
        allocations.push(ContextAllocation {
            item_id: bid.item_id.clone(),
            decision,
            tokens: bid.token_cost,
            rationale: "budgeted by utility per token, salience, confidence, and coherence".into(),
        });
    }
    let efficiency = ((included + compressed) as f64 / allocations.len().max(1) as f64 * 0.6
        + saved as f64 / (spent + saved).max(1) as f64 * 0.4)
        .clamp(0.0, 1.0);
    state.context_economy = AdaptiveContextEconomy {
        bids,
        allocations,
        compression_plans,
        debt,
        recall_trace,
        spent_tokens: spent,
        saved_tokens: saved,
        efficiency,
    };
    let report = ContextEconomyReport {
        generated_at: now,
        nodes: state.integration.nodes.len(),
        edges: state.integration.edges.len(),
        coherence,
        included,
        compressed,
        deferred,
        dropped,
        spent_tokens: spent,
        saved_tokens: saved,
        efficiency,
        prompt_status: format!(
            "Context economy: nodes={} edges={} coherence={:.2} included={} compressed={} deferred={} spent={} saved={} efficiency={:.2}",
            state.integration.nodes.len(),
            state.integration.edges.len(),
            coherence,
            included,
            compressed,
            deferred,
            spent,
            saved,
            efficiency
        ),
    };
    state.reports.push_back(report.clone());
    while state.reports.len() > state.budget.max_reports {
        state.reports.pop_front();
    }
    report
}

fn build_integration_nodes(
    reason: &str,
    synthetic: &SyntheticGovernanceReport,
    epistemology: &EpistemologyReport,
) -> Vec<CognitiveIntegrationNode> {
    vec![
        CognitiveIntegrationNode {
            id: "current_task".into(),
            layer: CognitiveLayerKind::Context,
            salience: 0.9,
            confidence: 0.8,
            token_cost: estimate_token_count(reason).max(16),
            summary: compact(reason, 120),
        },
        CognitiveIntegrationNode {
            id: "epistemology".into(),
            layer: CognitiveLayerKind::Epistemology,
            salience: 0.75,
            confidence: epistemology.epistemic_health,
            token_cost: 64,
            summary: format!(
                "claims={} evidence={} conflicts={}",
                epistemology.claims.len(),
                epistemology.evidence.len(),
                epistemology.conflict_sets.len()
            ),
        },
        CognitiveIntegrationNode {
            id: "synthetic_governance".into(),
            layer: CognitiveLayerKind::SyntheticScience,
            salience: 0.72,
            confidence: synthetic.stability,
            token_cost: 72,
            summary: synthetic.prompt_status.clone(),
        },
        CognitiveIntegrationNode {
            id: "deliberation".into(),
            layer: CognitiveLayerKind::Deliberation,
            salience: 0.65,
            confidence: 0.72,
            token_cost: 56,
            summary: "bounded actor review and dissent persistence".into(),
        },
        CognitiveIntegrationNode {
            id: "governance".into(),
            layer: CognitiveLayerKind::Governance,
            salience: 0.64,
            confidence: synthetic.stability,
            token_cost: 48,
            summary: "cybernetic epistemic control signals".into(),
        },
        CognitiveIntegrationNode {
            id: "memory".into(),
            layer: CognitiveLayerKind::Memory,
            salience: 0.58,
            confidence: 0.70,
            token_cost: 48,
            summary: "persistent adaptive cognition substrate".into(),
        },
    ]
}

fn build_integration_edges(nodes: &[CognitiveIntegrationNode]) -> Vec<CognitiveIntegrationEdge> {
    let mut edges = Vec::new();
    for win in nodes.windows(2) {
        edges.push(CognitiveIntegrationEdge {
            from: win[0].id.clone(),
            to: win[1].id.clone(),
            relation: "integrates_with".into(),
            weight: ((win[0].salience + win[1].salience) / 2.0).clamp(0.0, 1.0),
        });
    }
    edges
}
fn build_integration_conflicts(
    nodes: &[CognitiveIntegrationNode],
    epistemology: &EpistemologyReport,
) -> Vec<IntegrationConflict> {
    if epistemology.conflict_sets.is_empty() {
        Vec::new()
    } else {
        vec![IntegrationConflict {
            node_a: nodes.first().map(|n| n.id.clone()).unwrap_or_default(),
            node_b: "epistemology".into(),
            pressure: (epistemology.conflict_sets.len() as f64
                / epistemology.claims.len().max(1) as f64)
                .clamp(0.0, 1.0),
            resolution: "compress conflict summary and prioritize verification".into(),
        }]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchicalActivationEpistemicContextState {
    #[serde(default)]
    pub activation_tree: CognitionActivationTree,
    #[serde(default)]
    pub epistemic_governance: EpistemicContextGovernance,
    #[serde(default)]
    pub routing: HierarchicalContextRouting,
    #[serde(default)]
    pub limits: HierarchicalContextLimits,
    #[serde(default)]
    pub reports: VecDeque<HierarchicalActivationReport>,
}
impl Default for HierarchicalActivationEpistemicContextState {
    fn default() -> Self {
        Self {
            activation_tree: CognitionActivationTree::default(),
            epistemic_governance: EpistemicContextGovernance::default(),
            routing: HierarchicalContextRouting::default(),
            limits: HierarchicalContextLimits::default(),
            reports: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchicalContextLimits {
    pub max_depth: usize,
    pub max_active_branches: usize,
    pub max_prompt_tokens: usize,
    pub min_activation: f64,
    pub max_reports: usize,
}
impl Default for HierarchicalContextLimits {
    fn default() -> Self {
        Self {
            max_depth: 4,
            max_active_branches: 8,
            max_prompt_tokens: 640,
            min_activation: 0.12,
            max_reports: 32,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActivationHierarchyLevel {
    Operational,
    Procedural,
    Semantic,
    Strategic,
    Epistemic,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchicalActivationNode {
    pub id: String,
    pub parent: Option<String>,
    pub level: ActivationHierarchyLevel,
    pub activation: f64,
    pub epistemic_weight: f64,
    pub token_cost: usize,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchicalActivationEdge {
    pub parent: String,
    pub child: String,
    pub propagation: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivationPruningDecision {
    pub node_id: String,
    pub retained: bool,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionActivationTree {
    #[serde(default)]
    pub nodes: Vec<HierarchicalActivationNode>,
    #[serde(default)]
    pub edges: Vec<HierarchicalActivationEdge>,
    #[serde(default)]
    pub pruning: Vec<ActivationPruningDecision>,
    pub active_depth: usize,
    pub total_activation: f64,
}
impl Default for CognitionActivationTree {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            pruning: Vec::new(),
            active_depth: 0,
            total_activation: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EpistemicContextDecisionKind {
    Permit,
    Compress,
    RequireEvidence,
    Quarantine,
    Defer,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicContextDecision {
    pub node_id: String,
    pub kind: EpistemicContextDecisionKind,
    pub confidence: f64,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextContaminationSignal {
    pub node_id: String,
    pub risk: f64,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScopeBoundarySignal {
    pub boundary: String,
    pub leakage_risk: f64,
    pub action: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpistemicContextGovernance {
    #[serde(default)]
    pub decisions: Vec<EpistemicContextDecision>,
    #[serde(default)]
    pub contamination: Vec<ContextContaminationSignal>,
    #[serde(default)]
    pub scope_boundaries: Vec<ScopeBoundarySignal>,
    pub governance_pressure: f64,
}
impl Default for EpistemicContextGovernance {
    fn default() -> Self {
        Self {
            decisions: Vec::new(),
            contamination: Vec::new(),
            scope_boundaries: Vec::new(),
            governance_pressure: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContextRouteKind {
    Direct,
    Compressed,
    EvidenceFirst,
    Deferred,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextRoute {
    pub node_id: String,
    pub route: ContextRouteKind,
    pub allocated_tokens: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchicalContextRouting {
    #[serde(default)]
    pub routes: Vec<ContextRoute>,
    pub routed_tokens: usize,
    pub saved_tokens: usize,
}
impl Default for HierarchicalContextRouting {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            routed_tokens: 0,
            saved_tokens: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HierarchicalActivationReport {
    pub generated_at: DateTime<Utc>,
    pub nodes: usize,
    pub active_depth: usize,
    pub retained: usize,
    pub compressed: usize,
    pub evidence_first: usize,
    pub quarantined: usize,
    pub routed_tokens: usize,
    pub saved_tokens: usize,
    pub governance_pressure: f64,
    pub prompt_status: String,
}

pub fn run_hierarchical_epistemic_context(
    reason: impl Into<String>,
) -> io::Result<HierarchicalActivationReport> {
    let mut store = load_store()?;
    let report = run_hierarchical_epistemic_context_in_store(&mut store, reason.into());
    save_store(&store)?;
    Ok(report)
}

pub fn run_hierarchical_epistemic_context_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> HierarchicalActivationReport {
    let now = Utc::now();
    let context_report =
        run_cognitive_context_economy_in_store(store, format!("hierarchical_context:{reason}"));
    let state = &mut store.operational_state.hierarchical_epistemic_context;
    let mut nodes = build_hierarchical_activation_nodes(&reason, &context_report);
    nodes.sort_by(|a, b| {
        b.activation
            .partial_cmp(&a.activation)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let retained_ids: BTreeSet<String> = nodes
        .iter()
        .filter(|n| n.activation >= state.limits.min_activation)
        .take(state.limits.max_active_branches)
        .map(|n| n.id.clone())
        .collect();
    let pruning: Vec<_> = nodes
        .iter()
        .map(|n| ActivationPruningDecision {
            node_id: n.id.clone(),
            retained: retained_ids.contains(&n.id),
            reason: if retained_ids.contains(&n.id) {
                "activation above salience threshold"
            } else {
                "pruned by activation/context budget"
            }
            .into(),
        })
        .collect();
    let edges = build_hierarchical_activation_edges(&nodes, &retained_ids);
    let active_depth = nodes
        .iter()
        .filter(|n| retained_ids.contains(&n.id))
        .map(|n| match n.level {
            ActivationHierarchyLevel::Operational => 1,
            ActivationHierarchyLevel::Procedural => 2,
            ActivationHierarchyLevel::Semantic => 3,
            ActivationHierarchyLevel::Strategic => 4,
            ActivationHierarchyLevel::Epistemic => 5,
        })
        .max()
        .unwrap_or(0)
        .min(state.limits.max_depth);
    let total_activation = nodes
        .iter()
        .filter(|n| retained_ids.contains(&n.id))
        .map(|n| n.activation)
        .sum::<f64>()
        .clamp(0.0, 8.0);
    let mut decisions = Vec::new();
    let mut contamination = Vec::new();
    let mut boundaries = Vec::new();
    let mut routes = Vec::new();
    let mut routed_tokens = 0usize;
    let mut saved_tokens = 0usize;
    let mut compressed = 0usize;
    let mut evidence_first = 0usize;
    let mut quarantined = 0usize;
    for node in nodes.iter().filter(|n| retained_ids.contains(&n.id)) {
        let contamination_risk = (1.0 - node.epistemic_weight) * node.activation;
        let kind = if contamination_risk > 0.55 {
            quarantined += 1;
            EpistemicContextDecisionKind::Quarantine
        } else if node.epistemic_weight < 0.45 {
            evidence_first += 1;
            EpistemicContextDecisionKind::RequireEvidence
        } else if routed_tokens + node.token_cost > state.limits.max_prompt_tokens {
            compressed += 1;
            EpistemicContextDecisionKind::Compress
        } else {
            EpistemicContextDecisionKind::Permit
        };
        if contamination_risk > 0.35 {
            contamination.push(ContextContaminationSignal {
                node_id: node.id.clone(),
                risk: contamination_risk.clamp(0.0, 1.0),
                source: "low epistemic weight with high activation".into(),
            });
        }
        let route = match kind {
            EpistemicContextDecisionKind::Permit => {
                routed_tokens += node.token_cost;
                ContextRouteKind::Direct
            }
            EpistemicContextDecisionKind::Compress => {
                let ct = (node.token_cost / 3).max(12);
                routed_tokens += ct;
                saved_tokens += node.token_cost.saturating_sub(ct);
                ContextRouteKind::Compressed
            }
            EpistemicContextDecisionKind::RequireEvidence => {
                let ct = (node.token_cost / 4).max(8);
                routed_tokens += ct;
                saved_tokens += node.token_cost.saturating_sub(ct);
                ContextRouteKind::EvidenceFirst
            }
            EpistemicContextDecisionKind::Quarantine | EpistemicContextDecisionKind::Defer => {
                saved_tokens += node.token_cost;
                ContextRouteKind::Deferred
            }
        };
        decisions.push(EpistemicContextDecision { node_id: node.id.clone(), kind, confidence: node.epistemic_weight, reason: "epistemic context governance routed activation by confidence, contamination risk, and token budget".into() });
        routes.push(ContextRoute {
            node_id: node.id.clone(),
            route,
            allocated_tokens: node.token_cost,
        });
    }
    boundaries.push(ScopeBoundarySignal {
        boundary: "task/repo/session".into(),
        leakage_risk: contamination.iter().map(|c| c.risk).fold(0.0, f64::max),
        action: "compress or evidence-gate cross-scope activation".into(),
    });
    let pressure = (contamination.iter().map(|c| c.risk).sum::<f64>()
        / decisions.len().max(1) as f64)
        .clamp(0.0, 1.0);
    state.activation_tree = CognitionActivationTree {
        nodes: nodes.clone(),
        edges,
        pruning,
        active_depth,
        total_activation,
    };
    state.epistemic_governance = EpistemicContextGovernance {
        decisions,
        contamination,
        scope_boundaries: boundaries,
        governance_pressure: pressure,
    };
    state.routing = HierarchicalContextRouting {
        routes,
        routed_tokens,
        saved_tokens,
    };
    let retained = retained_ids.len();
    let report = HierarchicalActivationReport {
        generated_at: now,
        nodes: nodes.len(),
        active_depth,
        retained,
        compressed,
        evidence_first,
        quarantined,
        routed_tokens,
        saved_tokens,
        governance_pressure: pressure,
        prompt_status: format!(
            "Hierarchical context: nodes={} depth={} retained={} compressed={} evidence_first={} quarantined={} routed={} saved={} pressure={:.2}",
            nodes.len(),
            active_depth,
            retained,
            compressed,
            evidence_first,
            quarantined,
            routed_tokens,
            saved_tokens,
            pressure
        ),
    };
    state.reports.push_back(report.clone());
    while state.reports.len() > state.limits.max_reports {
        state.reports.pop_front();
    }
    report
}

fn build_hierarchical_activation_nodes(
    reason: &str,
    context: &ContextEconomyReport,
) -> Vec<HierarchicalActivationNode> {
    vec![
        HierarchicalActivationNode {
            id: "op_current_task".into(),
            parent: None,
            level: ActivationHierarchyLevel::Operational,
            activation: 0.92,
            epistemic_weight: 0.78,
            token_cost: estimate_token_count(reason).max(16),
            summary: compact(reason, 120),
        },
        HierarchicalActivationNode {
            id: "proc_execution_policy".into(),
            parent: Some("op_current_task".into()),
            level: ActivationHierarchyLevel::Procedural,
            activation: 0.78,
            epistemic_weight: 0.74,
            token_cost: 48,
            summary: "test, verify, commit, install workflow".into(),
        },
        HierarchicalActivationNode {
            id: "sem_context_economy".into(),
            parent: Some("proc_execution_policy".into()),
            level: ActivationHierarchyLevel::Semantic,
            activation: context.efficiency.max(0.45),
            epistemic_weight: context.coherence,
            token_cost: 72,
            summary: context.prompt_status.clone(),
        },
        HierarchicalActivationNode {
            id: "strat_token_governance".into(),
            parent: Some("sem_context_economy".into()),
            level: ActivationHierarchyLevel::Strategic,
            activation: 0.66,
            epistemic_weight: 0.70,
            token_cost: 64,
            summary: "allocate cognition by utility per token".into(),
        },
        HierarchicalActivationNode {
            id: "epis_confidence_boundary".into(),
            parent: Some("strat_token_governance".into()),
            level: ActivationHierarchyLevel::Epistemic,
            activation: 0.70,
            epistemic_weight: context.coherence,
            token_cost: 64,
            summary: "evidence gate context before prompt admission".into(),
        },
        HierarchicalActivationNode {
            id: "low_conf_raw_trace".into(),
            parent: Some("epis_confidence_boundary".into()),
            level: ActivationHierarchyLevel::Epistemic,
            activation: 0.42,
            epistemic_weight: 0.30,
            token_cost: 96,
            summary: "raw trace requires compression/evidence".into(),
        },
    ]
}

fn build_hierarchical_activation_edges(
    nodes: &[HierarchicalActivationNode],
    retained: &BTreeSet<String>,
) -> Vec<HierarchicalActivationEdge> {
    nodes
        .iter()
        .filter_map(|n| {
            n.parent.as_ref().and_then(|p| {
                if retained.contains(&n.id) && retained.contains(p) {
                    Some(HierarchicalActivationEdge {
                        parent: p.clone(),
                        child: n.id.clone(),
                        propagation: n.activation,
                    })
                } else {
                    None
                }
            })
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmergentCognitionQualityCoherenceState {
    #[serde(default)]
    pub quality: EmergentCognitionQuality,
    #[serde(default)]
    pub horizon: LongHorizonOperationalCoherence,
    #[serde(default)]
    pub regulation: CoherenceRegulationPolicy,
    #[serde(default)]
    pub reports: VecDeque<EmergentQualityReport>,
}
impl Default for EmergentCognitionQualityCoherenceState {
    fn default() -> Self {
        Self {
            quality: EmergentCognitionQuality::default(),
            horizon: LongHorizonOperationalCoherence::default(),
            regulation: CoherenceRegulationPolicy::default(),
            reports: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoherenceRegulationPolicy {
    pub max_reports: usize,
    pub min_quality: f64,
    pub min_horizon_coherence: f64,
    pub max_drift: f64,
    pub max_prompt_contribution: usize,
}
impl Default for CoherenceRegulationPolicy {
    fn default() -> Self {
        Self {
            max_reports: 32,
            min_quality: 0.55,
            min_horizon_coherence: 0.60,
            max_drift: 0.35,
            max_prompt_contribution: 240,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitionQualityMetric {
    pub name: String,
    pub value: f64,
    pub target: f64,
    pub trend: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmergentBehaviorSignal {
    pub behavior: String,
    pub quality_delta: f64,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityIntervention {
    pub action: String,
    pub intensity: f64,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmergentCognitionQuality {
    #[serde(default)]
    pub metrics: Vec<CognitionQualityMetric>,
    #[serde(default)]
    pub emergent_signals: Vec<EmergentBehaviorSignal>,
    #[serde(default)]
    pub interventions: Vec<QualityIntervention>,
    pub overall_quality: f64,
    pub quality_debt: f64,
}
impl Default for EmergentCognitionQuality {
    fn default() -> Self {
        Self {
            metrics: Vec::new(),
            emergent_signals: Vec::new(),
            interventions: Vec::new(),
            overall_quality: 1.0,
            quality_debt: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HorizonObjective {
    pub id: String,
    pub description: String,
    pub horizon_steps: usize,
    pub coherence: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalContinuityThread {
    pub id: String,
    pub anchors: Vec<String>,
    pub drift: f64,
    pub stable: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoherenceBreakCandidate {
    pub source: String,
    pub risk: f64,
    pub mitigation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LongHorizonOperationalCoherence {
    #[serde(default)]
    pub objectives: Vec<HorizonObjective>,
    #[serde(default)]
    pub continuity_threads: Vec<OperationalContinuityThread>,
    #[serde(default)]
    pub break_candidates: Vec<CoherenceBreakCandidate>,
    pub horizon_coherence: f64,
    pub drift: f64,
}
impl Default for LongHorizonOperationalCoherence {
    fn default() -> Self {
        Self {
            objectives: Vec::new(),
            continuity_threads: Vec::new(),
            break_candidates: Vec::new(),
            horizon_coherence: 1.0,
            drift: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmergentQualityReport {
    pub generated_at: DateTime<Utc>,
    pub quality: f64,
    pub horizon_coherence: f64,
    pub drift: f64,
    pub debt: f64,
    pub interventions: usize,
    pub breaks: usize,
    pub prompt_status: String,
}

pub fn run_emergent_quality_coherence(
    reason: impl Into<String>,
) -> io::Result<EmergentQualityReport> {
    let mut store = load_store()?;
    let report = run_emergent_quality_coherence_in_store(&mut store, reason.into());
    save_store(&store)?;
    Ok(report)
}

pub fn run_emergent_quality_coherence_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> EmergentQualityReport {
    let now = Utc::now();
    let hierarchical =
        run_hierarchical_epistemic_context_in_store(store, format!("quality_coherence:{reason}"));
    let state = &mut store.operational_state.emergent_quality_coherence;
    let activation_quality = (1.0 - hierarchical.governance_pressure).clamp(0.0, 1.0);
    let routing_quality = (hierarchical.routed_tokens as f64
        / (hierarchical.routed_tokens + hierarchical.saved_tokens).max(1) as f64)
        .clamp(0.0, 1.0);
    let depth_quality = (hierarchical.active_depth as f64 / 4.0).clamp(0.0, 1.0);
    let drift = (hierarchical.governance_pressure * 0.55
        + (1.0 - routing_quality) * 0.25
        + (1.0 - depth_quality) * 0.20)
        .clamp(0.0, 1.0);
    let overall = (activation_quality * 0.45
        + routing_quality * 0.25
        + depth_quality * 0.20
        + (1.0 - drift) * 0.10)
        .clamp(0.0, 1.0);
    let debt = (state.regulation.min_quality - overall).max(0.0)
        + (drift - state.regulation.max_drift).max(0.0);
    let metrics = vec![
        CognitionQualityMetric {
            name: "activation_quality".into(),
            value: activation_quality,
            target: 0.75,
            trend: if activation_quality >= 0.75 {
                "stable"
            } else {
                "needs_regulation"
            }
            .into(),
        },
        CognitionQualityMetric {
            name: "routing_quality".into(),
            value: routing_quality,
            target: 0.65,
            trend: if routing_quality >= 0.65 {
                "efficient"
            } else {
                "overcompressed"
            }
            .into(),
        },
        CognitionQualityMetric {
            name: "long_horizon_drift".into(),
            value: drift,
            target: state.regulation.max_drift,
            trend: if drift <= state.regulation.max_drift {
                "bounded"
            } else {
                "rising"
            }
            .into(),
        },
    ];
    let emergent_signals = vec![EmergentBehaviorSignal {
        behavior: "hierarchical context governance affects long-horizon coherence".into(),
        quality_delta: overall - 0.5,
        evidence: hierarchical.prompt_status.clone(),
    }];
    let mut interventions = Vec::new();
    if overall < state.regulation.min_quality {
        interventions.push(QualityIntervention {
            action: "increase evidence-gating and reduce raw context admission".into(),
            intensity: state.regulation.min_quality - overall,
            reason: "emergent quality below target".into(),
        });
    }
    if drift > state.regulation.max_drift {
        interventions.push(QualityIntervention {
            action: "stabilize continuity anchors and compress divergent branches".into(),
            intensity: drift - state.regulation.max_drift,
            reason: "long-horizon drift above policy".into(),
        });
    }
    let horizon_coherence = (1.0 - drift * 0.65 + overall * 0.35).clamp(0.0, 1.0);
    let objectives = vec![HorizonObjective {
        id: "maintain_operational_coherence".into(),
        description: compact(&reason, 120),
        horizon_steps: 8,
        coherence: horizon_coherence,
    }];
    let continuity_threads = vec![OperationalContinuityThread {
        id: "verification_memory_context_thread".into(),
        anchors: vec![
            "test".into(),
            "commit".into(),
            "install".into(),
            "prompt_budget".into(),
        ],
        drift,
        stable: drift <= state.regulation.max_drift,
    }];
    let break_candidates = if horizon_coherence < state.regulation.min_horizon_coherence {
        vec![CoherenceBreakCandidate {
            source: "context pressure or activation contamination".into(),
            risk: 1.0 - horizon_coherence,
            mitigation: "defer low-confidence context and strengthen evidence anchors".into(),
        }]
    } else {
        Vec::new()
    };
    state.quality = EmergentCognitionQuality {
        metrics,
        emergent_signals,
        interventions,
        overall_quality: overall,
        quality_debt: debt,
    };
    state.horizon = LongHorizonOperationalCoherence {
        objectives,
        continuity_threads,
        break_candidates,
        horizon_coherence,
        drift,
    };
    let report = EmergentQualityReport {
        generated_at: now,
        quality: overall,
        horizon_coherence,
        drift,
        debt,
        interventions: state.quality.interventions.len(),
        breaks: state.horizon.break_candidates.len(),
        prompt_status: format!(
            "Emergent quality: quality={:.2} horizon={:.2} drift={:.2} debt={:.2} interventions={} breaks={}",
            overall,
            horizon_coherence,
            drift,
            debt,
            state.quality.interventions.len(),
            state.horizon.break_candidates.len()
        ),
    };
    state.reports.push_back(report.clone());
    while state.reports.len() > state.regulation.max_reports {
        state.reports.pop_front();
    }
    report
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveSubstrateSynthesisState {
    #[serde(default)]
    pub field: CognitiveFieldState,
    #[serde(default)]
    pub attractors: Vec<CognitiveAttractor>,
    #[serde(default)]
    pub optimizer: SubstrateOptimizationState,
    #[serde(default)]
    pub repair: SubstrateRepairState,
    #[serde(default)]
    pub limits: SubstrateSynthesisLimits,
    #[serde(default)]
    pub reports: VecDeque<SubstrateSynthesisReport>,
}
impl Default for CognitiveSubstrateSynthesisState {
    fn default() -> Self {
        Self {
            field: CognitiveFieldState::default(),
            attractors: Vec::new(),
            optimizer: SubstrateOptimizationState::default(),
            repair: SubstrateRepairState::default(),
            limits: SubstrateSynthesisLimits::default(),
            reports: VecDeque::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstrateSynthesisLimits {
    pub max_field_nodes: usize,
    pub max_attractors: usize,
    pub max_repairs: usize,
    pub max_reports: usize,
    pub max_prompt_contribution: usize,
    pub resonance_target: f64,
    pub instability_limit: f64,
}
impl Default for SubstrateSynthesisLimits {
    fn default() -> Self {
        Self {
            max_field_nodes: 16,
            max_attractors: 8,
            max_repairs: 8,
            max_reports: 32,
            max_prompt_contribution: 240,
            resonance_target: 0.70,
            instability_limit: 0.35,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum CognitiveFieldLayer {
    Memory,
    Epistemic,
    Deliberative,
    Scientific,
    Contextual,
    Hierarchical,
    Quality,
    Operational,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveFieldNode {
    pub id: String,
    pub layer: CognitiveFieldLayer,
    pub activation: f64,
    pub coherence: f64,
    pub instability: f64,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveFieldCoupling {
    pub from: String,
    pub to: String,
    pub resonance: f64,
    pub damping: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveFieldState {
    #[serde(default)]
    pub nodes: Vec<CognitiveFieldNode>,
    #[serde(default)]
    pub couplings: Vec<CognitiveFieldCoupling>,
    pub global_resonance: f64,
    pub global_instability: f64,
    pub integration_density: f64,
}
impl Default for CognitiveFieldState {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            couplings: Vec::new(),
            global_resonance: 1.0,
            global_instability: 0.0,
            integration_density: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CognitiveAttractor {
    pub id: String,
    pub layers: Vec<CognitiveFieldLayer>,
    pub strength: f64,
    pub stability: f64,
    pub behavior: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstrateOptimizationAction {
    pub action: String,
    pub expected_gain: f64,
    pub bounded: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstrateOptimizationState {
    #[serde(default)]
    pub actions: Vec<SubstrateOptimizationAction>,
    pub optimization_pressure: f64,
    pub damping_applied: f64,
}
impl Default for SubstrateOptimizationState {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            optimization_pressure: 0.0,
            damping_applied: 0.0,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstrateRepairAction {
    pub target: String,
    pub repair: String,
    pub urgency: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstrateRepairState {
    #[serde(default)]
    pub actions: Vec<SubstrateRepairAction>,
    pub repair_debt: f64,
}
impl Default for SubstrateRepairState {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            repair_debt: 0.0,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubstrateSynthesisReport {
    pub generated_at: DateTime<Utc>,
    pub nodes: usize,
    pub attractors: usize,
    pub resonance: f64,
    pub instability: f64,
    pub density: f64,
    pub optimization_pressure: f64,
    pub repair_debt: f64,
    pub prompt_status: String,
}

pub fn run_cognitive_substrate_synthesis(
    reason: impl Into<String>,
) -> io::Result<SubstrateSynthesisReport> {
    let mut store = load_store()?;
    let report = run_cognitive_substrate_synthesis_in_store(&mut store, reason.into());
    save_store(&store)?;
    Ok(report)
}

pub fn run_cognitive_substrate_synthesis_in_store(
    store: &mut CognitiveStore,
    reason: String,
) -> SubstrateSynthesisReport {
    let now = Utc::now();
    let quality =
        run_emergent_quality_coherence_in_store(store, format!("substrate_synthesis:{reason}"));
    let substrate = &mut store.operational_state.cognitive_substrate_synthesis;
    let nodes = build_cognitive_field_nodes(&reason, &quality);
    let couplings = build_cognitive_field_couplings(&nodes);
    let global_resonance = if couplings.is_empty() {
        1.0
    } else {
        couplings.iter().map(|c| c.resonance).sum::<f64>() / couplings.len() as f64
    };
    let global_instability =
        nodes.iter().map(|n| n.instability).sum::<f64>() / nodes.len().max(1) as f64;
    let density = couplings.len() as f64 / (nodes.len().max(1) * nodes.len().max(1)) as f64;
    let mut attractors = derive_cognitive_attractors(&nodes, &quality);
    attractors.sort_by(|a, b| {
        b.strength
            .partial_cmp(&a.strength)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    attractors.truncate(substrate.limits.max_attractors);
    let optimization_pressure = ((substrate.limits.resonance_target - global_resonance).max(0.0)
        + (global_instability - substrate.limits.instability_limit).max(0.0))
    .clamp(0.0, 1.0);
    let mut opt_actions = Vec::new();
    if global_resonance < substrate.limits.resonance_target {
        opt_actions.push(SubstrateOptimizationAction {
            action: "increase cross-layer summary coupling".into(),
            expected_gain: substrate.limits.resonance_target - global_resonance,
            bounded: true,
        });
    }
    if global_instability > substrate.limits.instability_limit {
        opt_actions.push(SubstrateOptimizationAction {
            action: "apply instability damping and defer noisy activations".into(),
            expected_gain: global_instability - substrate.limits.instability_limit,
            bounded: true,
        });
    }
    let repair_actions: Vec<_> = nodes
        .iter()
        .filter(|n| n.instability > substrate.limits.instability_limit)
        .take(substrate.limits.max_repairs)
        .map(|n| SubstrateRepairAction {
            target: n.id.clone(),
            repair: "compress, evidence-gate, and re-anchor to continuity thread".into(),
            urgency: n.instability,
        })
        .collect();
    let repair_debt = repair_actions
        .iter()
        .map(|r| r.urgency)
        .sum::<f64>()
        .clamp(0.0, 1.0);
    substrate.field = CognitiveFieldState {
        nodes: nodes.clone(),
        couplings,
        global_resonance,
        global_instability,
        integration_density: density,
    };
    substrate.attractors = attractors;
    substrate.optimizer = SubstrateOptimizationState {
        actions: opt_actions,
        optimization_pressure,
        damping_applied: global_instability.min(substrate.limits.instability_limit),
    };
    substrate.repair = SubstrateRepairState {
        actions: repair_actions,
        repair_debt,
    };
    let report = SubstrateSynthesisReport {
        generated_at: now,
        nodes: substrate.field.nodes.len(),
        attractors: substrate.attractors.len(),
        resonance: global_resonance,
        instability: global_instability,
        density,
        optimization_pressure,
        repair_debt,
        prompt_status: format!(
            "Substrate synthesis: nodes={} attractors={} resonance={:.2} instability={:.2} density={:.2} opt={:.2} repair={:.2}",
            substrate.field.nodes.len(),
            substrate.attractors.len(),
            global_resonance,
            global_instability,
            density,
            optimization_pressure,
            repair_debt
        ),
    };
    substrate.reports.push_back(report.clone());
    while substrate.reports.len() > substrate.limits.max_reports {
        substrate.reports.pop_front();
    }
    report
}

fn build_cognitive_field_nodes(
    reason: &str,
    quality: &EmergentQualityReport,
) -> Vec<CognitiveFieldNode> {
    vec![
        CognitiveFieldNode {
            id: "memory_substrate".into(),
            layer: CognitiveFieldLayer::Memory,
            activation: 0.70,
            coherence: 0.72,
            instability: 0.12,
            summary: "persistent adaptive memory".into(),
        },
        CognitiveFieldNode {
            id: "epistemic_substrate".into(),
            layer: CognitiveFieldLayer::Epistemic,
            activation: 0.74,
            coherence: quality.horizon_coherence,
            instability: quality.drift,
            summary: "truth maintenance and evidence governance".into(),
        },
        CognitiveFieldNode {
            id: "deliberative_substrate".into(),
            layer: CognitiveFieldLayer::Deliberative,
            activation: 0.68,
            coherence: 0.70,
            instability: quality.debt.min(1.0),
            summary: "bounded internal debate".into(),
        },
        CognitiveFieldNode {
            id: "scientific_substrate".into(),
            layer: CognitiveFieldLayer::Scientific,
            activation: 0.72,
            coherence: quality.quality,
            instability: (1.0 - quality.quality).clamp(0.0, 1.0),
            summary: "hypothesis and experiment loops".into(),
        },
        CognitiveFieldNode {
            id: "context_substrate".into(),
            layer: CognitiveFieldLayer::Contextual,
            activation: 0.76,
            coherence: quality.horizon_coherence,
            instability: quality.drift,
            summary: compact(reason, 120),
        },
        CognitiveFieldNode {
            id: "quality_substrate".into(),
            layer: CognitiveFieldLayer::Quality,
            activation: 0.80,
            coherence: quality.quality,
            instability: quality.debt.min(1.0),
            summary: quality.prompt_status.clone(),
        },
    ]
}

fn build_cognitive_field_couplings(nodes: &[CognitiveFieldNode]) -> Vec<CognitiveFieldCoupling> {
    let mut out = Vec::new();
    for win in nodes.windows(2) {
        let resonance = ((win[0].coherence + win[1].coherence) / 2.0
            - (win[0].instability + win[1].instability) * 0.15)
            .clamp(0.0, 1.0);
        out.push(CognitiveFieldCoupling {
            from: win[0].id.clone(),
            to: win[1].id.clone(),
            resonance,
            damping: ((win[0].instability + win[1].instability) / 2.0).clamp(0.0, 1.0),
        });
    }
    out
}
fn derive_cognitive_attractors(
    nodes: &[CognitiveFieldNode],
    quality: &EmergentQualityReport,
) -> Vec<CognitiveAttractor> {
    vec![
        CognitiveAttractor {
            id: "verify_before_commit".into(),
            layers: vec![
                CognitiveFieldLayer::Operational,
                CognitiveFieldLayer::Quality,
            ],
            strength: 0.82,
            stability: quality.horizon_coherence,
            behavior: "validate, commit, push, install".into(),
        },
        CognitiveAttractor {
            id: "evidence_gated_context".into(),
            layers: vec![
                CognitiveFieldLayer::Epistemic,
                CognitiveFieldLayer::Contextual,
            ],
            strength: 0.78,
            stability: quality.quality,
            behavior: "admit context by evidence and token utility".into(),
        },
        CognitiveAttractor {
            id: "bounded_self_repair".into(),
            layers: nodes.iter().map(|n| n.layer.clone()).collect(),
            strength: (1.0 - quality.drift).clamp(0.0, 1.0),
            stability: (1.0 - quality.debt).clamp(0.0, 1.0),
            behavior: "damp instability and repair coherence breaks".into(),
        },
    ]
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

    #[test]
    fn observable_snapshot_renders_layers_and_clusters() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode observable cognition graph rendering"),
        );
        upsert_node_in_store(
            &mut store,
            upsert(".kcode observable cognition replay export"),
        );
        evolve_store(&mut store);
        let snapshot = observable_snapshot_from_store(
            &store,
            RenderOptions {
                layers: vec![
                    ObservationLayer::Summary,
                    ObservationLayer::Clusters,
                    ObservationLayer::Graph,
                ],
                token_budget: 600,
                include_replay: true,
                include_graph: true,
            },
        );
        assert!(!snapshot.frames.is_empty());
        assert!(!snapshot.clusters.is_empty());
        assert!(snapshot.stability_score >= 0.0 && snapshot.stability_score <= 1.0);
        assert!(render_snapshot_markdown(&snapshot).contains("Observable adaptive cognition"));
    }

    #[test]
    fn sideband_render_is_ctx_like_and_bounded() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode sideband observable cognition"));
        evolve_store(&mut store);
        let snapshot = observable_snapshot_from_store(&store, RenderOptions::default());
        let md = render_snapshot_markdown(&snapshot);
        assert!(md.len() < 10_000);
        assert!(
            snapshot
                .frames
                .iter()
                .all(|frame| frame.token_count_estimate <= 1_600)
        );
    }

    #[test]
    fn operational_cycle_schedules_executes_and_snapshots() {
        let mut store = CognitiveStore::default();
        store.operational_state.policy.max_tasks_per_cycle = 4;
        upsert_node_in_store(
            &mut store,
            upsert(".kcode operational cognition runtime scheduling"),
        );
        evolve_store(&mut store);
        let report = run_operational_cycle_in_store(&mut store, "test cycle".to_string());
        assert!(!report.scheduled_tasks.is_empty());
        assert!(!report.executed_tasks.is_empty());
        assert!(report.snapshot.is_some());
        assert!(store.operational_state.last_cycle_at.is_some());
        assert!(!store.operational_state.cycle_history.is_empty());
    }

    #[test]
    fn operational_mode_repairs_unstable_contradictions() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert("always remember .kcode operational memory"),
        );
        upsert_node_in_store(
            &mut store,
            upsert("never remember .kcode operational memory"),
        );
        evolve_store(&mut store);
        store.operational_state.policy.stability_floor = 0.99;
        let report =
            run_operational_cycle_in_store(&mut store, "repair contradictions".to_string());
        assert!(matches!(report.mode, OperationalMode::Repair));
        assert!(
            report
                .scheduled_tasks
                .iter()
                .any(|task| matches!(task.kind, OperationalTaskKind::ContradictionAudit))
        );
    }

    #[test]
    fn operational_json_export_shape_is_serializable() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode runtime export serialization"));
        let report = run_operational_cycle_in_store(&mut store, "json test".to_string());
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("entropy"));
        assert!(json.contains("scheduled_tasks"));
    }

    #[test]
    fn execution_governor_builds_and_applies_safe_plan() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode execution governor adaptive planning"),
        );
        evolve_store(&mut store);
        let report = run_execution_governor_in_store(&mut store, "test governor".to_string());
        assert!(!report.plan.actions.is_empty());
        assert!(!report.applied_results.is_empty());
        assert!(!store.operational_state.execution_plans.is_empty());
        assert!(!store.operational_state.governor_reports.is_empty());
    }

    #[test]
    fn execution_governor_prefers_repair_for_unhealthy_state() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert("always remember .kcode execution governor"),
        );
        upsert_node_in_store(
            &mut store,
            upsert("never remember .kcode execution governor"),
        );
        evolve_store(&mut store);
        store.operational_state.policy.stability_floor = 0.99;
        let report = run_execution_governor_in_store(&mut store, "repair".to_string());
        assert!(matches!(
            report.plan.strategy,
            ExecutionStrategy::RepairFirst
        ));
        assert!(report.plan.actions.iter().any(|action| matches!(
            action.kind,
            ExecutionActionKind::AuditContradictions | ExecutionActionKind::Reflect
        )));
    }

    #[test]
    fn execution_plan_blocks_actions_over_risk_budget() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode risk budget execution governor"));
        evolve_store(&mut store);
        let mut plan = build_execution_plan(&store, "risk");
        plan.risk_budget = 0.01;
        let (applied, blocked) = apply_execution_plan(&mut store, &plan);
        assert!(applied.len() <= 1);
        assert!(!blocked.is_empty() || plan.actions.iter().all(|a| a.risk <= 0.01));
    }

    #[test]
    fn procedural_runtime_synthesizes_and_selects_procedures() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode procedural intelligence runtime memory"),
        );
        upsert_node_in_store(
            &mut store,
            upsert(".kcode procedural intelligence runtime testing"),
        );
        evolve_store(&mut store);
        let report =
            run_procedural_runtime_in_store(&mut store, "procedural intelligence".to_string());
        assert!(report.procedure_count > 0);
        assert!(report.active_procedure_count > 0);
        assert!(!report.predictions.is_empty());
        assert!(
            !store
                .operational_state
                .procedural_runtime
                .reports
                .is_empty()
        );
    }

    #[test]
    fn procedural_runtime_quarantines_failing_procedures() {
        let mut store = CognitiveStore::default();
        let id = upsert_node_in_store(&mut store, upsert(".kcode failing procedure lineage"));
        evolve_store(&mut store);
        run_procedural_runtime_in_store(&mut store, "failing procedure".to_string());
        for _ in 0..4 {
            store.execution_signals.push(ExecutionSignal {
                node_id: id.clone(),
                recorded_at: Utc::now(),
                success: false,
                delta: -0.2,
                source: "test".to_string(),
                summary: "failed".to_string(),
            });
        }
        let report = run_procedural_runtime_in_store(&mut store, "failing procedure".to_string());
        assert!(report.procedure_count > 0);
        assert!(
            store
                .operational_state
                .procedural_runtime
                .procedures
                .values()
                .any(|p| matches!(p.status, ProcedureStatus::Quarantined))
        );
    }

    #[test]
    fn procedural_safety_notes_enforce_doctrine() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode safe procedural runtime"));
        let report =
            run_procedural_runtime_in_store(&mut store, "safe procedural runtime".to_string());
        assert!(
            report
                .safety_notes
                .iter()
                .any(|note| note.contains("dry-run"))
        );
        assert!(
            report
                .safety_notes
                .iter()
                .any(|note| note.contains("tests"))
        );
    }

    #[test]
    fn cognitive_fabric_reports_environment_subsystems_and_forecasts() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode cognitive fabric environment subsystem"),
        );
        run_procedural_runtime_in_store(&mut store, "fabric setup".to_string());
        let report = run_cognitive_fabric_in_store(&mut store, "fabric".to_string());
        assert!(!report.subsystems.is_empty());
        assert!(!report.latent_states.is_empty());
        assert_eq!(report.forecasts.len(), 3);
        assert!(!report.arbitration.selected_subsystems.is_empty());
        assert!(!store.operational_state.cognitive_fabric.reports.is_empty());
    }

    #[test]
    fn cognitive_fabric_detects_compression_and_reflection_needs() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert("always remember .kcode fabric contradiction"),
        );
        upsert_node_in_store(
            &mut store,
            upsert("never remember .kcode fabric contradiction"),
        );
        evolve_store(&mut store);
        let report = run_cognitive_fabric_in_store(&mut store, "fabric contradiction".to_string());
        assert!(
            report
                .latent_states
                .iter()
                .any(|state| state.name == "needs_reflection")
        );
        assert!(report.environment.contradiction_pressure >= 0.0);
    }

    #[test]
    fn cognitive_fabric_arbitration_has_rationale() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode fabric arbitration doctrine"));
        let report = run_cognitive_fabric_in_store(&mut store, "arbitrate".to_string());
        assert!(report.arbitration.rationale.contains("top_latent"));
        assert!(report.summary.contains("fabric stability"));
    }

    #[test]
    fn distributed_fabric_registers_nodes_routes_and_consensus() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode distributed fabric consensus routing"),
        );
        let report = run_distributed_fabric_in_store(&mut store, "distributed fabric".to_string());
        assert!(report.nodes.len() >= 6);
        assert!(!report.routes.is_empty());
        assert!(!report.consensus.is_empty());
        assert!(report.quorum_health > 0.0);
        assert!(
            !store
                .operational_state
                .distributed_fabric
                .sync_history
                .is_empty()
        );
    }

    #[test]
    fn distributed_fabric_routes_capabilities_by_score() {
        let mut store = CognitiveStore::default();
        let report = run_distributed_fabric_in_store(&mut store, "routing".to_string());
        assert!(report.routes.windows(2).all(|w| w[0].score >= w[1].score));
        assert!(report.routes.iter().any(|r| r.capability == "test"));
        assert!(report.routes.iter().any(|r| r.capability == "retrieve"));
    }

    #[test]
    fn distributed_fabric_quorum_health_detects_unhealthy_nodes() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "a".to_string(),
            FabricNode {
                id: "a".to_string(),
                kind: FabricNodeKind::LocalRuntime,
                capabilities: vec!["orchestrate".to_string()],
                health: 0.1,
                load: 1.0,
                last_seen_at: Utc::now(),
            },
        );
        nodes.insert(
            "b".to_string(),
            FabricNode {
                id: "b".to_string(),
                kind: FabricNodeKind::VerifierSubsystem,
                capabilities: vec!["test".to_string()],
                health: 0.2,
                load: 1.0,
                last_seen_at: Utc::now(),
            },
        );
        assert!(compute_quorum_health(&nodes) < 0.45);
    }

    #[test]
    fn strategic_civilization_runtime_builds_doctrines_resources_and_proposals() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode strategic civilization doctrine resource"),
        );
        run_distributed_fabric_in_store(&mut store, "seed fabric".to_string());
        let report =
            run_strategic_civilization_runtime_in_store(&mut store, "strategic".to_string());
        assert!(report.doctrines.len() >= 5);
        assert!(report.resources.len() >= 5);
        assert!(!report.syntheses.is_empty());
        assert!(!report.simulations.is_empty());
        assert!(!report.horizons.is_empty());
        assert!(report.civilization_score > 0.0);
    }

    #[test]
    fn strategic_civilization_updates_federation_and_identity() {
        let mut store = CognitiveStore::default();
        run_distributed_fabric_in_store(&mut store, "fabric".to_string());
        let report =
            run_strategic_civilization_runtime_in_store(&mut store, "identity".to_string());
        assert!(!report.federation.is_empty());
        assert!(report.identity.iter().any(|i| i.id == "identity-kcode"));
    }

    #[test]
    fn strategic_civilization_generates_archaeology_after_reports() {
        let mut store = CognitiveStore::default();
        run_strategic_civilization_runtime_in_store(&mut store, "first".to_string());
        let report = run_strategic_civilization_runtime_in_store(&mut store, "second".to_string());
        assert!(!report.archaeology.is_empty());
        assert!(report.summary.contains("civilization_score"));
    }

    #[test]
    fn civilization_os_builds_governance_and_continuity() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode civilization os governance"));
        run_distributed_fabric_in_store(&mut store, "fabric".to_string());
        let report = run_civilization_os_in_store(&mut store, "civilization os".to_string());
        assert!(report.institutions.len() >= 6);
        assert!(report.laws.len() >= 5);
        assert!(!report.scenarios.is_empty());
        assert!(!report.continuity.is_empty());
        assert!(report.os_health > 0.0);
    }

    #[test]
    fn civilization_os_records_precedents_and_civic_memory() {
        let mut store = CognitiveStore::default();
        run_civilization_os_in_store(&mut store, "first governance".to_string());
        let report = run_civilization_os_in_store(&mut store, "second governance".to_string());
        assert!(report.precedents.len() >= 2);
        assert!(report.civic_memory.len() >= 2);
        assert!(report.summary.contains("civilization_os"));
    }

    #[test]
    fn civilization_os_diplomacy_tracks_federation_peers() {
        let mut store = CognitiveStore::default();
        run_distributed_fabric_in_store(&mut store, "peers".to_string());
        let report = run_civilization_os_in_store(&mut store, "diplomacy".to_string());
        assert!(!report.diplomacy.is_empty());
        assert!(report
            .diplomacy
            .iter()
            .all(|stance| ["cooperate", "verify", "isolate"].contains(&stance.posture.as_str())));
    }

    #[test]
    fn sovereign_ecosystem_builds_invariants_and_economy() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(
            &mut store,
            upsert(".kcode sovereign ecosystem constitution economy"),
        );
        let report = run_sovereign_ecosystem_in_store(&mut store, "sovereign".to_string());
        assert!(report.invariants.len() >= 5);
        assert!(report.continuity.len() >= 3);
        assert!(report.compression_laws.len() >= 3);
        assert!(report.currencies.len() >= 4);
        assert!(report.sovereignty_score > 0.0);
    }

    #[test]
    fn sovereign_ecosystem_creates_virtual_shards_mythos_and_relations() {
        let mut store = CognitiveStore::default();
        run_sovereign_ecosystem_in_store(&mut store, "virtualization mythos".to_string());
        let report = run_sovereign_ecosystem_in_store(&mut store, "relations".to_string());
        assert!(report.runtime_shards.iter().all(|s| s.replayable));
        assert!(report.mythos.iter().all(|m| m.grounded));
        assert!(!report.relations.is_empty());
    }

    #[test]
    fn sovereign_ecosystem_reports_are_persisted() {
        let mut store = CognitiveStore::default();
        run_sovereign_ecosystem_in_store(&mut store, "first".to_string());
        run_sovereign_ecosystem_in_store(&mut store, "second".to_string());
        assert!(store.operational_state.sovereign_ecosystem.reports.len() >= 2);
    }

    #[test]
    fn hardening_runtime_collects_anchors_and_pulse() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode hardening runtime anchors"));
        run_sovereign_ecosystem_in_store(&mut store, "seed".to_string());
        let report = run_hardening_runtime_in_store(&mut store, "hardening".to_string());
        assert!(!report.reality_anchors.is_empty());
        assert!(report.pulse.store_size > 0);
        assert!(report.maturity_score > 0.0);
    }

    #[test]
    fn hardening_runtime_deactivates_bad_nodes() {
        let mut store = CognitiveStore::default();
        let id = upsert_node_in_store(&mut store, upsert(".kcode bad contradicted node"));
        if let Some(node) = store.nodes.get_mut(&id) {
            node.weights.contradiction = 0.95;
            node.weights.confidence = 0.1;
        }
        let report = run_hardening_runtime_in_store(&mut store, "gc".to_string());
        assert!(report.garbage_collection.iter().any(|d| d.target_id == id));
        assert!(!store.nodes[&id].active);
    }

    #[test]
    fn hardening_runtime_flags_missing_maturity_as_corrective_not_delusion() {
        let mut store = CognitiveStore::default();
        let report = run_hardening_runtime_in_store(&mut store, "fresh".to_string());
        assert!(
            report
                .delusion_checks
                .iter()
                .any(|c| c.claim.contains("all cognition layers") && !c.grounded)
        );
        assert!(report.immune_responses.iter().any(|r| r.quarantined));
    }

    #[test]
    fn reality_coupling_collects_telemetry_claims_and_world_state() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode reality coupling verification"));
        let report = run_reality_coupling_in_store(&mut store, "reality".to_string());
        assert!(!report.telemetry.is_empty());
        assert!(!report.claims.is_empty());
        assert!(!report.world_state.is_empty());
        assert!(report.coupling_score > 0.0);
    }

    #[test]
    fn reality_coupling_calibrates_prediction_error_over_samples() {
        let mut store = CognitiveStore::default();
        run_reality_coupling_in_store(&mut store, "sample one".to_string());
        let report = run_reality_coupling_in_store(&mut store, "sample two".to_string());
        assert!(report.calibrations.iter().any(|c| c.sample_count >= 2));
    }

    #[test]
    fn reality_coupling_surfaces_corrective_actions_for_false_claims() {
        let store = CognitiveStore::default();
        let hardening = HardeningReport {
            generated_at: Utc::now(),
            reality_anchors: Vec::new(),
            ontology_checks: Vec::new(),
            garbage_collection: Vec::new(),
            pulse: NervousSystemPulse {
                pulsed_at: Utc::now(),
                heartbeat_ok: true,
                store_size: 0,
                pending_queues: 0,
                warning: None,
            },
            delusion_checks: Vec::new(),
            immune_responses: Vec::new(),
            maturity_score: 0.0,
            summary: String::new(),
        };
        let claims = verify_runtime_claims(&store, &[], &hardening);
        assert!(
            claims
                .iter()
                .any(|c| !c.verified && !c.corrective_action.is_empty())
        );
    }

    #[test]
    fn epistemology_builds_claims_evidence_and_health() {
        let mut store = CognitiveStore::default();
        upsert_node_in_store(&mut store, upsert(".kcode epistemology claim evidence"));
        let report = run_epistemology_in_store(&mut store, "epistemology".to_string());
        assert!(!report.claims.is_empty());
        assert!(!report.evidence.is_empty());
        assert!(report.epistemic_health >= 0.0);
    }

    #[test]
    fn epistemology_revises_low_confidence_claims() {
        let mut store = CognitiveStore::default();
        let now = Utc::now();
        store.operational_state.epistemology.claims.insert(
            "bad".to_string(),
            EpistemicClaim {
                id: "bad".to_string(),
                statement: "unsupported low confidence claim".to_string(),
                status: EpistemicStatus::Hypothesis,
                confidence: 0.2,
                evidence_ids: Vec::new(),
                contradiction_ids: Vec::new(),
                last_revised_at: now,
            },
        );
        let wrong = detect_wrongness(&mut store);
        revise_beliefs(&mut store, &wrong);
        assert!(!store.operational_state.epistemology.revisions.is_empty());
        assert!(matches!(
            store.operational_state.epistemology.claims["bad"].status,
            EpistemicStatus::Deprecated
        ));
    }

    #[test]
    fn epistemology_tracks_source_reliability() {
        let mut store = CognitiveStore::default();
        run_epistemology_in_store(&mut store, "first".to_string());
        let report = run_epistemology_in_store(&mut store, "second".to_string());
        assert!(report.reliabilities.iter().any(|r| r.observations > 0));
    }

    #[test]
    fn substrate_synthesis_builds_field_and_attractors() {
        let mut store = CognitiveStore::default();
        let report =
            run_cognitive_substrate_synthesis_in_store(&mut store, "substrate synthesis".into());
        let substrate = &store.operational_state.cognitive_substrate_synthesis;
        assert!(!substrate.field.nodes.is_empty());
        assert!(!substrate.field.couplings.is_empty());
        assert!(!substrate.attractors.is_empty());
        assert_eq!(report.nodes, substrate.field.nodes.len());
    }

    #[test]
    fn substrate_optimizer_applies_bounded_actions_under_strict_targets() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .cognitive_substrate_synthesis
            .limits
            .resonance_target = 0.99;
        store
            .operational_state
            .cognitive_substrate_synthesis
            .limits
            .instability_limit = 0.01;
        run_cognitive_substrate_synthesis_in_store(&mut store, "strict substrate optimizer".into());
        let optimizer = &store
            .operational_state
            .cognitive_substrate_synthesis
            .optimizer;
        assert!(!optimizer.actions.is_empty());
        assert!(optimizer.actions.iter().all(|a| a.bounded));
    }

    #[test]
    fn substrate_repair_records_debt_for_unstable_nodes() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .cognitive_substrate_synthesis
            .limits
            .instability_limit = 0.01;
        run_cognitive_substrate_synthesis_in_store(&mut store, "repair debt".into());
        let repair = &store.operational_state.cognitive_substrate_synthesis.repair;
        assert!(!repair.actions.is_empty());
        assert!(repair.repair_debt >= 0.0);
    }

    #[test]
    fn substrate_prompt_status_is_compact() {
        let mut store = CognitiveStore::default();
        let report =
            run_cognitive_substrate_synthesis_in_store(&mut store, "compact prompt".into());
        assert!(
            report.prompt_status.len()
                <= store
                    .operational_state
                    .cognitive_substrate_synthesis
                    .limits
                    .max_prompt_contribution
        );
    }

    #[test]
    fn substrate_synthesis_persistence_compatibility_defaults() {
        let json = serde_json::to_string(&CognitiveStore::default()).unwrap();
        let restored: CognitiveStore = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .operational_state
                .cognitive_substrate_synthesis
                .limits
                .max_attractors,
            8
        );
        assert_eq!(
            restored
                .operational_state
                .cognitive_substrate_synthesis
                .field
                .global_instability,
            0.0
        );
    }

    #[test]
    fn emergent_quality_generates_metrics_and_report() {
        let mut store = CognitiveStore::default();
        let report = run_emergent_quality_coherence_in_store(
            &mut store,
            "emergent cognition quality".into(),
        );
        let state = &store.operational_state.emergent_quality_coherence;
        assert!(!state.quality.metrics.is_empty());
        assert!((0.0..=1.0).contains(&report.quality));
        assert!(report.prompt_status.len() <= state.regulation.max_prompt_contribution);
    }

    #[test]
    fn long_horizon_coherence_tracks_drift_and_threads() {
        let mut store = CognitiveStore::default();
        run_emergent_quality_coherence_in_store(&mut store, "long horizon coherence".into());
        let horizon = &store.operational_state.emergent_quality_coherence.horizon;
        assert!(!horizon.objectives.is_empty());
        assert!(!horizon.continuity_threads.is_empty());
        assert!((0.0..=1.0).contains(&horizon.drift));
        assert!((0.0..=1.0).contains(&horizon.horizon_coherence));
    }

    #[test]
    fn quality_regulation_intervenes_when_thresholds_are_strict() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .emergent_quality_coherence
            .regulation
            .min_quality = 0.99;
        store
            .operational_state
            .emergent_quality_coherence
            .regulation
            .max_drift = 0.01;
        run_emergent_quality_coherence_in_store(&mut store, "strict quality policy".into());
        assert!(
            !store
                .operational_state
                .emergent_quality_coherence
                .quality
                .interventions
                .is_empty()
        );
    }

    #[test]
    fn coherence_breaks_surface_under_high_horizon_requirement() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .emergent_quality_coherence
            .regulation
            .min_horizon_coherence = 1.01;
        run_emergent_quality_coherence_in_store(&mut store, "coherence break detection".into());
        let horizon = &store.operational_state.emergent_quality_coherence.horizon;
        assert!(!horizon.break_candidates.is_empty());
    }

    #[test]
    fn emergent_quality_persistence_compatibility_defaults() {
        let json = serde_json::to_string(&CognitiveStore::default()).unwrap();
        let restored: CognitiveStore = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .operational_state
                .emergent_quality_coherence
                .regulation
                .max_reports,
            32
        );
        assert_eq!(
            restored
                .operational_state
                .emergent_quality_coherence
                .quality
                .overall_quality,
            1.0
        );
    }

    #[test]
    fn hierarchical_activation_respects_depth_and_branch_limits() {
        let mut store = CognitiveStore::default();
        let report = run_hierarchical_epistemic_context_in_store(
            &mut store,
            "hierarchical cognition activation".into(),
        );
        let state = &store.operational_state.hierarchical_epistemic_context;
        assert!(report.active_depth <= state.limits.max_depth);
        assert!(report.retained <= state.limits.max_active_branches);
        assert!(report.routed_tokens <= state.limits.max_prompt_tokens);
    }

    #[test]
    fn epistemic_context_governance_gates_low_confidence_activation() {
        let mut store = CognitiveStore::default();
        run_hierarchical_epistemic_context_in_store(&mut store, "epistemic governance".into());
        let gov = &store
            .operational_state
            .hierarchical_epistemic_context
            .epistemic_governance;
        assert!(!gov.decisions.is_empty());
        assert!(gov.decisions.iter().any(|d| matches!(
            d.kind,
            EpistemicContextDecisionKind::RequireEvidence
                | EpistemicContextDecisionKind::Quarantine
                | EpistemicContextDecisionKind::Compress
                | EpistemicContextDecisionKind::Permit
        )));
        assert!((0.0..=1.0).contains(&gov.governance_pressure));
    }

    #[test]
    fn hierarchical_context_records_pruning_and_routes() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .hierarchical_epistemic_context
            .limits
            .max_active_branches = 3;
        run_hierarchical_epistemic_context_in_store(&mut store, "pruning route test".into());
        let state = &store.operational_state.hierarchical_epistemic_context;
        assert!(state.activation_tree.pruning.iter().any(|p| !p.retained));
        assert!(!state.routing.routes.is_empty());
    }

    #[test]
    fn hierarchical_context_surfaces_scope_boundaries_and_contamination() {
        let mut store = CognitiveStore::default();
        run_hierarchical_epistemic_context_in_store(&mut store, "contamination boundary".into());
        let gov = &store
            .operational_state
            .hierarchical_epistemic_context
            .epistemic_governance;
        assert!(!gov.scope_boundaries.is_empty());
        assert!(
            gov.contamination
                .iter()
                .all(|c| c.risk >= 0.0 && c.risk <= 1.0)
        );
    }

    #[test]
    fn hierarchical_epistemic_context_persistence_compatibility_defaults() {
        let json = serde_json::to_string(&CognitiveStore::default()).unwrap();
        let restored: CognitiveStore = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .operational_state
                .hierarchical_epistemic_context
                .limits
                .max_depth,
            4
        );
        assert_eq!(
            restored
                .operational_state
                .hierarchical_epistemic_context
                .activation_tree
                .total_activation,
            0.0
        );
    }

    #[test]
    fn context_economy_allocates_within_budget() {
        let mut store = CognitiveStore::default();
        let report = run_cognitive_context_economy_in_store(
            &mut store,
            "adaptive context economy budget".into(),
        );
        let state = &store.operational_state.cognitive_context_economy;
        assert!(report.spent_tokens <= state.budget.max_prompt_tokens);
        assert!(state.context_economy.allocations.len() <= state.budget.max_context_items);
        assert!(report.prompt_status.len() <= 240);
    }

    #[test]
    fn cognitive_integration_builds_mesh_and_coherence() {
        let mut store = CognitiveStore::default();
        run_cognitive_context_economy_in_store(&mut store, "integration mesh".into());
        let mesh = &store
            .operational_state
            .cognitive_context_economy
            .integration;
        assert!(!mesh.nodes.is_empty());
        assert!(!mesh.edges.is_empty());
        assert!((0.0..=1.0).contains(&mesh.coherence));
    }

    #[test]
    fn context_market_compresses_or_defers_over_budget_items() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .cognitive_context_economy
            .budget
            .max_prompt_tokens = 80;
        let report = run_cognitive_context_economy_in_store(&mut store, "very long context item that should force compression because the budget is deliberately tiny and must remain bounded".into());
        assert!(report.compressed + report.deferred > 0);
        assert!(
            store
                .operational_state
                .cognitive_context_economy
                .context_economy
                .efficiency
                >= 0.0
        );
    }

    #[test]
    fn context_economy_records_recall_trace_and_debt() {
        let mut store = CognitiveStore::default();
        store
            .operational_state
            .cognitive_context_economy
            .budget
            .max_prompt_tokens = 40;
        run_cognitive_context_economy_in_store(&mut store, "debt trace".into());
        let economy = &store
            .operational_state
            .cognitive_context_economy
            .context_economy;
        assert!(!economy.recall_trace.is_empty());
        assert!(economy.debt.iter().all(|d| d.debt >= 0.0));
    }

    #[test]
    fn cognitive_context_economy_persistence_compatibility_defaults() {
        let json = serde_json::to_string(&CognitiveStore::default()).unwrap();
        let restored: CognitiveStore = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .operational_state
                .cognitive_context_economy
                .budget
                .max_prompt_tokens,
            512
        );
        assert_eq!(
            restored
                .operational_state
                .cognitive_context_economy
                .integration
                .coherence,
            1.0
        );
    }

    #[test]
    fn synthetic_governance_runs_bounded_ecosystem_cycles() {
        let mut store = CognitiveStore::default();
        let report = run_synthetic_scientific_governance_in_store(
            &mut store,
            "synthetic scientific cognition".into(),
        );
        let sg = &store.operational_state.synthetic_governance;
        assert!(report.cycles <= sg.safety.max_ecosystem_cycles);
        assert!(sg.ecosystem.proposals.len() <= sg.safety.max_agent_proposals);
        assert!(report.prompt_status.len() <= sg.safety.max_prompt_contribution);
    }

    #[test]
    fn cybernetic_epistemic_governor_surfaces_control_actions() {
        let mut store = CognitiveStore::default();
        run_synthetic_scientific_governance_in_store(
            &mut store,
            "overconfidence calibration".into(),
        );
        let gov = &store
            .operational_state
            .synthetic_governance
            .cybernetic_governor;
        assert!(!gov.control_signals.is_empty());
        assert!(!gov.feedback_loops.is_empty());
        assert!(!gov.actions.is_empty());
        assert!((0.0..=1.0).contains(&gov.stability));
    }

    #[test]
    fn synthetic_science_requires_replication_and_productive_dissent() {
        let mut store = CognitiveStore::default();
        run_synthetic_scientific_governance_in_store(&mut store, "replication requirement".into());
        let eco = &store.operational_state.synthetic_governance.ecosystem;
        assert!(!eco.replications.is_empty());
        assert!(eco.dissent_ecology.iter().any(|d| d.productive));
        assert!(eco.self_challenges.iter().any(|c| c.reduced_overconfidence));
    }

    #[test]
    fn epistemic_institutions_emit_bounded_decisions() {
        let mut store = CognitiveStore::default();
        run_synthetic_scientific_governance_in_store(&mut store, "institutional governance".into());
        let inst = &store.operational_state.synthetic_governance.institutions;
        assert!(!inst.decisions.is_empty());
        assert!(
            inst.decisions.len()
                <= store
                    .operational_state
                    .synthetic_governance
                    .safety
                    .max_institutional_decisions
        );
    }

    #[test]
    fn synthetic_governance_persistence_compatibility_defaults() {
        let json = serde_json::to_string(&CognitiveStore::default()).unwrap();
        let restored: CognitiveStore = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .operational_state
                .synthetic_governance
                .ecosystem
                .scientists
                .len(),
            8
        );
        assert_eq!(
            restored
                .operational_state
                .synthetic_governance
                .safety
                .max_ecosystem_cycles,
            3
        );
    }

    #[test]
    fn deliberation_is_bounded_and_persists_dissent() {
        let mut store = CognitiveStore::default();
        let (session, _) =
            run_deliberative_science_in_store(&mut store, "risky autonomy increase".into());
        assert!(
            session.bounded_turns
                <= store
                    .operational_state
                    .deliberative_science
                    .safety
                    .max_deliberation_turns
        );
        assert!(!session.outcome.dissent.is_empty());
        assert!(
            !store
                .operational_state
                .deliberative_science
                .deliberation
                .persistent_dissent
                .is_empty()
        );
    }

    #[test]
    fn evidence_weighted_consensus_and_adversarial_review_blocks_risk() {
        let mut store = CognitiveStore::default();
        let (session, _) = run_deliberative_science_in_store(
            &mut store,
            "risky execution with low evidence".into(),
        );
        assert!(session.outcome.consensus.score >= 0.0);
        assert_ne!(
            session.outcome.adversarial_review.outcome,
            ReviewOutcome::Pass
        );
        assert_eq!(
            session.outcome.arbitration.action,
            "repair_or_gather_evidence"
        );
    }

    #[test]
    fn hypothesis_lifecycle_information_gain_and_prompt_budget_are_protected() {
        let mut store = CognitiveStore::default();
        let (_, report) =
            run_deliberative_science_in_store(&mut store, "testable uncertainty".into());
        let ds = &store.operational_state.deliberative_science;
        assert!(ds.science.hypotheses.len() <= ds.safety.max_active_hypotheses);
        assert!(ds.science.uncertainty_plan.as_ref().unwrap().priorities[0].score >= 0.0);
        assert!(report.prompt_status.len() <= ds.safety.max_prompt_contribution);
    }

    #[test]
    fn causal_edges_require_evidence_and_low_confounding() {
        let mut store = CognitiveStore::default();
        run_deliberative_science_in_store(&mut store, "causal caution".into());
        let causal = &store
            .operational_state
            .deliberative_science
            .science
            .causal_engine;
        assert!(
            causal
                .accepted_edges
                .iter()
                .all(|e| e.evidence_count >= 2 && e.confounder_risk.0 < 0.4)
        );
    }

    #[test]
    fn competing_models_are_scored_and_unsupported_hypothesis_cannot_promote() {
        let models = score_competing_models(0.2, 0.5);
        assert_eq!(models.len(), 3);
        assert!(models.iter().all(|m| m.score.total.is_finite()));
        assert!(!decide_hypothesis_promotion(0.4, 0.8, 0.0, 0.0).promoted);
        assert!(decide_hypothesis_promotion(0.8, 0.8, 0.0, 0.0).promoted);
    }

    #[test]
    fn deliberative_science_persistence_compatibility_defaults() {
        let json = serde_json::to_string(&CognitiveStore::default()).unwrap();
        let restored: CognitiveStore = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .operational_state
                .deliberative_science
                .safety
                .max_deliberation_turns,
            6
        );
        assert_eq!(
            restored
                .operational_state
                .deliberative_science
                .deliberation
                .actors
                .len(),
            9
        );
    }

    #[test]
    fn relational_truth_maintenance_builds_support_relations() {
        let mut store = CognitiveStore::default();
        let now = Utc::now();
        store.operational_state.epistemology.claims.insert(
            "a".to_string(),
            EpistemicClaim {
                id: "a".to_string(),
                statement: "runtime store is grounded".to_string(),
                status: EpistemicStatus::Supported,
                confidence: 0.8,
                evidence_ids: vec![],
                contradiction_ids: vec![],
                last_revised_at: now,
            },
        );
        store.operational_state.epistemology.claims.insert(
            "b".to_string(),
            EpistemicClaim {
                id: "b".to_string(),
                statement: "grounded runtime store has evidence".to_string(),
                status: EpistemicStatus::Supported,
                confidence: 0.7,
                evidence_ids: vec![],
                contradiction_ids: vec![],
                last_revised_at: now,
            },
        );
        let relations = build_epistemic_relations(&store);
        assert!(relations.iter().any(|r| matches!(
            r.kind,
            EpistemicRelationKind::Supports | EpistemicRelationKind::Refines
        )));
    }

    #[test]
    fn relational_truth_maintenance_detects_conflicts_and_schedules_audit() {
        let mut store = CognitiveStore::default();
        let now = Utc::now();
        store.operational_state.epistemology.claims.insert(
            "a".to_string(),
            EpistemicClaim {
                id: "a".to_string(),
                statement: "always trust runtime evidence".to_string(),
                status: EpistemicStatus::Supported,
                confidence: 0.8,
                evidence_ids: vec![],
                contradiction_ids: vec![],
                last_revised_at: now,
            },
        );
        store.operational_state.epistemology.claims.insert(
            "b".to_string(),
            EpistemicClaim {
                id: "b".to_string(),
                statement: "never trust runtime evidence".to_string(),
                status: EpistemicStatus::Supported,
                confidence: 0.8,
                evidence_ids: vec![],
                contradiction_ids: vec![],
                last_revised_at: now,
            },
        );
        let relations = build_epistemic_relations(&store);
        let conflicts = build_conflict_sets(&store, &relations);
        assert!(!conflicts.is_empty());
        couple_epistemology_to_governor(&mut store, &conflicts);
        assert!(
            store
                .operational_state
                .task_queue
                .iter()
                .any(|task| matches!(task.kind, OperationalTaskKind::ContradictionAudit))
        );
    }

    #[test]
    fn relational_truth_maintenance_propagates_confidence_deltas() {
        let mut store = CognitiveStore::default();
        let now = Utc::now();
        store.operational_state.epistemology.claims.insert(
            "a".to_string(),
            EpistemicClaim {
                id: "a".to_string(),
                statement: "runtime evidence is supported".to_string(),
                status: EpistemicStatus::Supported,
                confidence: 0.7,
                evidence_ids: vec![],
                contradiction_ids: vec![],
                last_revised_at: now,
            },
        );
        store.operational_state.epistemology.claims.insert(
            "b".to_string(),
            EpistemicClaim {
                id: "b".to_string(),
                statement: "runtime evidence is supported by telemetry".to_string(),
                status: EpistemicStatus::Supported,
                confidence: 0.6,
                evidence_ids: vec![],
                contradiction_ids: vec![],
                last_revised_at: now,
            },
        );
        let relations = build_epistemic_relations(&store);
        let conflicts = build_conflict_sets(&store, &relations);
        let deltas = propagate_epistemic_confidence(&mut store, &relations, &conflicts);
        assert!(!deltas.is_empty());
    }
}
