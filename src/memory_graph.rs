//! Graph-based memory storage with tags, clusters, and semantic links
//!
//! This module provides a graph structure for organizing memories with:
//! - Tag nodes for explicit organization
//! - Cluster nodes for automatic grouping (future)
//! - Various edge types (HasTag, RelatesTo, Supersedes, etc.)
//! - BFS cascade retrieval through the graph

use crate::memory::{MemoryEntry, MemoryStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};

/// Current graph format version for migration detection
pub const GRAPH_VERSION: u32 = 2;

#[derive(Debug)]
struct TopKItem<T> {
    score: f32,
    ordinal: usize,
    value: T,
}

impl<T> PartialEq for TopKItem<T> {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.ordinal == other.ordinal
    }
}

impl<T> Eq for TopKItem<T> {}

impl<T> PartialOrd for TopKItem<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for TopKItem<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.ordinal.cmp(&other.ordinal))
    }
}

fn top_k_scored<T, I>(items: I, limit: usize) -> Vec<(T, f32)>
where
    I: IntoIterator<Item = (T, f32)>,
{
    if limit == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<Reverse<TopKItem<T>>> = BinaryHeap::new();
    for (ordinal, (value, score)) in items.into_iter().enumerate() {
        let candidate = Reverse(TopKItem {
            score,
            ordinal,
            value,
        });

        if heap.len() < limit {
            heap.push(candidate);
            continue;
        }

        let replace = heap
            .peek()
            .map(|smallest| score > smallest.0.score)
            .unwrap_or(false);
        if replace {
            heap.pop();
            heap.push(candidate);
        }
    }

    let mut results: Vec<_> = heap
        .into_iter()
        .map(|Reverse(item)| (item.value, item.score, item.ordinal))
        .collect();
    results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    results
        .into_iter()
        .map(|(value, score, _)| (value, score))
        .collect()
}

/// Semantic edge relationship types between nodes.
///
/// `similar_to` replaces the legacy `relates_to` (kept as a deserialize alias so
/// existing graphs migrate transparently). The remaining variants are true
/// semantic relations produced by tag structure, Hebbian learning, or LLM
/// review. The learned strength / provenance of an edge lives in [`EdgeMeta`],
/// not in the kind itself — the kind only says *what* the relationship is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Memory has this explicit tag (structural)
    HasTag,
    /// Memory belongs to an auto-discovered community/cluster (structural)
    InCluster,
    /// Undirected semantic similarity (legacy `relates_to`)
    #[serde(alias = "relates_to")]
    SimilarTo,
    /// Source provides evidence for target
    Supports,
    /// Conflicting information (both kept, flagged)
    Contradicts,
    /// Source causes / leads to target
    Causes,
    /// Source is a part of target
    PartOf,
    /// Source contains target
    Contains,
    /// Source uses target
    Uses,
    /// Source depends on target
    DependsOn,
    /// Source happens before target (temporal)
    Before,
    /// Source happens after target (temporal)
    After,
    /// Procedural knowledge derived from facts
    DerivedFrom,
    /// Source is an instance of target (individual -> type)
    InstanceOf,
    /// Source generalizes target (broader concept)
    Generalizes,
    /// Source specializes target (narrower concept)
    Specializes,
    /// Newer memory replaces older one (episodic bookkeeping)
    Supersedes,
}

fn default_weight() -> f32 {
    1.0
}

/// Short stable hex hash of a set of ids (order-independent since callers sort).
fn short_hash(items: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    for item in items {
        item.hash(&mut hasher);
    }
    format!("{:012x}", hasher.finish())
}

/// Tunables for one offline consolidation ("sleep") pass.
#[derive(Debug, Clone, Copy)]
pub struct SleepConfig {
    pub cooccurrence_min_overlap: f32,
    pub decay_factor: f32,
    pub decay_floor: f32,
    pub min_community_size: usize,
    pub label_prop_iters: usize,
    pub confidence_decay_base: f32,
}

impl Default for SleepConfig {
    fn default() -> Self {
        Self {
            cooccurrence_min_overlap: 0.34,
            decay_factor: 0.95,
            decay_floor: 0.15,
            min_community_size: 3,
            label_prop_iters: 8,
            confidence_decay_base: 0.98,
        }
    }
}

/// What one sleep cycle changed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SleepReport {
    pub linked: usize,
    pub weakened: usize,
    pub pruned: usize,
    pub communities: usize,
    pub consolidated: usize,
    pub contradictions_found: usize,
    pub concept_embeddings_refreshed: usize,
    pub confidence_decayed: usize,
    /// Concepts created/updated by the knowledge-source refresh that runs at
    /// the start of a full sleep cycle (0 when no sources are registered).
    #[serde(default)]
    pub knowledge_concepts_refreshed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

/// A single graph-integrity finding.
#[derive(Debug, Clone)]
pub enum GraphIssue {
    DanglingEdgeSource { from: String },
    DanglingEdgeTarget { from: String, to: String, kind: &'static str },
    MissingReverseEdge { from: String, to: String },
    EdgeConfidenceOutOfRange { from: String, to: String, weight: f32, confidence: f32 },
    EvidenceCountMismatch { from: String, to: String, count: u32, stored: usize },
    MemoryConfidenceOutOfRange { id: String, confidence: f32 },
    AsymmetricEdge { from: String, to: String, kind: &'static str },
    DuplicateSemanticMemory { a: String, b: String },
    CyclicSupersedes { chain: Vec<String> },
}

impl GraphIssue {
    /// Stable category label for grouping counts.
    pub fn category(&self) -> &'static str {
        match self {
            GraphIssue::DanglingEdgeSource { .. } => "dangling_edge_source",
            GraphIssue::DanglingEdgeTarget { .. } => "dangling_edge_target",
            GraphIssue::MissingReverseEdge { .. } => "missing_reverse_edge",
            GraphIssue::EdgeConfidenceOutOfRange { .. } => "edge_value_out_of_range",
            GraphIssue::EvidenceCountMismatch { .. } => "evidence_count_mismatch",
            GraphIssue::MemoryConfidenceOutOfRange { .. } => "memory_confidence_out_of_range",
            GraphIssue::AsymmetricEdge { .. } => "asymmetric_edge",
            GraphIssue::DuplicateSemanticMemory { .. } => "duplicate_semantic_memory",
            GraphIssue::CyclicSupersedes { .. } => "cyclic_supersedes",
        }
    }
}

impl std::fmt::Display for GraphIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphIssue::DanglingEdgeSource { from } => {
                write!(f, "edge from non-memory node '{from}'")
            }
            GraphIssue::DanglingEdgeTarget { from, to, kind } => {
                write!(f, "{kind} edge '{from}' -> '{to}' points at a missing node")
            }
            GraphIssue::MissingReverseEdge { from, to } => {
                write!(f, "edge '{from}' -> '{to}' missing reverse index entry")
            }
            GraphIssue::EdgeConfidenceOutOfRange { from, to, weight, confidence } => {
                write!(f, "edge '{from}' -> '{to}' weight={weight} confidence={confidence} out of [0,1]")
            }
            GraphIssue::EvidenceCountMismatch { from, to, count, stored } => {
                write!(f, "edge '{from}' -> '{to}' evidence_count={count} < stored {stored}")
            }
            GraphIssue::MemoryConfidenceOutOfRange { id, confidence } => {
                write!(f, "memory '{id}' confidence={confidence} out of [0,1]")
            }
            GraphIssue::AsymmetricEdge { from, to, kind } => {
                write!(f, "symmetric {kind} edge '{from}' -> '{to}' has no mirror")
            }
            GraphIssue::DuplicateSemanticMemory { a, b } => {
                write!(f, "duplicate semantic memories '{a}' and '{b}'")
            }
            GraphIssue::CyclicSupersedes { chain } => {
                write!(f, "cyclic Supersedes chain: {}", chain.join(" -> "))
            }
        }
    }
}

/// A record of one automatic consolidation event (Phase 1), retained as history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationRecord {
    /// The semantic memory created/updated.
    pub semantic_id: String,
    /// Episodic sources merged into it.
    pub sources: Vec<String>,
    /// Short label of what concept was consolidated.
    pub concept: String,
    pub at: DateTime<Utc>,
}

impl EdgeKind {
    /// Base traversal strength for BFS scoring (before the learned per-edge
    /// weight in [`EdgeMeta`] modulates it). `SimilarTo` is 1.0 so its learned
    /// weight *is* its traversal weight (preserving legacy `relates_to`).
    pub fn base_weight(&self) -> f32 {
        match self {
            EdgeKind::SimilarTo => 1.0,
            EdgeKind::Supports => 0.9,
            EdgeKind::Causes => 0.85,
            EdgeKind::DependsOn => 0.8,
            EdgeKind::PartOf | EdgeKind::Contains => 0.8,
            EdgeKind::Supersedes => 0.9,
            EdgeKind::HasTag => 0.8,
            EdgeKind::Uses => 0.75,
            EdgeKind::DerivedFrom => 0.7,
            EdgeKind::InstanceOf | EdgeKind::Generalizes | EdgeKind::Specializes => 0.7,
            EdgeKind::InCluster => 0.6,
            EdgeKind::Before | EdgeKind::After => 0.5,
            EdgeKind::Contradicts => 0.3,
        }
    }

    /// Snake_case label used in stats / mermaid / CLI.
    pub fn label(&self) -> &'static str {
        match self {
            EdgeKind::HasTag => "has_tag",
            EdgeKind::InCluster => "in_cluster",
            EdgeKind::SimilarTo => "similar_to",
            EdgeKind::Supports => "supports",
            EdgeKind::Contradicts => "contradicts",
            EdgeKind::Causes => "causes",
            EdgeKind::PartOf => "part_of",
            EdgeKind::Contains => "contains",
            EdgeKind::Uses => "uses",
            EdgeKind::DependsOn => "depends_on",
            EdgeKind::Before => "before",
            EdgeKind::After => "after",
            EdgeKind::DerivedFrom => "derived_from",
            EdgeKind::InstanceOf => "instance_of",
            EdgeKind::Generalizes => "generalizes",
            EdgeKind::Specializes => "specializes",
            EdgeKind::Supersedes => "supersedes",
        }
    }

    /// Associative (undirected co-relevance) relations that Hebbian learning is
    /// allowed to reinforce. Logical / directional relations (Causes, PartOf,
    /// Before, …) must NOT be strengthened just because two memories co-fired —
    /// that would corrupt their meaning.
    pub fn is_reinforceable(&self) -> bool {
        matches!(self, EdgeKind::SimilarTo | EdgeKind::Supports)
    }

    /// Whether the relation is symmetric (a↔b) vs. directional (a→b).
    pub fn is_symmetric(&self) -> bool {
        matches!(
            self,
            EdgeKind::SimilarTo | EdgeKind::Contradicts | EdgeKind::InCluster
        )
    }

    /// Memory↔memory semantic relations (excludes structural has_tag/in_cluster).
    pub fn is_semantic(&self) -> bool {
        !matches!(self, EdgeKind::HasTag | EdgeKind::InCluster)
    }
}

/// Where an edge came from — its provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSource {
    /// Derived from shared tags / structure.
    Tag,
    /// Learned by Hebbian co-activation.
    Hebbian,
    /// Proposed by an LLM review pass.
    Llm,
    /// Created explicitly by the user / an API call.
    Manual,
    /// Internal bookkeeping (has_tag, supersedes migration, clusters).
    #[default]
    System,
}

/// A piece of evidence justifying a fact or an edge. "Never store confidence
/// without evidence."
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// What kind of thing the evidence is.
    pub kind: EvidenceKind,
    /// Identifier: a memory id, conversation id, project path, or free text.
    pub id: String,
    /// Optional human-readable note about why this counts as evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// When the evidence was recorded.
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Memory,
    Conversation,
    Project,
    Observation,
}

impl EvidenceRef {
    pub fn memory(id: impl Into<String>) -> Self {
        Self {
            kind: EvidenceKind::Memory,
            id: id.into(),
            note: None,
            at: Utc::now(),
        }
    }
    pub fn observation(note: impl Into<String>) -> Self {
        Self {
            kind: EvidenceKind::Observation,
            id: String::new(),
            note: Some(note.into()),
            at: Utc::now(),
        }
    }
}

/// Rich per-edge metadata: learned strength, evidence-backed confidence, and
/// provenance. Flattened into [`Edge`] so old edges (which only had `weight`)
/// deserialize cleanly with defaults for the new fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeMeta {
    /// Learned strength (0.0-1.0). Modulates `EdgeKind::base_weight`.
    #[serde(default = "default_weight")]
    pub weight: f32,
    /// Confidence in the relationship, derived from accumulated evidence.
    #[serde(default)]
    pub confidence: f32,
    /// How many independent observations support this edge.
    #[serde(default)]
    pub evidence_count: u32,
    /// When the edge was first created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// When the edge was last reinforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reinforced: Option<DateTime<Utc>>,
    /// Provenance.
    #[serde(default)]
    pub source: EdgeSource,
    /// Concrete evidence entries (bounded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRef>,
}

impl Default for EdgeMeta {
    fn default() -> Self {
        Self {
            weight: 1.0,
            confidence: 0.0,
            evidence_count: 0,
            created_at: None,
            last_reinforced: None,
            source: EdgeSource::default(),
            evidence: Vec::new(),
        }
    }
}

/// Max evidence entries retained per edge (keeps graphs from ballooning).
const MAX_EDGE_EVIDENCE: usize = 8;

impl EdgeMeta {
    /// Confidence as a saturating function of evidence count:
    /// `1 - 0.55^n` → 0.45, 0.70, 0.83, 0.91, … Never exceeds ~0.99.
    pub fn confidence_from_evidence(n: u32) -> f32 {
        if n == 0 {
            0.0
        } else {
            (1.0 - 0.55_f32.powi(n as i32)).min(0.99)
        }
    }

    /// Record a new observation: bump count, recompute confidence, append
    /// evidence (bounded), and stamp `last_reinforced`.
    pub fn add_evidence(&mut self, ev: EvidenceRef) {
        self.evidence_count = self.evidence_count.saturating_add(1);
        self.confidence = Self::confidence_from_evidence(self.evidence_count);
        self.last_reinforced = Some(Utc::now());
        self.evidence.push(ev);
        if self.evidence.len() > MAX_EDGE_EVIDENCE {
            let overflow = self.evidence.len() - MAX_EDGE_EVIDENCE;
            self.evidence.drain(0..overflow);
        }
    }
}

/// An edge in the memory graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// Target node ID
    pub target: String,
    /// Type of relationship
    pub kind: EdgeKind,
    /// Learned strength, evidence-backed confidence, and provenance.
    #[serde(flatten)]
    pub meta: EdgeMeta,
}

impl Edge {
    pub fn new(target: impl Into<String>, kind: EdgeKind) -> Self {
        Self {
            target: target.into(),
            kind,
            meta: EdgeMeta {
                created_at: Some(Utc::now()),
                ..EdgeMeta::default()
            },
        }
    }

    /// Traversal strength for BFS: semantic base × learned weight, lightly
    /// lifted by confidence so well-evidenced edges propagate a bit further.
    pub fn traversal_weight(&self) -> f32 {
        let conf_boost = 1.0 + 0.25 * self.meta.confidence;
        (self.kind.base_weight() * self.meta.weight * conf_boost).clamp(0.0, 1.0)
    }

    pub fn with_weight(mut self, weight: f32) -> Self {
        self.meta.weight = weight.clamp(0.0, 1.0);
        self
    }

    pub fn with_source(mut self, source: EdgeSource) -> Self {
        self.meta.source = source;
        self
    }
}

/// A tag node in the graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagEntry {
    /// Unique ID (format: "tag:{name}")
    pub id: String,
    /// Display name
    pub name: String,
    /// Optional description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Number of memories with this tag
    pub count: u32,
    /// When the tag was first created
    pub created_at: DateTime<Utc>,
}

impl TagEntry {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: format!("tag:{}", name),
            name,
            description: None,
            count: 0,
            created_at: Utc::now(),
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// A cluster node (auto-discovered grouping via embeddings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterEntry {
    /// Unique ID (format: "cluster:{id}")
    pub id: String,
    /// Optional human-readable name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Centroid embedding (average of member embeddings)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub centroid: Vec<f32>,
    /// Number of memories in this cluster
    pub member_count: u32,
    /// When the cluster was discovered
    pub created_at: DateTime<Utc>,
    /// When the cluster was last updated
    pub updated_at: DateTime<Utc>,
}

impl ClusterEntry {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let now = Utc::now();
        Self {
            id: format!("cluster:{}", id),
            name: None,
            centroid: Vec::new(),
            member_count: 0,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Graph metadata for tracking statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphMetadata {
    /// When clusters were last updated
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cluster_update: Option<DateTime<Utc>>,
    /// Total retrieval operations
    #[serde(default)]
    pub retrieval_count: u64,
    /// Total links discovered via co-relevance
    #[serde(default)]
    pub link_discovery_count: u64,
    /// Bounded history of automatic semantic consolidations (Phase 1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consolidations: Vec<ConsolidationRecord>,
    /// The most recent full sleep-cycle report (Phase 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sleep: Option<SleepReport>,
    /// When concept embeddings were last refreshed (Phase 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_concept_embed: Option<DateTime<Utc>>,
    /// Registered knowledge sources (unified knowledge layer) and their
    /// incremental-ingest state, keyed by source id. Lives here so sources
    /// reuse the graph's own persistence — no parallel store.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub knowledge_sources:
        std::collections::BTreeMap<String, crate::knowledge::KnowledgeSourceState>,
    /// Rolling calibration of architectural predictions (v0.14 adaptive
    /// planning); updated only at reflection time.
    #[serde(default)]
    pub prediction_stats: crate::knowledge::reasoning::PredictionStats,
}

/// The memory graph - HashMap-based for clean JSON serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGraph {
    /// Format version for migration detection
    pub graph_version: u32,

    /// Memory nodes by ID
    pub memories: HashMap<String, MemoryEntry>,

    /// Tag nodes by ID (format: "tag:{name}")
    pub tags: HashMap<String, TagEntry>,

    /// Cluster nodes by ID (format: "cluster:{id}")
    #[serde(default)]
    pub clusters: HashMap<String, ClusterEntry>,

    /// Forward edges: source_id -> Vec<Edge>
    #[serde(default)]
    pub edges: HashMap<String, Vec<Edge>>,

    /// Reverse edges for efficient BFS: target_id -> Vec<source_id>
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub reverse_edges: HashMap<String, Vec<String>>,

    /// Graph statistics and metadata
    #[serde(default)]
    pub metadata: GraphMetadata,
}

impl Default for MemoryGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryGraph {
    /// Create a new empty memory graph
    pub fn new() -> Self {
        Self {
            graph_version: GRAPH_VERSION,
            memories: HashMap::new(),
            tags: HashMap::new(),
            clusters: HashMap::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            metadata: GraphMetadata::default(),
        }
    }

    /// Get the number of memories in the graph
    pub fn memory_count(&self) -> usize {
        self.memories.len()
    }

    // ==================== Memory Operations ====================

    /// Add a memory entry to the graph
    /// Also creates tag nodes and HasTag edges for any tags on the entry
    pub fn add_memory(&mut self, mut entry: MemoryEntry) -> String {
        entry.refresh_search_text();
        let id = entry.id.clone();

        // Create tag nodes and edges for existing tags
        for tag_name in &entry.tags {
            self.ensure_tag(tag_name);
            let tag_id = format!("tag:{}", tag_name);
            self.add_edge_internal(&id, &tag_id, EdgeKind::HasTag);

            // Increment tag count
            if let Some(tag) = self.tags.get_mut(&tag_id) {
                tag.count += 1;
            }
        }

        // Handle superseded_by as a Supersedes edge (reverse direction)
        if let Some(ref superseded_by) = entry.superseded_by {
            // The newer memory supersedes this one
            self.add_edge_internal(superseded_by, &id, EdgeKind::Supersedes);
        }

        self.memories.insert(id.clone(), entry);
        id
    }

    /// Get a memory by ID
    pub fn get_memory(&self, id: &str) -> Option<&MemoryEntry> {
        self.memories.get(id)
    }

    /// Get a mutable memory by ID
    pub fn get_memory_mut(&mut self, id: &str) -> Option<&mut MemoryEntry> {
        self.memories.get_mut(id)
    }

    /// Remove a memory from the graph (also removes associated edges)
    pub fn remove_memory(&mut self, id: &str) -> Option<MemoryEntry> {
        // Remove all edges from this memory
        if let Some(edges) = self.edges.remove(id) {
            for edge in edges {
                // Update reverse edges
                if let Some(reverse) = self.reverse_edges.get_mut(&edge.target) {
                    reverse.retain(|src| src != id);
                }
                // Decrement tag count if HasTag
                if matches!(edge.kind, EdgeKind::HasTag)
                    && let Some(tag) = self.tags.get_mut(&edge.target)
                {
                    tag.count = tag.count.saturating_sub(1);
                }
            }
        }

        // Remove all edges pointing to this memory
        if let Some(sources) = self.reverse_edges.remove(id) {
            for source in sources {
                if let Some(edges) = self.edges.get_mut(&source) {
                    edges.retain(|e| e.target != id);
                }
            }
        }

        self.memories.remove(id)
    }

    /// Get all memories (for iteration)
    pub fn all_memories(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.memories.values()
    }

    /// Get all active memories
    pub fn active_memories(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.memories.values().filter(|m| m.active)
    }

    // ==================== Tag Operations ====================

    /// Ensure a tag exists, creating it if necessary
    pub fn ensure_tag(&mut self, name: &str) -> &TagEntry {
        let tag_id = format!("tag:{}", name);
        self.tags
            .entry(tag_id.clone())
            .or_insert_with(|| TagEntry::new(name))
    }

    /// Add a tag to a memory
    pub fn tag_memory(&mut self, memory_id: &str, tag_name: &str) {
        // Ensure tag exists
        self.ensure_tag(tag_name);
        let tag_id = format!("tag:{}", tag_name);

        // Check if edge already exists
        if let Some(edges) = self.edges.get(memory_id)
            && edges
                .iter()
                .any(|e| e.target == tag_id && matches!(e.kind, EdgeKind::HasTag))
        {
            return;
        }

        // Add edge
        self.add_edge_internal(memory_id, &tag_id, EdgeKind::HasTag);

        // Update tag count
        if let Some(tag) = self.tags.get_mut(&tag_id) {
            tag.count += 1;
        }

        // Update memory's tags list
        if let Some(memory) = self.memories.get_mut(memory_id)
            && !memory.tags.contains(&tag_name.to_string())
        {
            memory.tags.push(tag_name.to_string());
            memory.refresh_search_text();
        }
    }

    /// Remove a tag from a memory
    pub fn untag_memory(&mut self, memory_id: &str, tag_name: &str) {
        let tag_id = format!("tag:{}", tag_name);

        // Remove edge
        if let Some(edges) = self.edges.get_mut(memory_id) {
            edges.retain(|e| !(e.target == tag_id && matches!(e.kind, EdgeKind::HasTag)));
        }

        // Update reverse edges
        if let Some(sources) = self.reverse_edges.get_mut(&tag_id) {
            sources.retain(|s| s != memory_id);
        }

        // Update tag count
        if let Some(tag) = self.tags.get_mut(&tag_id) {
            tag.count = tag.count.saturating_sub(1);
        }

        // Update memory's tags list
        if let Some(memory) = self.memories.get_mut(memory_id) {
            memory.tags.retain(|t| t != tag_name);
            memory.refresh_search_text();
        }
    }

    /// Get all memories with a specific tag
    pub fn get_memories_by_tag(&self, tag_name: &str) -> Vec<&MemoryEntry> {
        let tag_id = format!("tag:{}", tag_name);

        // Find all sources pointing to this tag via HasTag
        self.reverse_edges
            .get(&tag_id)
            .map(|sources| {
                sources
                    .iter()
                    .filter_map(|id| self.memories.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all tags
    pub fn all_tags(&self) -> impl Iterator<Item = &TagEntry> {
        self.tags.values()
    }

    // ==================== Edge Operations ====================

    /// Add an edge between two nodes (internal, no validation)
    fn add_edge_internal(&mut self, from: &str, to: &str, kind: EdgeKind) {
        // Add forward edge
        self.edges
            .entry(from.to_string())
            .or_default()
            .push(Edge::new(to, kind));

        // Add reverse edge
        self.reverse_edges
            .entry(to.to_string())
            .or_default()
            .push(from.to_string());
    }

    /// Add an edge between two nodes
    pub fn add_edge(&mut self, from: &str, to: &str, kind: EdgeKind) {
        // Check if edge already exists
        if let Some(edges) = self.edges.get(from)
            && edges.iter().any(|e| e.target == to && e.kind == kind)
        {
            return;
        }

        self.add_edge_internal(from, to, kind);
    }

    /// Remove an edge between two nodes
    pub fn remove_edge(&mut self, from: &str, to: &str, kind: &EdgeKind) {
        if let Some(edges) = self.edges.get_mut(from) {
            edges.retain(|e| !(e.target == to && &e.kind == kind));
        }
        if let Some(sources) = self.reverse_edges.get_mut(to) {
            sources.retain(|s| s != from);
        }
    }

    /// Get all edges from a node
    pub fn get_edges(&self, node_id: &str) -> &[Edge] {
        self.edges.get(node_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all nodes pointing to this node
    pub fn get_incoming(&self, node_id: &str) -> Vec<&str> {
        self.reverse_edges
            .get(node_id)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Link two memories with a `SimilarTo` edge (legacy API; manual source).
    pub fn link_memories(&mut self, from: &str, to: &str, weight: f32) {
        let w = weight.clamp(0.0, 1.0);
        self.upsert_edge_meta(from, to, EdgeKind::SimilarTo, EdgeSource::Manual, |m| {
            m.weight = w;
        });
        self.metadata.link_discovery_count += 1;
    }

    /// Mark a memory as superseding another
    pub fn supersede(&mut self, newer_id: &str, older_id: &str) {
        self.add_edge(newer_id, older_id, EdgeKind::Supersedes);
        // Mark older as inactive
        if let Some(older) = self.memories.get_mut(older_id) {
            older.active = false;
            older.superseded_by = Some(newer_id.to_string());
        }
    }

    /// Mark two memories as contradicting, optionally recording why + how sure.
    pub fn mark_contradiction(&mut self, id_a: &str, id_b: &str) {
        self.add_edge(id_a, id_b, EdgeKind::Contradicts);
        self.add_edge(id_b, id_a, EdgeKind::Contradicts);
    }

    // ==================== Typed edge upsert ====================

    /// Create or update a single directed edge, applying `f` to its metadata.
    /// New edges are stamped with `source` + `created_at`.
    fn upsert_edge_meta(
        &mut self,
        from: &str,
        to: &str,
        kind: EdgeKind,
        source: EdgeSource,
        f: impl FnOnce(&mut EdgeMeta),
    ) {
        let mut created = false;
        {
            let edges = self.edges.entry(from.to_string()).or_default();
            match edges.iter_mut().find(|e| e.target == to && e.kind == kind) {
                Some(edge) => f(&mut edge.meta),
                None => {
                    let mut edge = Edge::new(to.to_string(), kind).with_source(source);
                    // Learned edges start at zero weight so the callback fully
                    // owns the initial strength (reinforce → delta, ensure/typed
                    // → the given weight). The 1.0 default only applies to
                    // structural edges created via `add_edge_internal`.
                    edge.meta.weight = 0.0;
                    f(&mut edge.meta);
                    edges.push(edge);
                    created = true;
                }
            }
        }
        if created {
            self.reverse_edges
                .entry(to.to_string())
                .or_default()
                .push(from.to_string());
        }
    }

    /// Add (or reinforce) a typed semantic edge with explicit weight, source,
    /// and optional evidence. Symmetric kinds (SimilarTo/Contradicts) get both
    /// directions. This is the general entry point for tag/LLM/manual relations.
    pub fn add_typed_edge(
        &mut self,
        from: &str,
        to: &str,
        kind: EdgeKind,
        weight: f32,
        source: EdgeSource,
        evidence: Option<EvidenceRef>,
    ) {
        if from == to || !self.memories.contains_key(from) || !self.memories.contains_key(to) {
            return;
        }
        let w = weight.clamp(0.0, 1.0);
        let dirs: &[(&str, &str)] = if kind.is_symmetric() {
            &[(from, to), (to, from)]
        } else {
            &[(from, to)]
        };
        for (a, b) in dirs {
            let ev = evidence.clone();
            self.upsert_edge_meta(a, b, kind, source, |m| {
                m.weight = m.weight.max(w);
                if let Some(ev) = ev {
                    m.add_evidence(ev);
                }
            });
        }
    }

    // ==================== Hebbian association learning ====================

    /// Ensure a symmetric `SimilarTo` edge between two memories with *at least*
    /// `weight` (never lowers an existing stronger link). Used by structural
    /// bootstrapping (shared tags): source = `Tag`, seeded with one observation
    /// of evidence so it carries a non-zero, justified confidence.
    fn ensure_relates_to(&mut self, a: &str, b: &str, weight: f32) {
        let weight = weight.clamp(0.0, 1.0);
        for (from, to) in [(a, b), (b, a)] {
            self.upsert_edge_meta(from, to, EdgeKind::SimilarTo, EdgeSource::Tag, |m| {
                m.weight = m.weight.max(weight);
                if m.evidence_count == 0 {
                    m.add_evidence(EvidenceRef::observation("shared tags"));
                }
            });
        }
    }

    /// Hebbian reinforcement: two memories relevant together strengthen their
    /// symmetric `SimilarTo` link (co-activation → wire together), capped at
    /// 1.0, and accrue an observation of evidence (raising confidence). Only
    /// associative kinds are ever reinforced this way — logical/directional
    /// relations are never Hebbian-bumped.
    pub fn reinforce_link(&mut self, a: &str, b: &str, delta: f32) {
        if a == b || !self.memories.contains_key(a) || !self.memories.contains_key(b) {
            return;
        }
        for (from, to) in [(a, b), (b, a)] {
            self.upsert_edge_meta(from, to, EdgeKind::SimilarTo, EdgeSource::Hebbian, |m| {
                m.weight = (m.weight + delta).min(1.0);
                m.add_evidence(EvidenceRef::observation("co-relevant in a turn"));
            });
        }
        self.metadata.link_discovery_count += 1;
    }

    /// Cold-start / continuous association growth: create weak `RelatesTo`
    /// edges between active memories that share tags, weighted by tag Jaccard
    /// overlap. This lets the graph form associations from the tags it already
    /// has instead of waiting for two memories to co-verify in the same turn.
    ///
    /// Ultra-generic tags are ignored so they don't over-connect everything.
    /// Returns the number of memory pairs linked or strengthened.
    pub fn bootstrap_cooccurrence_links(&mut self, min_overlap: f32) -> usize {
        const GENERIC: &[&str] = &["sensitive", "general", "misc", "other", "note", "info"];

        let mut tag_sets: HashMap<String, HashSet<String>> = HashMap::new();
        for (id, m) in &self.memories {
            if !m.active {
                continue;
            }
            let tags: HashSet<String> = m
                .tags
                .iter()
                .filter(|t| !GENERIC.contains(&t.as_str()))
                .cloned()
                .collect();
            if !tags.is_empty() {
                tag_sets.insert(id.clone(), tags);
            }
        }

        // Inverted index tag -> memories, so we only consider pairs that
        // actually share a tag (avoids the full O(n^2) over all memories).
        let mut inverted: HashMap<String, Vec<String>> = HashMap::new();
        for (id, tags) in &tag_sets {
            for t in tags {
                inverted.entry(t.clone()).or_default().push(id.clone());
            }
        }

        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut pending: Vec<(String, String, f32)> = Vec::new();
        for members in inverted.values() {
            for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let (a, b) = (&members[i], &members[j]);
                    let key = if a < b {
                        (a.clone(), b.clone())
                    } else {
                        (b.clone(), a.clone())
                    };
                    if !seen.insert(key) {
                        continue;
                    }
                    let ta = &tag_sets[a];
                    let tb = &tag_sets[b];
                    let inter = ta.intersection(tb).count() as f32;
                    let uni = ta.union(tb).count() as f32;
                    let jac = if uni > 0.0 { inter / uni } else { 0.0 };
                    if jac >= min_overlap {
                        pending.push((a.clone(), b.clone(), 0.3 + 0.4 * jac));
                    }
                }
            }
        }

        let linked = pending.len();
        for (a, b, w) in pending {
            self.ensure_relates_to(&a, &b, w);
        }
        linked
    }

    /// Fade associations that are not being reinforced. Multiplies every
    /// `RelatesTo` weight by `factor` (`< 1.0`) and prunes edges whose weight
    /// falls below `floor`. Keeps the graph from ossifying around stale links.
    /// Returns `(weakened, pruned)`.
    pub fn decay_relates_to(&mut self, factor: f32, floor: f32) -> (usize, usize) {
        let mut weakened = 0usize;
        let mut prune_pairs: Vec<(String, String)> = Vec::new();

        for (from, edges) in self.edges.iter_mut() {
            edges.retain_mut(|e| {
                // Only faded learned associations; structural / logical relations
                // (has_tag, causes, part_of, supersedes, …) are never decayed.
                if e.kind.is_reinforceable() {
                    e.meta.weight *= factor;
                    weakened += 1;
                    if e.meta.weight < floor {
                        prune_pairs.push((from.clone(), e.target.clone()));
                        return false;
                    }
                }
                true
            });
        }

        let pruned = prune_pairs.len();
        for (from, to) in prune_pairs {
            if let Some(sources) = self.reverse_edges.get_mut(&to) {
                sources.retain(|s| s != &from);
            }
        }
        (weakened, pruned)
    }

    // ==================== Graph Stats ====================

    /// Count edges by kind label (e.g. `"similar_to" -> 12`).
    pub fn edge_type_counts(&self) -> std::collections::BTreeMap<&'static str, usize> {
        let mut c: std::collections::BTreeMap<&'static str, usize> =
            std::collections::BTreeMap::new();
        for edges in self.edges.values() {
            for e in edges {
                *c.entry(e.kind.label()).or_insert(0) += 1;
            }
        }
        c
    }

    /// Convenience: number of edges of a given kind.
    pub fn count_edges_of_kind(&self, kind: EdgeKind) -> usize {
        self.edges
            .values()
            .flatten()
            .filter(|e| e.kind == kind)
            .count()
    }

    /// Most connected memories (by combined out+in degree over memory<->memory
    /// and memory->tag edges). Returns `(memory_id, degree)` sorted desc.
    pub fn top_hubs(&self, n: usize) -> Vec<(String, usize)> {
        let mut degree: HashMap<String, usize> = HashMap::new();
        for (from, edges) in &self.edges {
            if !self.memories.contains_key(from) {
                continue;
            }
            *degree.entry(from.clone()).or_insert(0) += edges.len();
        }
        for (target, sources) in &self.reverse_edges {
            if self.memories.contains_key(target) {
                *degree.entry(target.clone()).or_insert(0) += sources.len();
            }
        }
        let mut v: Vec<(String, usize)> = degree.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// Render a compact Mermaid `graph LR` of the memory<->memory relationships
    /// (RelatesTo / Supersedes / Contradicts) plus tag hubs, for observability.
    /// Bounded to `max_nodes` memories (highest-degree first) so it stays legible.
    pub fn to_mermaid(&self, max_nodes: usize) -> String {
        let hubs: HashSet<String> = self
            .top_hubs(max_nodes)
            .into_iter()
            .map(|(id, _)| id)
            .collect();

        let short = |id: &str, graph: &MemoryGraph| -> String {
            graph
                .memories
                .get(id)
                .map(|m| {
                    let c: String = m.content.chars().take(32).collect();
                    c.replace(['"', '\n', '[', ']', '(', ')'], " ")
                })
                .unwrap_or_else(|| id.to_string())
        };

        let node_id = |id: &str| -> String {
            let mut s = String::from("m_");
            for ch in id.chars() {
                if ch.is_ascii_alphanumeric() {
                    s.push(ch);
                } else {
                    s.push('_');
                }
            }
            s
        };

        let mut out = String::from("graph LR\n");
        let mut drawn: HashSet<(String, String, &str)> = HashSet::new();
        for (from, edges) in &self.edges {
            if !hubs.contains(from) {
                continue;
            }
            for e in edges {
                // Only draw memory↔memory semantic relations.
                if !e.kind.is_semantic() || !self.memories.contains_key(&e.target) {
                    continue;
                }
                let label = match e.kind {
                    EdgeKind::SimilarTo => format!("~{:.2}", e.traversal_weight()),
                    other => other.label().to_string(),
                };
                if !hubs.contains(&e.target) {
                    continue;
                }
                let key = (from.clone(), e.target.clone(), "e");
                if !drawn.insert(key) {
                    continue;
                }
                out.push_str(&format!(
                    "  {}[\"{}\"] -->|{}| {}[\"{}\"]\n",
                    node_id(from),
                    short(from, self),
                    label,
                    node_id(&e.target),
                    short(&e.target, self),
                ));
            }
        }
        out
    }

    /// Get total number of nodes (memories + tags + clusters)
    pub fn node_count(&self) -> usize {
        self.memories.len() + self.tags.len() + self.clusters.len()
    }

    /// Get total number of edges
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|v| v.len()).sum()
    }

    // ==================== Cascade Retrieval ====================

    /// Perform BFS cascade retrieval starting from seed memories
    ///
    /// Starting from embedding search hits (seeds), traverse through the graph
    /// via tags and other edges to find related memories.
    ///
    /// Returns (memory_id, score) pairs sorted by score descending.
    pub fn cascade_retrieve(
        &mut self,
        seed_ids: &[String],
        seed_scores: &[f32],
        max_depth: usize,
        max_results: usize,
    ) -> Vec<(String, f32)> {
        self.metadata.retrieval_count += 1;

        let mut visited: HashSet<String> = HashSet::new();
        let mut results: HashMap<String, f32> = HashMap::new();
        let mut queue: VecDeque<(String, f32, usize)> = VecDeque::new();

        // Initialize with seeds
        for (id, score) in seed_ids.iter().zip(seed_scores.iter()) {
            if self.memories.contains_key(id) {
                queue.push_back((id.clone(), *score, 0));
                results.insert(id.clone(), *score);
            }
        }

        // BFS traversal
        while let Some((node_id, score, depth)) = queue.pop_front() {
            if visited.contains(&node_id) {
                continue;
            }
            visited.insert(node_id.clone());

            if depth >= max_depth {
                continue;
            }

            // Traverse edges from this node
            for edge in self.get_edges(&node_id).to_vec() {
                let target = &edge.target;

                // Skip if already visited
                if visited.contains(target) {
                    continue;
                }

                // Calculate decayed score
                let edge_weight = edge.traversal_weight();
                let decay = 0.7_f32.powi(depth as i32 + 1);
                let new_score = score * edge_weight * decay;

                // If target is a tag, find all memories with this tag
                if target.starts_with("tag:") {
                    for source_id in self.get_incoming(target).iter() {
                        let source_id = source_id.to_string();
                        if !visited.contains(&source_id) && self.memories.contains_key(&source_id) {
                            let existing = results.get(&source_id).copied().unwrap_or(0.0);
                            if new_score > existing {
                                results.insert(source_id.clone(), new_score);
                                queue.push_back((source_id, new_score, depth + 1));
                            }
                        }
                    }
                }
                // If target is a memory, add it
                else if self.memories.contains_key(target) {
                    let existing = results.get(target).copied().unwrap_or(0.0);
                    if new_score > existing {
                        results.insert(target.clone(), new_score);
                        queue.push_back((target.clone(), new_score, depth + 1));
                    }
                }
            }
        }

        // Keep only the top-scoring results
        top_k_scored(results, max_results)
    }

    // ==================== Semantic neighbourhood helpers ====================

    /// Semantic (memory↔memory) out-edges from `id`, excluding structural
    /// has_tag / in_cluster edges.
    pub fn semantic_out_edges(&self, id: &str) -> Vec<&Edge> {
        self.get_edges(id)
            .iter()
            .filter(|e| e.kind.is_semantic() && self.memories.contains_key(&e.target))
            .collect()
    }

    /// Combined semantic degree (out + in) of a memory node.
    fn memory_degree(&self, id: &str) -> usize {
        let out = self.semantic_out_edges(id).len();
        let inc = self
            .get_incoming(id)
            .iter()
            .filter(|s| self.memories.contains_key(**s))
            .count();
        out + inc
    }

    // ==================== #7 Memory importance ====================

    /// Score a memory's importance in `[0,1]` from six signals: usage,
    /// graph centrality, recency, user emphasis (trust), fact confidence, and
    /// connectivity (average strength of its semantic links). Important
    /// memories are protected from decay/pruning during the sleep cycle.
    pub fn importance(&self, id: &str) -> f32 {
        let Some(m) = self.memories.get(id) else {
            return 0.0;
        };

        let usage = ((1.0 + m.access_count as f32).ln() / 4.0).min(1.0);
        let centrality = (self.memory_degree(id) as f32 / 8.0).min(1.0);
        let days = (Utc::now() - m.updated_at).num_seconds().max(0) as f32 / 86_400.0;
        let recency = 0.5_f32.powf(days / 14.0);
        let emphasis = match m.trust {
            crate::memory::TrustLevel::High => 1.0,
            crate::memory::TrustLevel::Medium => 0.5,
            crate::memory::TrustLevel::Low => 0.25,
        };
        let confidence = m.confidence.clamp(0.0, 1.0);
        let sem = self.semantic_out_edges(id);
        let connectivity = if sem.is_empty() {
            0.0
        } else {
            sem.iter().map(|e| e.traversal_weight()).sum::<f32>() / sem.len() as f32
        };

        (0.22 * usage
            + 0.20 * centrality
            + 0.16 * recency
            + 0.14 * emphasis
            + 0.16 * confidence
            + 0.12 * connectivity)
            .clamp(0.0, 1.0)
    }

    /// Importance ranking of active memories, highest first.
    pub fn importance_ranking(&self, top_n: usize) -> Vec<(String, f32)> {
        let mut v: Vec<(String, f32)> = self
            .memories
            .iter()
            .filter(|(_, m)| m.active)
            .map(|(id, _)| (id.clone(), self.importance(id)))
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(top_n);
        v
    }

    // ==================== #6 Community detection ====================

    /// Detect concept communities via weighted label propagation over the
    /// memory↔memory semantic graph (Contradicts excluded — opposing facts
    /// shouldn't cluster together). Communities of `>= min_size` become
    /// `cluster:comm-*` nodes (named after their dominant tag) with `InCluster`
    /// edges. Deterministic given a fixed graph (sorted iteration + tie-break).
    /// Returns the number of communities formed.
    pub fn detect_communities(&mut self, min_size: usize, max_iters: usize) -> usize {
        let mut order: Vec<String> = self
            .memories
            .iter()
            .filter(|(_, m)| m.active)
            .map(|(id, _)| id.clone())
            .collect();
        if order.len() < min_size {
            self.clear_communities();
            return 0;
        }
        order.sort();

        // Undirected weighted adjacency over semantic, non-contradicting edges.
        let mut adj: HashMap<String, HashMap<String, f32>> = HashMap::new();
        for (from, edges) in &self.edges {
            if !self.memories.contains_key(from) {
                continue;
            }
            for e in edges {
                if !e.kind.is_semantic()
                    || e.kind == EdgeKind::Contradicts
                    || !self.memories.contains_key(&e.target)
                {
                    continue;
                }
                let w = e.traversal_weight();
                *adj.entry(from.clone()).or_default().entry(e.target.clone()).or_insert(0.0) += w;
                *adj.entry(e.target.clone()).or_default().entry(from.clone()).or_insert(0.0) += w;
            }
        }

        // Label propagation: each node adopts the highest-weighted neighbour label.
        let mut label: HashMap<String, String> =
            order.iter().map(|n| (n.clone(), n.clone())).collect();
        for _ in 0..max_iters {
            let mut changed = false;
            for n in &order {
                let Some(neighbours) = adj.get(n) else {
                    continue;
                };
                if neighbours.is_empty() {
                    continue;
                }
                let mut tally: HashMap<String, f32> = HashMap::new();
                for (m, w) in neighbours {
                    if let Some(l) = label.get(m) {
                        *tally.entry(l.clone()).or_insert(0.0) += *w;
                    }
                }
                // Highest weight wins; deterministic tie-break on smallest label.
                if let Some((best, _)) = tally
                    .into_iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                    && label.get(n) != Some(&best)
                {
                    label.insert(n.clone(), best);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Group members by final label.
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        for (node, lab) in &label {
            groups.entry(lab.clone()).or_default().push(node.clone());
        }

        // Rebuild community clusters from scratch (they're fully derived).
        self.clear_communities();

        let mut formed = 0usize;
        for (_, mut members) in groups {
            if members.len() < min_size {
                continue;
            }
            members.sort();
            let cluster_id = format!("cluster:comm-{}", short_hash(&members));
            let name = self.dominant_tag(&members);
            let now = Utc::now();
            let entry = ClusterEntry {
                id: cluster_id.clone(),
                name,
                centroid: Vec::new(),
                member_count: members.len() as u32,
                created_at: now,
                updated_at: now,
            };
            self.clusters.insert(cluster_id.clone(), entry);
            for m in &members {
                self.add_edge(m, &cluster_id, EdgeKind::InCluster);
            }
            formed += 1;
        }
        formed
    }

    /// Remove all derived community clusters (`cluster:comm-*`) and their
    /// `InCluster` edges. Leaves co-relevance clusters from other subsystems.
    fn clear_communities(&mut self) {
        let comm_ids: HashSet<String> = self
            .clusters
            .keys()
            .filter(|id| id.starts_with("cluster:comm-"))
            .cloned()
            .collect();
        if comm_ids.is_empty() {
            return;
        }
        for edges in self.edges.values_mut() {
            edges.retain(|e| !(e.kind == EdgeKind::InCluster && comm_ids.contains(&e.target)));
        }
        for cid in &comm_ids {
            self.reverse_edges.remove(cid);
            self.clusters.remove(cid);
        }
    }

    /// Most common non-generic tag among a set of memories (for naming a cluster).
    fn dominant_tag(&self, members: &[String]) -> Option<String> {
        const GENERIC: &[&str] = &["sensitive", "general", "misc", "other", "note", "info"];
        let mut counts: HashMap<String, usize> = HashMap::new();
        for id in members {
            if let Some(m) = self.memories.get(id) {
                for t in &m.tags {
                    if !GENERIC.contains(&t.as_str()) {
                        *counts.entry(t.clone()).or_insert(0) += 1;
                    }
                }
            }
        }
        counts
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
            .filter(|(_, c)| *c >= 2)
            .map(|(t, _)| t)
    }

    // ==================== #3 Episodic → semantic ====================

    /// Strength at which a repeatedly-observed fact is promoted to "semantic".
    pub const SEMANTIC_STRENGTH: u32 = 4;

    /// Record another observation of an existing fact (episodic reinforcement).
    /// Raises its evidence-backed confidence and, once enough observations
    /// accumulate, tags it `semantic`. Returns `true` if this call promoted the
    /// memory to semantic.
    pub fn record_fact_observation(&mut self, id: &str, ev: EvidenceRef) -> bool {
        let promote = {
            let Some(m) = self.memories.get_mut(id) else {
                return false;
            };
            let was_semantic = m.tags.iter().any(|t| t == "semantic");
            m.record_evidence(ev);
            !was_semantic && m.strength >= Self::SEMANTIC_STRENGTH
        };
        if promote {
            self.tag_memory(id, "semantic");
        }
        promote
    }

    // ==================== #10 Reasoning primitives ====================

    /// Semantic neighbours of `id` ranked by relationship strength, as
    /// `(kind, target_id, weight, confidence)`. Powers "why is X relevant".
    pub fn ranked_relations(&self, id: &str) -> Vec<(EdgeKind, String, f32, f32)> {
        let mut v: Vec<(EdgeKind, String, f32, f32)> = self
            .semantic_out_edges(id)
            .iter()
            .map(|e| {
                (
                    e.kind,
                    e.target.clone(),
                    e.traversal_weight(),
                    e.meta.confidence,
                )
            })
            .collect();
        v.sort_by(|a, b| b.2.total_cmp(&a.2));
        v
    }

    /// Shortest reasoning path between two memories over semantic edges.
    /// Returns the node sequence with the edge kind taken at each hop:
    /// `[(start, None), (n1, Some(kind0)), …, (end, Some(kindK))]`.
    pub fn shortest_semantic_path(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
    ) -> Option<Vec<(String, Option<EdgeKind>)>> {
        if !self.memories.contains_key(from) || !self.memories.contains_key(to) {
            return None;
        }
        if from == to {
            return Some(vec![(from.to_string(), None)]);
        }
        let mut prev: HashMap<String, (String, EdgeKind)> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::from([from.to_string()]);
        let mut queue: VecDeque<(String, usize)> = VecDeque::from([(from.to_string(), 0usize)]);

        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let mut edges = self.semantic_out_edges(&node);
            edges.sort_by(|a, b| b.traversal_weight().total_cmp(&a.traversal_weight()));
            for e in edges {
                if visited.insert(e.target.clone()) {
                    prev.insert(e.target.clone(), (node.clone(), e.kind));
                    if e.target == to {
                        // Reconstruct.
                        let mut path: Vec<(String, Option<EdgeKind>)> = Vec::new();
                        let mut cur = to.to_string();
                        while let Some((p, k)) = prev.get(&cur) {
                            path.push((cur.clone(), Some(*k)));
                            cur = p.clone();
                        }
                        path.push((from.to_string(), None));
                        path.reverse();
                        return Some(path);
                    }
                    queue.push_back((e.target.clone(), depth + 1));
                }
            }
        }
        None
    }

    /// All memories that contradict `id`.
    pub fn contradictions_of(&self, id: &str) -> Vec<String> {
        self.get_edges(id)
            .iter()
            .filter(|e| e.kind == EdgeKind::Contradicts)
            .map(|e| e.target.clone())
            .collect()
    }

    /// Strongest contradiction in the whole graph, as `(a, b, confidence)`.
    pub fn strongest_contradiction(&self) -> Option<(String, String, f32)> {
        let mut best: Option<(String, String, f32)> = None;
        for (from, edges) in &self.edges {
            for e in edges {
                if e.kind == EdgeKind::Contradicts && from.as_str() < e.target.as_str() {
                    let c = e.meta.confidence.max(e.meta.weight);
                    if best.as_ref().map(|(_, _, bc)| c > *bc).unwrap_or(true) {
                        best = Some((from.clone(), e.target.clone(), c));
                    }
                }
            }
        }
        best
    }

    /// Compare two memories: their direct relation (if any), shared tags, and
    /// shared semantic neighbours.
    pub fn compare_memories(
        &self,
        a: &str,
        b: &str,
    ) -> (Option<EdgeKind>, Vec<String>, Vec<String>) {
        let direct = self
            .get_edges(a)
            .iter()
            .find(|e| e.target == b && e.kind.is_semantic())
            .map(|e| e.kind);

        let tags_a: HashSet<&String> = self.memories.get(a).map(|m| m.tags.iter().collect()).unwrap_or_default();
        let tags_b: HashSet<&String> = self.memories.get(b).map(|m| m.tags.iter().collect()).unwrap_or_default();
        let mut shared_tags: Vec<String> =
            tags_a.intersection(&tags_b).map(|s| (*s).clone()).collect();
        shared_tags.sort();

        let nb_a: HashSet<String> =
            self.semantic_out_edges(a).iter().map(|e| e.target.clone()).collect();
        let nb_b: HashSet<String> =
            self.semantic_out_edges(b).iter().map(|e| e.target.clone()).collect();
        let mut shared_nb: Vec<String> = nb_a.intersection(&nb_b).cloned().collect();
        shared_nb.sort();

        (direct, shared_tags, shared_nb)
    }

    // ==================== #9 Sleep / consolidation cycle ====================

    /// Importance-aware confidence decay: each memory's confidence is
    /// multiplied by a factor between `base` (unimportant) and ~1.0 (important),
    /// so valuable memories fade far more slowly. Returns memories touched.
    pub fn decay_confidence_importance_aware(&mut self, base: f32) -> usize {
        let ids: Vec<String> = self.memories.keys().cloned().collect();
        let imp: HashMap<String, f32> =
            ids.iter().map(|id| (id.clone(), self.importance(id))).collect();
        let mut touched = 0usize;
        for id in ids {
            let importance = imp.get(&id).copied().unwrap_or(0.0);
            let factor = base + (1.0 - base) * importance;
            if let Some(m) = self.memories.get_mut(&id) {
                m.confidence = (m.confidence * factor).clamp(0.0, 1.0);
                touched += 1;
            }
        }
        touched
    }

    /// Run one offline consolidation ("sleep") pass over this graph:
    /// reinforce structural associations, fade noise, detect communities, and
    /// recompute importance-weighted confidence. Returns a report of what
    /// changed. Pure graph work — no user interaction, no network.
    pub fn run_sleep_cycle(&mut self, cfg: SleepConfig) -> SleepReport {
        let linked = self.bootstrap_cooccurrence_links(cfg.cooccurrence_min_overlap);
        let (weakened, pruned) = self.decay_relates_to(cfg.decay_factor, cfg.decay_floor);
        let communities = self.detect_communities(cfg.min_community_size, cfg.label_prop_iters);
        let confidence_decayed = self.decay_confidence_importance_aware(cfg.confidence_decay_base);
        self.metadata.last_cluster_update = Some(Utc::now());
        SleepReport {
            linked,
            weakened,
            pruned,
            communities,
            confidence_decayed,
            at: Some(Utc::now()),
            ..Default::default()
        }
    }

    // ==================== #1 Semantic consolidation ====================

    /// Groups of episodic (non-semantic) active memories that describe the same
    /// concept and are ripe for consolidation. Uses detected communities as the
    /// grouping signal. Deterministic (sorted by cluster id, members sorted).
    pub fn consolidation_candidates(&self, min_size: usize) -> Vec<Vec<String>> {
        let mut comm_ids: Vec<String> = self
            .clusters
            .keys()
            .filter(|id| id.starts_with("cluster:comm-"))
            .cloned()
            .collect();
        comm_ids.sort();

        let mut groups: Vec<Vec<String>> = Vec::new();
        for cid in comm_ids {
            let mut members: Vec<String> = Vec::new();
            for (mid, edges) in &self.edges {
                if edges.iter().any(|e| e.kind == EdgeKind::InCluster && e.target == cid) {
                    let keep = self
                        .memories
                        .get(mid)
                        .map(|m| m.active && !m.tags.iter().any(|t| t == "semantic"))
                        .unwrap_or(false);
                    if keep {
                        members.push(mid.clone());
                    }
                }
            }
            members.sort();
            members.dedup();
            if members.len() >= min_size {
                groups.push(members);
            }
        }
        groups
    }

    /// Create or update the semantic memory for a consolidation group. Idempotent:
    /// the semantic id is derived from the sorted member ids, so re-running with
    /// the same group updates the same node. Links episodes via `DerivedFrom`,
    /// aggregates evidence + confidence, and `Supersedes` prior semantic
    /// summaries whose sources are a subset. **Never deletes originals.**
    ///
    /// `summary` is the merged text (from the sidecar, with a caller fallback);
    /// `concept` is a short human label. Returns the semantic memory id.
    pub fn apply_consolidation(
        &mut self,
        member_ids: &[String],
        summary: &str,
        concept: &str,
    ) -> Option<String> {
        let mut members: Vec<String> = member_ids
            .iter()
            .filter(|id| self.memories.contains_key(*id))
            .cloned()
            .collect();
        members.sort();
        members.dedup();
        if members.len() < 2 {
            return None;
        }
        let now = Utc::now();
        let sem_id = format!("mem-sem-{}", short_hash(&members));
        let summary = summary.trim();
        if summary.is_empty() {
            return None;
        }

        // Aggregate confidence (strongest instance, capped) + inherited tags.
        let category = self.memories.get(&members[0]).map(|m| m.category.clone())?;
        let mut max_conf = 0.0f32;
        let mut tags: Vec<String> = Vec::new();
        for id in &members {
            if let Some(m) = self.memories.get(id) {
                if m.confidence > max_conf {
                    max_conf = m.confidence;
                }
                for t in &m.tags {
                    if !tags.contains(t) {
                        tags.push(t.clone());
                    }
                }
            }
        }
        for t in ["semantic", "consolidated"] {
            if !tags.iter().any(|x| x == t) {
                tags.push(t.to_string());
            }
        }
        let agg_conf = max_conf.min(0.97);
        let evidence: Vec<EvidenceRef> = members.iter().map(EvidenceRef::memory).collect();

        let existed = self.memories.contains_key(&sem_id);
        if existed {
            if let Some(e) = self.memories.get_mut(&sem_id) {
                e.content = summary.to_string();
                e.category = category;
                e.confidence = e.confidence.max(agg_conf);
                for t in &tags {
                    if !e.tags.contains(t) {
                        e.tags.push(t.clone());
                    }
                }
                e.evidence = evidence.clone();
                e.refresh_search_text();
                e.updated_at = now;
            }
        } else {
            let mut e = MemoryEntry::new(category, summary.to_string());
            e.id = sem_id.clone();
            e.tags = tags.clone();
            e.confidence = agg_conf;
            e.evidence = evidence.clone();
            self.add_memory(e);
        }

        // Link episodes -> semantic memory via DerivedFrom (provenance).
        for src in &members {
            self.add_typed_edge(
                &sem_id,
                src,
                EdgeKind::DerivedFrom,
                0.8,
                EdgeSource::System,
                Some(EvidenceRef::memory(src)),
            );
        }

        // Supersede prior semantic summaries whose sources are a subset.
        let member_set: HashSet<&String> = members.iter().collect();
        let prior: Vec<String> = self
            .memories
            .iter()
            .filter(|(id, m)| {
                id.as_str() != sem_id
                    && m.active
                    && m.tags.iter().any(|t| t == "semantic")
            })
            .filter_map(|(id, _)| {
                let srcs = self.derived_sources(id);
                if !srcs.is_empty() && srcs.iter().all(|s| member_set.contains(s)) {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        for old in prior {
            self.add_typed_edge(
                &sem_id,
                &old,
                EdgeKind::Supersedes,
                0.9,
                EdgeSource::System,
                Some(EvidenceRef::observation("re-consolidated into a broader concept")),
            );
        }

        // Record consolidation history (bounded).
        self.metadata.consolidations.push(ConsolidationRecord {
            semantic_id: sem_id.clone(),
            sources: members.clone(),
            concept: concept.trim().to_string(),
            at: now,
        });
        const MAX_HISTORY: usize = 200;
        if self.metadata.consolidations.len() > MAX_HISTORY {
            let overflow = self.metadata.consolidations.len() - MAX_HISTORY;
            self.metadata.consolidations.drain(0..overflow);
        }

        Some(sem_id)
    }

    /// Episodic sources a semantic memory was DerivedFrom (its provenance).
    pub fn derived_sources(&self, id: &str) -> Vec<String> {
        self.edges
            .get(id)
            .map(|edges| {
                let mut v: Vec<String> = edges
                    .iter()
                    .filter(|e| e.kind == EdgeKind::DerivedFrom)
                    .map(|e| e.target.clone())
                    .collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    // ==================== #2 Contradiction review (apply side) ====================

    /// Candidate pairs of high-confidence semantic memories to submit to the
    /// LLM reviewer. Bounded and deterministic. Skips pairs that already have a
    /// `Contradicts` edge. Returns `(id_a, id_b)` with `id_a < id_b`.
    pub fn contradiction_candidates(&self, min_conf: f32, max_pairs: usize) -> Vec<(String, String)> {
        let mut sem: Vec<String> = self
            .memories
            .iter()
            .filter(|(_, m)| {
                m.active && m.confidence >= min_conf && m.tags.iter().any(|t| t == "semantic")
            })
            .map(|(id, _)| id.clone())
            .collect();
        sem.sort();

        let mut pairs = Vec::new();
        for i in 0..sem.len() {
            for j in (i + 1)..sem.len() {
                if pairs.len() >= max_pairs {
                    return pairs;
                }
                let already = self
                    .edges
                    .get(&sem[i])
                    .map(|es| es.iter().any(|e| e.kind == EdgeKind::Contradicts && e.target == sem[j]))
                    .unwrap_or(false);
                if !already {
                    pairs.push((sem[i].clone(), sem[j].clone()));
                }
            }
        }
        pairs
    }

    /// Apply a reviewer's contradiction finding as graph knowledge only: a
    /// symmetric `Contradicts` edge carrying the reasoning as evidence. Never
    /// modifies memory content or confidence.
    pub fn apply_contradiction(&mut self, a: &str, b: &str, reason: &str) {
        if !self.memories.contains_key(a) || !self.memories.contains_key(b) {
            return;
        }
        self.add_typed_edge(
            a,
            b,
            EdgeKind::Contradicts,
            0.8,
            EdgeSource::Llm,
            Some(EvidenceRef::observation(reason.trim())),
        );
    }

    // ==================== #3 Concept (graph-neighborhood) embeddings ====================

    /// Build the text that represents a *concept* — this memory together with
    /// its semantic neighborhood — used to compute a concept embedding.
    /// Incorporates: memory text, typed relationships + neighbor snippets,
    /// community label, important supporting facts, and edge weights.
    pub fn build_concept_text(&self, id: &str) -> Option<String> {
        let m = self.memories.get(id)?;
        if !m.active {
            return None;
        }
        let mut out = String::new();
        out.push_str(m.content.trim());

        // Community label, if the memory belongs to a detected community.
        if let Some(edges) = self.edges.get(id) {
            for e in edges {
                if e.kind == EdgeKind::InCluster
                    && let Some(c) = self.clusters.get(&e.target)
                    && let Some(name) = c.name.as_deref()
                {
                    out.push_str(&format!("\n[concept: {name}]"));
                    break;
                }
            }
        }

        // Typed relationships + neighbor snippets, strongest first, bounded.
        let mut rel: Vec<(f32, String)> = Vec::new();
        if let Some(edges) = self.edges.get(id) {
            for e in edges {
                if !e.kind.is_semantic() {
                    continue;
                }
                if let Some(n) = self.memories.get(&e.target) {
                    let snippet: String = n.content.chars().take(80).collect();
                    let w = e.traversal_weight();
                    rel.push((
                        w,
                        format!("{} ({:.2}): {}", e.kind.label(), w, snippet.trim()),
                    ));
                }
            }
        }
        rel.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        for (_, line) in rel.into_iter().take(6) {
            out.push('\n');
            out.push_str(&line);
        }

        // Important supporting facts (Supports edges) already included above via
        // typed relationships; also fold in the memory's own strongest tags.
        if !m.tags.is_empty() {
            let mut tags = m.tags.clone();
            tags.sort();
            out.push_str(&format!("\n[tags: {}]", tags.join(", ")));
        }

        Some(out)
    }

    /// Recompute concept embeddings for all active memories using the provided
    /// embedder. Deterministic given a deterministic embedder. Returns count.
    pub fn refresh_concept_embeddings<F>(&mut self, embed: F) -> usize
    where
        F: Fn(&str) -> Option<Vec<f32>>,
    {
        let ids: Vec<String> = self
            .memories
            .iter()
            .filter(|(_, m)| m.active)
            .map(|(id, _)| id.clone())
            .collect();
        let mut refreshed = 0usize;
        for id in ids {
            if let Some(text) = self.build_concept_text(&id)
                && let Some(vec) = embed(&text)
                && let Some(m) = self.memories.get_mut(&id)
            {
                m.concept_embedding = Some(vec);
                refreshed += 1;
            }
        }
        if refreshed > 0 {
            self.metadata.last_concept_embed = Some(Utc::now());
        }
        refreshed
    }

    // ==================== #6 Graph integrity ====================

    /// Validate structural + semantic invariants of the graph. Returns a list
    /// of issues (empty ⇒ healthy). Read-only; pair with [`repair`] to fix.
    pub fn validate(&self) -> Vec<GraphIssue> {
        let mut issues = Vec::new();

        let node_exists = |id: &str| -> bool {
            self.memories.contains_key(id)
                || self.tags.contains_key(id)
                || self.clusters.contains_key(id)
        };

        // Dangling edges + out-of-range edge confidence/weight + evidence sanity.
        for (from, edges) in &self.edges {
            if !self.memories.contains_key(from) {
                issues.push(GraphIssue::DanglingEdgeSource { from: from.clone() });
            }
            for e in edges {
                if !node_exists(&e.target) {
                    issues.push(GraphIssue::DanglingEdgeTarget {
                        from: from.clone(),
                        to: e.target.clone(),
                        kind: e.kind.label(),
                    });
                }
                if !(0.0..=1.0).contains(&e.meta.weight)
                    || !(0.0..=1.0).contains(&e.meta.confidence)
                {
                    issues.push(GraphIssue::EdgeConfidenceOutOfRange {
                        from: from.clone(),
                        to: e.target.clone(),
                        weight: e.meta.weight,
                        confidence: e.meta.confidence,
                    });
                }
                if (e.meta.evidence.len() as u32) > e.meta.evidence_count {
                    issues.push(GraphIssue::EvidenceCountMismatch {
                        from: from.clone(),
                        to: e.target.clone(),
                        count: e.meta.evidence_count,
                        stored: e.meta.evidence.len(),
                    });
                }
            }
        }

        // Reverse-edge consistency: every forward edge has a reverse entry.
        for (from, edges) in &self.edges {
            for e in edges {
                let has_rev = self
                    .reverse_edges
                    .get(&e.target)
                    .map(|srcs| srcs.iter().any(|s| s == from))
                    .unwrap_or(false);
                if !has_rev {
                    issues.push(GraphIssue::MissingReverseEdge {
                        from: from.clone(),
                        to: e.target.clone(),
                    });
                }
            }
        }

        // Memory confidence bounds + fact evidence sanity.
        for (id, m) in &self.memories {
            if !(0.0..=1.0).contains(&m.confidence) {
                issues.push(GraphIssue::MemoryConfidenceOutOfRange {
                    id: id.clone(),
                    confidence: m.confidence,
                });
            }
        }

        // Symmetric edge rule for Contradicts / SimilarTo.
        for (from, edges) in &self.edges {
            for e in edges {
                if e.kind.is_symmetric()
                    && e.kind != EdgeKind::InCluster
                    && self.memories.contains_key(&e.target)
                {
                    let mirrored = self
                        .edges
                        .get(&e.target)
                        .map(|es| es.iter().any(|x| x.target == *from && x.kind == e.kind))
                        .unwrap_or(false);
                    if !mirrored {
                        issues.push(GraphIssue::AsymmetricEdge {
                            from: from.clone(),
                            to: e.target.clone(),
                            kind: e.kind.label(),
                        });
                    }
                }
            }
        }

        // Duplicate semantic memories (same normalized content).
        let mut seen: HashMap<String, String> = HashMap::new();
        for (id, m) in &self.memories {
            if m.active && m.tags.iter().any(|t| t == "semantic") {
                let key = m.content.trim().to_lowercase();
                if let Some(prev) = seen.get(&key) {
                    issues.push(GraphIssue::DuplicateSemanticMemory {
                        a: prev.clone(),
                        b: id.clone(),
                    });
                } else {
                    seen.insert(key, id.clone());
                }
            }
        }

        // Cyclic Supersedes chains (newer -> older should be a DAG).
        if let Some(cycle) = self.find_supersedes_cycle() {
            issues.push(GraphIssue::CyclicSupersedes { chain: cycle });
        }

        issues
    }

    fn find_supersedes_cycle(&self) -> Option<Vec<String>> {
        // Build adjacency of Supersedes edges only.
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for (from, edges) in &self.edges {
            for e in edges {
                if e.kind == EdgeKind::Supersedes {
                    adj.entry(from.as_str()).or_default().push(e.target.as_str());
                }
            }
        }
        let mut color: HashMap<&str, u8> = HashMap::new(); // 0=unseen,1=stack,2=done
        let mut stack: Vec<(&str, usize)> = Vec::new();
        let mut path: Vec<&str> = Vec::new();
        let nodes: Vec<&str> = adj.keys().copied().collect();
        for &start in &nodes {
            if color.get(start).copied().unwrap_or(0) != 0 {
                continue;
            }
            stack.push((start, 0));
            path.push(start);
            color.insert(start, 1);
            while let Some(&(node, idx)) = stack.last() {
                let nbrs = adj.get(node).map(|v| v.as_slice()).unwrap_or(&[]);
                if idx < nbrs.len() {
                    stack.last_mut().unwrap().1 += 1;
                    let next = nbrs[idx];
                    match color.get(next).copied().unwrap_or(0) {
                        1 => {
                            // Back-edge → cycle. Extract the loop from path.
                            let mut chain: Vec<String> =
                                path.iter().map(|s| s.to_string()).collect();
                            chain.push(next.to_string());
                            return Some(chain);
                        }
                        0 => {
                            color.insert(next, 1);
                            path.push(next);
                            stack.push((next, 0));
                        }
                        _ => {}
                    }
                } else {
                    color.insert(node, 2);
                    stack.pop();
                    path.pop();
                }
            }
        }
        None
    }

    /// Best-effort repair of mechanical issues: drop dangling edges, rebuild
    /// reverse index, clamp confidences into `[0,1]`. Returns issues fixed.
    /// Never touches memory content or semantic relationships beyond dropping
    /// edges that point at nothing.
    pub fn repair(&mut self) -> usize {
        let mut fixed = 0usize;
        let valid: HashSet<String> = self
            .memories
            .keys()
            .chain(self.tags.keys())
            .chain(self.clusters.keys())
            .cloned()
            .collect();

        // Drop edges from non-memory sources or to missing targets; clamp values.
        let sources: Vec<String> = self.edges.keys().cloned().collect();
        for from in sources {
            if !self.memories.contains_key(&from) {
                if let Some(removed) = self.edges.remove(&from) {
                    fixed += removed.len();
                }
                continue;
            }
            if let Some(edges) = self.edges.get_mut(&from) {
                let before = edges.len();
                edges.retain(|e| valid.contains(&e.target));
                fixed += before - edges.len();
                for e in edges.iter_mut() {
                    e.meta.weight = e.meta.weight.clamp(0.0, 1.0);
                    e.meta.confidence = e.meta.confidence.clamp(0.0, 1.0);
                }
            }
        }

        for m in self.memories.values_mut() {
            if !(0.0..=1.0).contains(&m.confidence) {
                m.confidence = m.confidence.clamp(0.0, 1.0);
                fixed += 1;
            }
        }

        // Rebuild the reverse index from scratch (authoritative).
        self.rebuild_reverse_index();
        fixed
    }

    fn rebuild_reverse_index(&mut self) {
        self.reverse_edges.clear();
        for (from, edges) in &self.edges {
            for e in edges {
                self.reverse_edges
                    .entry(e.target.clone())
                    .or_default()
                    .push(from.clone());
            }
        }
    }

    // ==================== Migration ====================

    /// Convert a legacy MemoryStore to a MemoryGraph
    ///
    /// This handles migration from the old flat JSON format to the graph format.
    pub fn from_legacy_store(store: MemoryStore) -> Self {
        let mut graph = MemoryGraph::new();

        for entry in store.entries {
            let memory_id = entry.id.clone();
            let tags = entry.tags.clone();
            let superseded_by = entry.superseded_by.clone();

            // Add memory (this will also create tag nodes and HasTag edges)
            graph.memories.insert(memory_id.clone(), entry);

            // Create tag nodes and edges
            for tag_name in &tags {
                graph.ensure_tag(tag_name);
                let tag_id = format!("tag:{}", tag_name);
                graph.add_edge_internal(&memory_id, &tag_id, EdgeKind::HasTag);

                // Update tag count
                if let Some(tag) = graph.tags.get_mut(&tag_id) {
                    tag.count += 1;
                }
            }

            // Create Supersedes edge if applicable
            if let Some(ref newer_id) = superseded_by {
                // newer_id supersedes memory_id
                graph.add_edge_internal(newer_id, &memory_id, EdgeKind::Supersedes);
            }
        }

        graph
    }

    /// Check if this graph was migrated from legacy format
    pub fn is_migrated(&self) -> bool {
        self.graph_version == GRAPH_VERSION
    }
}

#[cfg(test)]
#[path = "memory_graph_tests.rs"]
mod memory_graph_tests;
