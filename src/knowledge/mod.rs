//! Unified knowledge-source layer.
//!
//! "Everything becomes knowledge": a [`KnowledgeSource`] turns an external
//! origin — a repository first; conversations, documentation, APIs, websites,
//! PDFs, and logs over time — into semantic concepts inside the *existing*
//! memory graph.
//!
//! This module deliberately owns no storage, no retrieval, no embeddings, no
//! reasoning, and no persistence of its own. A source only produces
//! [`SourceUnit`] values (concept statements plus typed relations plus
//! evidence); everything downstream — sleep cycles, Hebbian association,
//! community detection, consolidation, concept embeddings, cascade retrieval,
//! MMR selection, contradiction review — is the memory system that already
//! exists. Repository knowledge is therefore never a parallel subsystem: once
//! ingested, the graph cannot tell (and should not care) whether a concept
//! came from a conversation or a codebase.
//!
//! Design rules, mirrored from the memory graph:
//!   * **Deterministic where possible.** Discovery, extraction, fingerprints,
//!     ids, and edges are pure functions of the source. The sidecar LLM is
//!     used only to upgrade structural summaries into architectural prose,
//!     and every sidecar step has a structural fallback.
//!   * **Incremental and resumable.** A manifest of per-item fingerprints is
//!     persisted in [`GraphMetadata`]; only changed items are re-extracted,
//!     bounded per pass, and unfinished work carries over.
//!   * **Never delete.** Concepts whose backing items disappear are retired
//!     (`active = false`), preserving history and provenance; if the item
//!     reappears the same deterministic id reactivates.
//!   * **Never store confidence without evidence.** Structure, git commits,
//!     and tool outcomes all land as [`EvidenceRef`]s.

pub mod engineering;
pub mod evidence;
pub mod insights;
pub mod reasoning;
pub mod repo;
pub mod verify;

use crate::memory::{MemoryEntry, MemoryManager};
use crate::memory_graph::{EdgeKind, EdgeSource, EvidenceRef, MemoryGraph};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Schema version for the persisted per-source state.
pub const KNOWLEDGE_STATE_VERSION: u32 = 1;

/// Where a piece of knowledge originally came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeSourceKind {
    Repository,
    Conversation,
    Documentation,
    Api,
    Website,
    Pdf,
    Log,
    Tool,
}

impl KnowledgeSourceKind {
    pub fn label(&self) -> &'static str {
        match self {
            KnowledgeSourceKind::Repository => "repository",
            KnowledgeSourceKind::Conversation => "conversation",
            KnowledgeSourceKind::Documentation => "documentation",
            KnowledgeSourceKind::Api => "api",
            KnowledgeSourceKind::Website => "website",
            KnowledgeSourceKind::Pdf => "pdf",
            KnowledgeSourceKind::Log => "log",
            KnowledgeSourceKind::Tool => "tool",
        }
    }
}

impl std::fmt::Display for KnowledgeSourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Deterministic manifest of a source: item key → structure fingerprint.
/// Item keys are source-relative (file paths for repositories).
#[derive(Debug, Clone, Default)]
pub struct SourceManifest {
    pub items: BTreeMap<String, String>,
}

/// A typed relation from one unit to another, by unit key.
#[derive(Debug, Clone)]
pub struct UnitRelation {
    pub kind: EdgeKind,
    pub target_key: String,
    pub weight: f32,
}

/// One concept extracted from a source. Content is a concept-level statement
/// (what this is, what it is responsible for) — never a raw file dump.
#[derive(Debug, Clone)]
pub struct SourceUnit {
    /// Stable key within the source, e.g. `module:src/agent/turn_loops.rs`.
    pub key: String,
    /// Deterministic (structural) concept statement.
    pub content: String,
    pub category: crate::memory::MemoryCategory,
    pub tags: Vec<String>,
    /// Manifest items this unit was derived from (files for repositories).
    pub derived_from_items: Vec<String>,
    pub relations: Vec<UnitRelation>,
    pub evidence: Vec<EvidenceRef>,
    /// Whether the sidecar should later upgrade this concept into
    /// architectural prose (responsibility / intent / integration points).
    pub wants_abstraction: bool,
}

/// A knowledge source: something Neura can continuously learn from.
///
/// `discover` and `extract` must be deterministic; the shared pipeline owns
/// diffing, persistence, embeddings, evidence, retirement, and the optional
/// semantic-abstraction pass.
pub trait KnowledgeSource {
    fn kind(&self) -> KnowledgeSourceKind;
    /// Stable identity, e.g. `repo:/abs/path`.
    fn source_id(&self) -> String;
    fn display_name(&self) -> String;
    /// Locator string sufficient to reconstruct the source (path for repos).
    fn locator(&self) -> String;
    /// Enumerate current items with structure fingerprints.
    fn discover(&mut self) -> Result<SourceManifest>;
    /// Extract concept units. `changed_items` lists per-item units to
    /// (re)build; sources may additionally emit aggregate units (repository /
    /// package level) that are cheap to regenerate every pass.
    fn extract(&mut self, changed_items: &[String], manifest: &SourceManifest)
    -> Result<Vec<SourceUnit>>;
}

/// What one ingest pass changed. Mirrors [`crate::memory_graph::SleepReport`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestReport {
    pub items_seen: usize,
    pub items_changed: usize,
    pub items_removed: usize,
    /// Changed items deferred to the next pass by the per-pass bound.
    pub items_deferred: usize,
    pub concepts_created: usize,
    pub concepts_updated: usize,
    pub concepts_unchanged: usize,
    pub concepts_retired: usize,
    pub concepts_reactivated: usize,
    pub edges_added: usize,
    pub evidence_recorded: usize,
    pub embeddings_generated: usize,
    pub abstracted: usize,
    pub abstraction_pending: usize,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

impl IngestReport {
    pub fn render(&self, name: &str) -> String {
        format!(
            "{name}: {} items ({} changed, {} removed, {} deferred) → \
             concepts +{} ~{} retired {} reactivated {} · edges +{} · \
             evidence +{} · embeddings +{} · abstracted {} ({} pending) · {} ms",
            self.items_seen,
            self.items_changed,
            self.items_removed,
            self.items_deferred,
            self.concepts_created,
            self.concepts_updated,
            self.concepts_retired,
            self.concepts_reactivated,
            self.edges_added,
            self.evidence_recorded,
            self.embeddings_generated,
            self.abstracted,
            self.abstraction_pending,
            self.duration_ms,
        )
    }
}

/// One point on a source's architectural-evolution timeline (recorded on
/// ingest passes that changed something; bounded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionPoint {
    pub at: DateTime<Utc>,
    pub items: usize,
    pub active_concepts: usize,
    pub concepts_created: usize,
    pub concepts_updated: usize,
    pub concepts_retired: usize,
    /// Sample of the items that changed in this pass (bounded) — feeds
    /// co-change observations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_sample: Vec<String>,
}

/// Persisted per-source incremental state. Lives inside [`GraphMetadata`] so
/// knowledge sources reuse the graph's own persistence — no side store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSourceState {
    #[serde(default)]
    pub version: u32,
    pub source_id: String,
    pub kind: KnowledgeSourceKind,
    /// Enough to reconstruct the source (root path for repositories).
    pub locator: String,
    /// item key → structure fingerprint at last successful extraction.
    #[serde(default)]
    pub fingerprints: BTreeMap<String, String>,
    /// unit key → memory id (deterministic, but cached for reverse lookups).
    #[serde(default)]
    pub unit_ids: BTreeMap<String, String>,
    /// item key → unit keys derived from it (for retirement + tool evidence).
    #[serde(default)]
    pub item_units: BTreeMap<String, Vec<String>>,
    /// unit key → hash of the last *structural* content extracted. Lets the
    /// pipeline detect real structural change without clobbering
    /// sidecar-abstracted prose on no-op re-ingests.
    #[serde(default)]
    pub unit_content_hash: BTreeMap<String, String>,
    /// Unit keys awaiting sidecar architectural abstraction (bounded, FIFO).
    #[serde(default)]
    pub pending_abstraction: Vec<String>,
    /// Bounded architectural-evolution timeline: one point per ingest pass
    /// that changed something, enabling reasoning across history ("how has
    /// this grown", "what changed together").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<EvolutionPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ingest: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report: Option<IngestReport>,
}

impl KnowledgeSourceState {
    fn new(source: &dyn KnowledgeSource) -> Self {
        Self {
            version: KNOWLEDGE_STATE_VERSION,
            source_id: source.source_id(),
            kind: source.kind(),
            locator: source.locator(),
            fingerprints: BTreeMap::new(),
            unit_ids: BTreeMap::new(),
            item_units: BTreeMap::new(),
            unit_content_hash: BTreeMap::new(),
            pending_abstraction: Vec::new(),
            history: Vec::new(),
            last_ingest: None,
            last_report: None,
        }
    }
}

/// Tunables for one ingest pass.
#[derive(Debug, Clone, Copy)]
pub struct IngestOptions {
    /// Re-extract everything regardless of fingerprints.
    pub full: bool,
    /// Max changed items processed this pass (rest deferred, resumable).
    pub max_items_per_pass: usize,
    /// Max embeddings generated this pass (rest via existing backfill).
    pub max_embeddings_per_pass: usize,
    /// Max sidecar abstraction upgrades this pass (0 disables).
    pub abstraction_budget: usize,
}

impl Default for IngestOptions {
    fn default() -> Self {
        Self {
            full: false,
            max_items_per_pass: 600,
            max_embeddings_per_pass: 200,
            abstraction_budget: 8,
        }
    }
}

/// Cap on the pending-abstraction queue so a huge repo cannot grow it forever.
const MAX_PENDING_ABSTRACTION: usize = 512;
/// Marker tag for concepts whose prose came from the sidecar.
pub const TAG_ABSTRACTED: &str = "abstracted";
/// Marker tag for retired concepts (backing item disappeared).
pub const TAG_RETIRED: &str = "retired";

/// Deterministic memory id for a unit: stable across runs and machines so
/// re-ingesting is idempotent (mirrors the `mem-sem-<hash>` convention).
pub fn unit_memory_id(source_id: &str, unit_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(unit_key.as_bytes());
    let digest = hasher.finalize();
    format!("mem-src-{}", hex::encode(&digest[..8]))
}

/// Stable content hash used for structural-change detection.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// Ingest a source into the project graph and persist. This is the standalone
/// entry point (CLI / tools); [`refresh_sources_in_graph`] is the maintenance
/// entry point used by the sleep cycle on an already-loaded graph.
pub async fn ingest_source(
    manager: &MemoryManager,
    source: &mut dyn KnowledgeSource,
    opts: IngestOptions,
) -> Result<IngestReport> {
    let mut graph = manager.load_project_graph()?;
    let report = ingest_source_into_graph(&mut graph, source, opts).await?;
    manager.save_project_graph(&graph)?;
    Ok(report)
}

/// Run one incremental ingest pass for `source` against `graph`, updating the
/// graph and the persisted source state in place. Does not save the graph.
pub async fn ingest_source_into_graph(
    graph: &mut MemoryGraph,
    source: &mut dyn KnowledgeSource,
    opts: IngestOptions,
) -> Result<IngestReport> {
    let started = std::time::Instant::now();
    let source_id = source.source_id();
    let mut state = graph
        .metadata
        .knowledge_sources
        .get(&source_id)
        .cloned()
        .unwrap_or_else(|| KnowledgeSourceState::new(source));
    // Keep the locator fresh (a repo may have been moved).
    state.locator = source.locator();

    let manifest = source.discover()?;
    let mut report = IngestReport {
        items_seen: manifest.items.len(),
        ..Default::default()
    };

    // ---- Diff against the persisted fingerprints ----
    let mut changed: Vec<String> = manifest
        .items
        .iter()
        .filter(|(key, fp)| opts.full || state.fingerprints.get(*key) != Some(*fp))
        .map(|(key, _)| key.clone())
        .collect();
    changed.sort();
    let removed: Vec<String> = state
        .fingerprints
        .keys()
        .filter(|key| !manifest.items.contains_key(*key))
        .cloned()
        .collect();
    report.items_changed = changed.len();
    report.items_removed = removed.len();

    // Bound the pass; deferred items keep their stale fingerprints so the
    // next pass picks them up (resumable).
    if changed.len() > opts.max_items_per_pass {
        report.items_deferred = changed.len() - opts.max_items_per_pass;
        changed.truncate(opts.max_items_per_pass);
    }

    // ---- Retire concepts whose backing items are all gone (never delete).
    // Aggregate concepts (packages) are backed by many items and survive
    // until every backing item disappears.
    let mut retire_candidates: BTreeSet<String> = BTreeSet::new();
    for item in &removed {
        if let Some(unit_keys) = state.item_units.remove(item) {
            retire_candidates.extend(unit_keys);
        }
        state.fingerprints.remove(item);
    }
    for key in retire_candidates {
        let still_backed = state
            .item_units
            .values()
            .any(|units| units.iter().any(|k| k == &key));
        if still_backed {
            continue;
        }
        let Some(id) = state.unit_ids.get(&key).cloned() else {
            continue;
        };
        let retired = if let Some(m) = graph.get_memory_mut(&id)
            && m.active
        {
            m.active = false;
            m.record_evidence(EvidenceRef::observation(
                "backing source items removed; concept retired",
            ));
            true
        } else {
            false
        };
        if retired {
            graph.tag_memory(&id, TAG_RETIRED);
            report.concepts_retired += 1;
            report.evidence_recorded += 1;
        }
    }

    // ---- Extract and upsert concept units ----
    if !changed.is_empty() || opts.full {
        let units = source.extract(&changed, &manifest)?;
        upsert_units(graph, &mut state, &source_id, &units, opts, &mut report);
    }

    // Record the fingerprints of what was actually processed.
    for item in &changed {
        if let Some(fp) = manifest.items.get(item) {
            state.fingerprints.insert(item.clone(), fp.clone());
        }
    }
    // Items that were already up to date keep their fingerprints; prune any
    // stragglers for items that no longer exist (handled above via removal).

    // ---- Optional sidecar semantic-abstraction pass (bounded, resumable) ----
    let sidecar_on = crate::memory::memory_sidecar_enabled();
    if opts.abstraction_budget > 0 && sidecar_on {
        report.abstracted =
            drain_abstraction_queue(graph, &mut state, opts.abstraction_budget).await;
    }
    report.abstraction_pending = state.pending_abstraction.len();

    report.duration_ms = started.elapsed().as_millis() as u64;
    report.at = Some(Utc::now());
    state.last_ingest = Some(Utc::now());
    state.last_report = Some(report.clone());

    // Architectural evolution: one bounded timeline point per pass that
    // changed something (plus the very first pass).
    if report.items_changed > 0 || report.items_removed > 0 || state.history.is_empty() {
        let active_concepts = state
            .unit_ids
            .values()
            .filter(|id| graph.get_memory(id).map(|m| m.active).unwrap_or(false))
            .count();
        state.history.push(EvolutionPoint {
            at: Utc::now(),
            items: report.items_seen,
            active_concepts,
            concepts_created: report.concepts_created,
            concepts_updated: report.concepts_updated,
            concepts_retired: report.concepts_retired,
            changed_sample: changed.iter().take(10).cloned().collect(),
        });
        const MAX_EVOLUTION_HISTORY: usize = 60;
        if state.history.len() > MAX_EVOLUTION_HISTORY {
            let overflow = state.history.len() - MAX_EVOLUTION_HISTORY;
            state.history.drain(0..overflow);
        }
    }
    graph
        .metadata
        .knowledge_sources
        .insert(source_id.clone(), state);

    crate::memory_log::log_knowledge(
        "knowledge_ingest",
        serde_json::json!({
            "source": source_id,
            "items_seen": report.items_seen,
            "items_changed": report.items_changed,
            "items_removed": report.items_removed,
            "items_deferred": report.items_deferred,
            "concepts_created": report.concepts_created,
            "concepts_updated": report.concepts_updated,
            "concepts_retired": report.concepts_retired,
            "edges_added": report.edges_added,
            "abstracted": report.abstracted,
            "duration_ms": report.duration_ms,
        }),
    );

    Ok(report)
}

/// Upsert extracted units into the graph: idempotent ids, structural-change
/// detection, tag/evidence merge, typed edges, bounded embedding generation.
fn upsert_units(
    graph: &mut MemoryGraph,
    state: &mut KnowledgeSourceState,
    source_id: &str,
    units: &[SourceUnit],
    opts: IngestOptions,
    report: &mut IngestReport,
) {
    let embed_on = crate::embedding::is_model_available();
    let mut embed_budget = opts.max_embeddings_per_pass;

    for unit in units {
        let id = unit_memory_id(source_id, &unit.key);
        let new_hash = content_hash(&unit.content);
        let structurally_changed =
            state.unit_content_hash.get(&unit.key) != Some(&new_hash);

        let existed = graph.get_memory(&id).is_some();
        if existed {
            let mut reactivated = false;
            if let Some(m) = graph.get_memory_mut(&id) {
                if !m.active {
                    m.active = true;
                    m.superseded_by = None;
                    reactivated = true;
                }
                if structurally_changed {
                    // Real structural change: replace content, invalidate
                    // embeddings so they regenerate through existing paths.
                    m.content = unit.content.clone();
                    m.embedding = None;
                    m.concept_embedding = None;
                    m.updated_at = Utc::now();
                    for ev in &unit.evidence {
                        m.record_evidence(ev.clone());
                        report.evidence_recorded += 1;
                    }
                    report.concepts_updated += 1;
                } else {
                    report.concepts_unchanged += 1;
                }
                m.refresh_search_text();
            }
            if reactivated {
                graph.untag_memory(&id, TAG_RETIRED);
                report.concepts_reactivated += 1;
            }
            if structurally_changed {
                // Stale sidecar prose: the concept re-queues for abstraction.
                graph.untag_memory(&id, TAG_ABSTRACTED);
            }
            for tag in &unit.tags {
                graph.tag_memory(&id, tag);
            }
        } else {
            let mut entry = MemoryEntry::new(unit.category.clone(), unit.content.clone());
            entry.id = id.clone();
            entry.tags = unit.tags.clone();
            entry.source = Some(source_id.to_string());
            entry.trust = crate::memory::TrustLevel::Medium;
            // Structure-derived facts start at moderate confidence; evidence
            // accrual below raises it ("never confidence without evidence").
            entry.confidence = 0.6;
            entry.refresh_search_text();
            graph.add_memory(entry);
            if let Some(m) = graph.get_memory_mut(&id) {
                for ev in &unit.evidence {
                    m.record_evidence(ev.clone());
                    report.evidence_recorded += 1;
                }
            }
            report.concepts_created += 1;
        }

        // Bounded embedding generation; the rest flows through the existing
        // backfill + sleep concept-embedding refresh.
        if embed_on
            && embed_budget > 0
            && let Some(m) = graph.get_memory_mut(&id)
            && !m.has_embedding()
            && m.ensure_embedding()
        {
            embed_budget -= 1;
            report.embeddings_generated += 1;
        }

        // Queue for sidecar architectural abstraction when structure changed.
        if unit.wants_abstraction
            && structurally_changed
            && !state.pending_abstraction.iter().any(|k| k == &unit.key)
        {
            state.pending_abstraction.push(unit.key.clone());
            if state.pending_abstraction.len() > MAX_PENDING_ABSTRACTION {
                let overflow = state.pending_abstraction.len() - MAX_PENDING_ABSTRACTION;
                state.pending_abstraction.drain(0..overflow);
            }
        }

        state.unit_ids.insert(unit.key.clone(), id);
        state.unit_content_hash.insert(unit.key.clone(), new_hash);
        for item in &unit.derived_from_items {
            let entry = state.item_units.entry(item.clone()).or_default();
            if !entry.iter().any(|k| k == &unit.key) {
                entry.push(unit.key.clone());
            }
        }
    }

    // ---- Typed relations (after all endpoints exist) ----
    for unit in units {
        let from = unit_memory_id(source_id, &unit.key);
        for rel in &unit.relations {
            let to = state
                .unit_ids
                .get(&rel.target_key)
                .cloned()
                .unwrap_or_else(|| unit_memory_id(source_id, &rel.target_key));
            if graph.get_memory(&to).is_none() {
                continue;
            }
            graph.add_typed_edge(
                &from,
                &to,
                rel.kind,
                rel.weight,
                EdgeSource::System,
                Some(EvidenceRef::observation(format!(
                    "derived from {} structure",
                    state.kind.label()
                ))),
            );
            report.edges_added += 1;
        }
    }
}

/// Upgrade up to `budget` queued concepts from structural summaries into
/// architectural prose using the sidecar. Graph knowledge only; fail-quiet
/// (unprocessed keys stay queued — resumable).
async fn drain_abstraction_queue(
    graph: &mut MemoryGraph,
    state: &mut KnowledgeSourceState,
    budget: usize,
) -> usize {
    let mut done = 0usize;
    let mut requeue: Vec<String> = Vec::new();

    while done < budget {
        let Some(key) = state.pending_abstraction.first().cloned() else {
            break;
        };
        state.pending_abstraction.remove(0);
        let Some(id) = state.unit_ids.get(&key).cloned() else {
            continue;
        };

        // Snapshot content + neighbor labels before the await.
        let Some((content, neighbors)) = graph.get_memory(&id).map(|m| {
            let neighbors: Vec<String> = graph
                .ranked_relations(&id)
                .into_iter()
                .take(6)
                .filter_map(|(kind, other_id, _, _)| {
                    graph.get_memory(&other_id).map(|other| {
                        format!(
                            "{} {}",
                            kind.label(),
                            other.content.chars().take(90).collect::<String>()
                        )
                    })
                })
                .collect();
            (m.content.clone(), neighbors)
        }) else {
            continue;
        };
        if !graph.get_memory(&id).map(|m| m.active).unwrap_or(false) {
            continue;
        }

        match abstract_concept_with_sidecar(&content, &neighbors).await {
            Some(prose) => {
                let upgraded = if let Some(m) = graph.get_memory_mut(&id) {
                    // Keep the deterministic structure line so symbol names
                    // stay searchable underneath the architectural prose.
                    m.content = format!("{prose}\n{content}");
                    m.embedding = None;
                    m.concept_embedding = None;
                    m.record_evidence(EvidenceRef::observation(
                        "sidecar architectural abstraction",
                    ));
                    m.refresh_search_text();
                    m.updated_at = Utc::now();
                    true
                } else {
                    false
                };
                if upgraded {
                    graph.tag_memory(&id, TAG_ABSTRACTED);
                    done += 1;
                    crate::memory_log::log_knowledge(
                        "knowledge_abstracted",
                        serde_json::json!({ "unit": key, "memory": id }),
                    );
                }
            }
            None => {
                // Sidecar unavailable / declined: keep for a later pass.
                requeue.push(key);
                break;
            }
        }
    }

    state.pending_abstraction.splice(0..0, requeue);
    done
}

/// Ask the sidecar to describe a concept's architectural responsibility.
/// Returns `None` when the sidecar is off, fails, or answers unusably.
async fn abstract_concept_with_sidecar(
    structural: &str,
    neighbors: &[String],
) -> Option<String> {
    let mut prompt = format!(
        "Structural summary of a code concept:\n{}\n",
        structural.trim()
    );
    if !neighbors.is_empty() {
        prompt.push_str("\nRelated concepts:\n");
        for n in neighbors {
            prompt.push_str(&format!("- {n}\n"));
        }
    }
    prompt.push_str(
        "\nIn 1-2 sentences, state this concept's architectural responsibility: \
         what it is for, why it exists, and how it fits with the related \
         concepts. Do not restate the symbol list. Reply with only the sentences.",
    );
    let sidecar = crate::sidecar::Sidecar::new();
    let text = sidecar
        .complete(
            "You are a software architecture analyst. You describe the \
             responsibility and intent of code concepts concisely and \
             factually, without inventing details.",
            &prompt,
        )
        .await
        .ok()?;
    let text = text.trim().trim_matches('"').trim().to_string();
    if text.is_empty() || text.len() > 700 {
        None
    } else {
        Some(text)
    }
}

/// Reconstruct a source from persisted state. Extend here as new source kinds
/// gain implementations; unknown kinds are skipped (forward compatibility).
fn source_from_state(state: &KnowledgeSourceState) -> Option<Box<dyn KnowledgeSource>> {
    match state.kind {
        KnowledgeSourceKind::Repository => Some(Box::new(repo::RepositorySource::new(
            std::path::PathBuf::from(&state.locator),
        ))),
        _ => None,
    }
}

/// Incrementally refresh every registered source in `graph` and apply queued
/// tool-outcome evidence. This is the maintenance hook the sleep cycle calls
/// so knowledge stays live without a separate scheduler. Fail-quiet per
/// source; returns the per-source reports.
pub async fn refresh_sources_in_graph(
    graph: &mut MemoryGraph,
    opts: IngestOptions,
) -> Vec<(String, IngestReport)> {
    let source_ids: Vec<String> = graph.metadata.knowledge_sources.keys().cloned().collect();
    let mut reports = Vec::new();
    for source_id in source_ids {
        let Some(state) = graph.metadata.knowledge_sources.get(&source_id) else {
            continue;
        };
        let Some(mut source) = source_from_state(state) else {
            continue;
        };
        match ingest_source_into_graph(graph, source.as_mut(), opts).await {
            Ok(report) => reports.push((source_id, report)),
            Err(e) => {
                crate::logging::warn(&format!(
                    "knowledge refresh failed for {source_id}: {e}"
                ));
            }
        }
    }
    // Engineering intent stays synchronized with the graph: mirror the
    // project's persistent goals (src/goal.rs) as concepts each maintenance
    // pass, so plans, briefs, and reasoning always see current goal state.
    let working_dir = graph
        .metadata
        .knowledge_sources
        .values()
        .find(|s| matches!(s.kind, KnowledgeSourceKind::Repository))
        .map(|s| std::path::PathBuf::from(&s.locator));
    let _ = engineering::sync_goals_into_graph(graph, working_dir.as_deref());

    let applied = evidence::apply_queued_outcomes(graph);
    if applied > 0 {
        crate::memory_log::log_knowledge(
            "knowledge_tool_evidence",
            serde_json::json!({ "applied": applied }),
        );
    }
    reports
}

/// Human-readable status of all registered sources (CLI `knowledge status`).
pub fn render_sources_status(graph: &MemoryGraph) -> String {
    if graph.metadata.knowledge_sources.is_empty() {
        return "No knowledge sources registered. Run `neura knowledge ingest [path]`.".to_string();
    }
    let mut out = String::new();
    for (id, state) in &graph.metadata.knowledge_sources {
        let active = state
            .unit_ids
            .values()
            .filter(|mid| graph.get_memory(mid).map(|m| m.active).unwrap_or(false))
            .count();
        out.push_str(&format!(
            "{} ({})\n  locator: {}\n  concepts: {} ({} active) · items tracked: {} · pending abstraction: {}\n",
            id,
            state.kind.label(),
            state.locator,
            state.unit_ids.len(),
            active,
            state.fingerprints.len(),
            state.pending_abstraction.len(),
        ));
        if let Some(at) = state.last_ingest {
            out.push_str(&format!("  last ingest: {}\n", at.format("%Y-%m-%d %H:%M:%S UTC")));
        }
        if let Some(report) = &state.last_report {
            out.push_str(&format!("  last pass: {}\n", report.render("ingest")));
        }
    }
    out
}

/// Look up the memory ids of concepts derived from a repo-relative item path
/// across all registered sources. Used by tool-evidence feedback.
pub fn concept_ids_for_item(graph: &MemoryGraph, item: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for state in graph.metadata.knowledge_sources.values() {
        if let Some(keys) = state.item_units.get(item) {
            for key in keys {
                if let Some(id) = state.unit_ids.get(key) {
                    ids.push(id.clone());
                }
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests;
