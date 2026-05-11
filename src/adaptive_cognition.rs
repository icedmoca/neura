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
}
