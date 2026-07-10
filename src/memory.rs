//! Memory system for cross-session learning
//!
//! Provides persistent memory that survives across sessions, organized by:
//! - Project (per working directory)
//! - Global (user-level preferences)
//!
//! Integrates with the Haiku sidecar for relevance verification and extraction.

use crate::memory_graph::{GRAPH_VERSION, MemoryGraph};
use crate::memory_types::{
    InjectedMemoryItem, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryState, StepResult,
    StepStatus,
};
use crate::sidecar::Sidecar;
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[path = "memory/activity.rs"]
mod activity;
mod cache;
pub(crate) mod model;
#[path = "memory/pending.rs"]
mod pending;
#[path = "memory_prompt.rs"]
mod prompt_support;
mod ranking;
pub(crate) mod search;

pub use activity::{
    activity_snapshot, add_event, apply_remote_activity_snapshot, check_staleness, clear_activity,
    get_activity, pipeline_start, pipeline_update, record_injected_prompt, set_state,
};
use cache::{cache_graph, cache_search, cached_graph, cached_search, clear_search_cache};
pub use model::{MemoryCategory, MemoryEntry, Reinforcement, TrustLevel};
#[cfg(test)]
use pending::insert_pending_memory_for_test;
pub use pending::{
    PendingMemory, clear_all_injected_memories, clear_all_pending_memory, clear_injected_memories,
    clear_pending_memory, has_any_pending_memory, has_pending_memory, is_memory_injected,
    is_memory_injected_any, mark_memories_injected, set_pending_memory,
    set_pending_memory_with_ids, set_pending_memory_with_ids_and_display, sync_injected_memories,
    take_pending_memory,
};
use pending::{begin_memory_check, finish_memory_check};
use prompt_support::format_entries_for_prompt;
pub(crate) use prompt_support::{
    format_context_for_extraction, format_context_for_relevance, format_relevant_display_prompt,
    format_relevant_prompt,
};
use ranking::{top_k_by_ord, top_k_by_score};
use search::{
    collect_skill_query_terms, memory_matches_search, normalize_memory_search_text,
    normalize_search_text, skill_retrieval_bonus,
};

const LEGACY_NOTE_CATEGORY: &str = "note";

// === Phase 3 — `.mem_get` retrieval contract ===
//
// Anchor mode (`NEURA_MEMORY_ANCHOR=1`) replaces the full memory injection
// with a tiny `<mem-anchor count="N" via=".mem_get" />` pointer. To keep the
// model's recall capability intact, the full prompt that *would* have been
// injected is stashed here, keyed by session id, so when the model emits
// `.mem_get reason=<why>` we can fulfil the request on the next turn.

static ANCHORED_MEMORY_PROMPTS: std::sync::Mutex<
    Option<std::collections::HashMap<String, AnchoredMemorySnapshot>>,
> = std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
struct AnchoredMemorySnapshot {
    prompt: String,
    memory_ids: Vec<String>,
    stashed_at: std::time::Instant,
}

const ANCHORED_MEMORY_FRESH_SECS: u64 = 600;

/// Remember the full memory prompt for this session so it can be returned by
/// `.mem_get`. Called from the turn loop right after deciding to inject or
/// anchor; both paths stash so a follow-up `.mem_get` always has fresh data.
pub fn stash_memory_for_anchor_rehydration(session_id: &str, prompt: &str, memory_ids: &[String]) {
    if prompt.trim().is_empty() {
        return;
    }
    if let Ok(mut guard) = ANCHORED_MEMORY_PROMPTS.lock() {
        let map = guard.get_or_insert_with(std::collections::HashMap::new);
        map.insert(
            session_id.to_string(),
            AnchoredMemorySnapshot {
                prompt: prompt.to_string(),
                memory_ids: memory_ids.to_vec(),
                stashed_at: std::time::Instant::now(),
            },
        );
    }
}

/// Discard any stashed anchor-mode memory for this session.
pub fn clear_anchored_memory(session_id: &str) {
    if let Ok(mut guard) = ANCHORED_MEMORY_PROMPTS.lock() {
        if let Some(map) = guard.as_mut() {
            map.remove(session_id);
        }
    }
}

/// Test/inspection helper: returns true if a fresh anchor snapshot exists.
pub fn has_anchored_memory(session_id: &str) -> bool {
    if let Ok(guard) = ANCHORED_MEMORY_PROMPTS.lock() {
        if let Some(map) = guard.as_ref() {
            if let Some(snapshot) = map.get(session_id) {
                return snapshot.stashed_at.elapsed().as_secs() < ANCHORED_MEMORY_FRESH_SECS;
            }
        }
    }
    false
}

/// Parse a `.mem_get` request from model text. Mirrors `interlang::parse_exact_request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemGetRequest {
    pub reason: Option<String>,
}

pub fn parse_mem_get_request(text: &str) -> Option<MemGetRequest> {
    let line = text
        .lines()
        .find(|line| line.trim().starts_with(".mem_get"))?;
    let trimmed = line.trim();
    let mut reason = None;
    for part in trimmed.split_whitespace().skip(1) {
        if let Some(value) = part.strip_prefix("reason=") {
            reason = Some(value.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    Some(MemGetRequest { reason })
}

/// Build the `<system-reminder>` rehydration block for a `.mem_get` request.
/// Returns None when no fresh anchored memory snapshot exists for the session.
pub fn maybe_rehydrate_mem_get(session_id: &str, model_text: &str) -> Option<String> {
    let req = parse_mem_get_request(model_text)?;
    let snapshot = {
        let guard = ANCHORED_MEMORY_PROMPTS.lock().ok()?;
        let map = guard.as_ref()?;
        let snap = map.get(session_id)?;
        if snap.stashed_at.elapsed().as_secs() >= ANCHORED_MEMORY_FRESH_SECS {
            return None;
        }
        snap.clone()
    };
    Some(format!(
        "<system-reminder>\nNeura .mem_get rehydration fulfilled (reason={}). The relevant memory entries follow. Treat them as authoritative for this turn.\n\n{}\n\n(memory_ids: {})\n</system-reminder>",
        req.reason.as_deref().unwrap_or("unspecified"),
        snapshot.prompt,
        snapshot.memory_ids.join(",")
    ))
}

#[cfg(test)]
mod mem_get_tests {
    use super::*;

    #[test]
    fn parses_mem_get_with_reason() {
        let req = parse_mem_get_request(".mem_get reason=preferences").expect("should parse");
        assert_eq!(req.reason.as_deref(), Some("preferences"));
    }

    #[test]
    fn parses_mem_get_without_reason() {
        let req = parse_mem_get_request(".mem_get").expect("should parse bare");
        assert!(req.reason.is_none());
    }

    #[test]
    fn rehydrate_returns_stashed_prompt() {
        let session = "test-session-mem-get-1";
        clear_anchored_memory(session);
        stash_memory_for_anchor_rehydration(
            session,
            "## Notes\n1. user prefers tabs",
            &["m1".to_string()],
        );
        let response = maybe_rehydrate_mem_get(session, ".mem_get reason=indentation")
            .expect("rehydrate must produce text");
        assert!(response.contains("user prefers tabs"));
        assert!(response.contains("indentation"));
        clear_anchored_memory(session);
    }

    #[test]
    fn rehydrate_returns_none_when_nothing_stashed() {
        let session = "test-session-mem-get-2";
        clear_anchored_memory(session);
        assert!(maybe_rehydrate_mem_get(session, ".mem_get").is_none());
    }
}

pub type MemoryEventSink = Arc<dyn Fn(crate::protocol::ServerEvent) + Send + Sync>;

pub fn memory_sidecar_enabled() -> bool {
    crate::config::config().agents.memory_sidecar_enabled
}

/// Pick a human concept label for a consolidation group: the most common tag
/// among members that isn't a structural tag. Deterministic.
fn dominant_group_tag(group: &[String], graph: &crate::memory_graph::MemoryGraph) -> String {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in group {
        if let Some(m) = graph.get_memory(id) {
            for t in &m.tags {
                if t == "semantic" || t == "consolidated" {
                    continue;
                }
                *counts.entry(t.clone()).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(t, _)| t)
        .unwrap_or_else(|| "concept".to_string())
}

/// Merge several episodic memories into a single semantic statement using the
/// sidecar LLM. Falls back to the longest constituent when the sidecar is off
/// or fails — consolidation is a graph operation; the text is best-effort.
async fn summarize_group_with_sidecar(contents: &[String], sidecar_on: bool) -> Option<String> {
    let fallback = || {
        contents
            .iter()
            .max_by_key(|c| c.trim().len())
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
    };
    if !sidecar_on {
        return fallback();
    }
    let mut prompt = String::from(
        "These memories describe the same concept. Write ONE concise sentence that \
         captures the shared fact. Preserve specifics; do not invent. Reply with only the sentence:\n",
    );
    for (i, c) in contents.iter().enumerate() {
        prompt.push_str(&format!("{}. {}\n", i + 1, c.trim()));
    }
    let sidecar = Sidecar::new();
    match sidecar
        .complete(
            "You consolidate memories into a single factual sentence. \
             Reply with ONLY that sentence, no preamble.",
            &prompt,
        )
        .await
    {
        Ok(text) => {
            let text = text.trim().trim_matches('"').trim().to_string();
            if text.is_empty() || text.len() > 600 {
                fallback()
            } else {
                Some(text)
            }
        }
        Err(_) => fallback(),
    }
}

/// Ask the sidecar whether two semantic memories contradict. Returns a short
/// reason when they do, or `None` when they are consistent / the sidecar is
/// unavailable. Never mutates anything; the caller records graph knowledge.
async fn review_contradiction_with_sidecar(a: &str, b: &str) -> Option<String> {
    let prompt = format!(
        "Statement A: {}\nStatement B: {}\n\nDo these two statements directly contradict each \
         other? If YES, reply with 'YES: <one short reason>'. If they are consistent or unrelated, \
         reply with exactly 'NO'.",
        a.trim(),
        b.trim()
    );
    let sidecar = Sidecar::new();
    let resp = sidecar
        .complete(
            "You detect factual contradictions between two statements. Be strict: only report a \
             contradiction when both cannot be true at once. Reply 'YES: reason' or 'NO'.",
            &prompt,
        )
        .await
        .ok()?;
    let resp = resp.trim();
    let upper = resp.to_uppercase();
    if upper.starts_with("YES") {
        let reason = resp
            .splitn(2, ':')
            .nth(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("LLM flagged a contradiction")
            .to_string();
        Some(format!("contradiction: {reason}"))
    } else {
        None
    }
}

fn emit_memory_activity(event_tx: Option<&MemoryEventSink>) {
    let (Some(event_tx), Some(activity)) = (event_tx, activity_snapshot()) else {
        return;
    };
    (event_tx)(crate::protocol::ServerEvent::MemoryActivity { activity });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Project,
    Global,
    All,
}

impl MemoryScope {
    fn includes_project(self) -> bool {
        matches!(self, Self::Project | Self::All)
    }

    fn includes_global(self) -> bool {
        matches!(self, Self::Global | Self::All)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LegacyNotesFile {
    #[serde(default)]
    entries: Vec<LegacyNoteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyNoteEntry {
    id: String,
    content: String,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStore {
    pub entries: Vec<MemoryEntry>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: MemoryEntry) -> String {
        clear_search_cache();
        let id = entry.id.clone();
        self.entries.push(entry);
        id
    }

    pub fn by_category(&self, category: &MemoryCategory) -> Vec<&MemoryEntry> {
        self.entries
            .iter()
            .filter(|e| &e.category == category)
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let query_lower = normalize_search_text(query);
        if query_lower.is_empty() {
            return Vec::new();
        }

        self.entries
            .iter()
            .filter(|e| memory_matches_search(e, &query_lower))
            .collect()
    }

    pub fn search_explained(&self, query: &str, limit: usize) -> Vec<MemorySearchExplanation> {
        let query_lower = normalize_search_text(query);
        if query_lower.is_empty() {
            return Vec::new();
        }
        if let Some(cached) = cached_search(&PathBuf::from("memory-store"), &query_lower, limit) {
            return cached;
        }
        let mut scored: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.active)
            .map(|entry| explain_memory_match(entry, &query_lower))
            .filter(|explanation| explanation.lexical_score > 0.0)
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        cache_search(PathBuf::from("memory-store"), query_lower, limit, &scored);
        scored
    }

    pub fn search_ranked(&self, query: &str, limit: usize) -> Vec<&MemoryEntry> {
        self.search_explained(query, limit)
            .into_iter()
            .filter_map(|explanation| self.get(&explanation.id))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&MemoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        clear_search_cache();
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            Some(self.entries.remove(pos))
        } else {
            None
        }
    }

    pub fn get_relevant(&self, limit: usize) -> Vec<&MemoryEntry> {
        top_k_by_score(
            self.entries
                .iter()
                .filter(|entry| entry.active)
                .map(|entry| (entry, memory_score(entry) as f32)),
            limit,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect()
    }

    pub fn format_for_prompt(&self, limit: usize) -> Option<String> {
        let relevant: Vec<MemoryEntry> = self.get_relevant(limit).into_iter().cloned().collect();
        format_entries_for_prompt(&relevant, limit)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemorySearchExplanation {
    pub id: String,
    pub score: f64,
    pub lexical_score: f64,
    pub recency_score: f64,
    pub trust_score: f64,
    pub category_score: f64,
    pub reasons: Vec<String>,
}

fn explain_memory_match(entry: &MemoryEntry, normalized_query: &str) -> MemorySearchExplanation {
    let mut reasons = Vec::new();
    let haystack = normalize_search_text(&format!("{} {} {:?}", entry.content, "", entry.tags));
    let query_terms: Vec<&str> = normalized_query
        .split_whitespace()
        .filter(|t| t.len() > 1)
        .collect();
    let hits = query_terms
        .iter()
        .filter(|term| haystack.contains(**term))
        .count();
    let lexical_score = if query_terms.is_empty() {
        0.0
    } else {
        hits as f64 / query_terms.len() as f64
    };
    if hits > 0 {
        reasons.push(format!("{hits}/{} query terms matched", query_terms.len()));
    }

    let age_hours = (Utc::now() - entry.updated_at).num_hours().max(0) as f64;
    let recency_score = 1.0 / (1.0 + age_hours / (24.0 * 14.0));
    if recency_score > 0.7 {
        reasons.push("recently updated".into());
    }

    let trust_score = match entry.trust {
        TrustLevel::High => 1.0,
        TrustLevel::Medium => 0.72,
        TrustLevel::Low => 0.42,
    };
    if matches!(entry.trust, TrustLevel::High) {
        reasons.push("high trust".into());
    }

    let category_score = match entry.category {
        MemoryCategory::Correction => 1.15,
        MemoryCategory::Preference => 1.05,
        MemoryCategory::Entity => 1.0,
        MemoryCategory::Fact => 0.92,
        MemoryCategory::Custom(_) => 0.96,
    };
    if matches!(entry.category, MemoryCategory::Correction) {
        reasons.push("correction precedence".into());
    }

    let active_score = if entry.active { 1.0 } else { 0.0 };
    let correction_bonus = if matches!(entry.category, MemoryCategory::Correction) {
        1.25
    } else {
        0.0
    };
    let score = active_score
        * (((lexical_score * 3.0) + (recency_score * 0.45) + (trust_score * 0.8)) * category_score
            + correction_bonus);
    MemorySearchExplanation {
        id: entry.id.clone(),
        score,
        lexical_score,
        recency_score,
        trust_score,
        category_score,
        reasons,
    }
}

const MEMORY_RELEVANCE_MAX_CANDIDATES: usize = 30;
const MEMORY_RELEVANCE_MAX_RESULTS: usize = 10;

fn memory_score(entry: &MemoryEntry) -> f64 {
    // Skip inactive memories
    if !entry.active {
        return 0.0;
    }

    let mut score = 0.0;

    // Recency factor (decays over time)
    let age_hours = (Utc::now() - entry.updated_at).num_hours() as f64;
    score += 100.0 / (1.0 + age_hours / 24.0);

    // Access frequency bonus
    score += (entry.access_count as f64).sqrt() * 10.0;

    // Category importance
    score += match entry.category {
        MemoryCategory::Correction => 50.0,
        MemoryCategory::Preference => 30.0,
        MemoryCategory::Fact => 20.0,
        MemoryCategory::Entity => 10.0,
        MemoryCategory::Custom(_) => 5.0,
    };

    // Trust level multiplier
    score *= match entry.trust {
        TrustLevel::High => 1.5,
        TrustLevel::Medium => 1.0,
        TrustLevel::Low => 0.7,
    };

    // Consolidation strength bonus
    score += (entry.strength as f64).ln() * 5.0;

    score
}

#[derive(Debug, Clone)]
pub struct MemoryManager {
    project_dir: Option<PathBuf>,
    /// When true, use isolated test storage instead of real memory
    test_mode: bool,
    include_skills: bool,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            project_dir: None,
            test_mode: false,
            include_skills: true,
        }
    }

    pub fn with_project_dir(mut self, project_dir: impl Into<PathBuf>) -> Self {
        self.project_dir = Some(project_dir.into());
        self
    }

    pub fn with_skills(mut self, include_skills: bool) -> Self {
        self.include_skills = include_skills;
        self
    }

    /// Create a memory manager in test mode (isolated storage)
    pub fn new_test() -> Self {
        Self {
            project_dir: None,
            test_mode: true,
            include_skills: true,
        }
    }

    /// Check if running in test mode
    pub fn is_test_mode(&self) -> bool {
        self.test_mode
    }

    /// Set test mode (for debug sessions)
    pub fn set_test_mode(&mut self, test_mode: bool) {
        self.test_mode = test_mode;
    }

    /// Clear all test memories (only works in test mode)
    pub fn clear_test_storage(&self) -> Result<()> {
        if !self.test_mode {
            anyhow::bail!("clear_test_storage only allowed in test mode");
        }

        let test_dir = storage::neura_dir()?.join("memory").join("test");
        if test_dir.exists() {
            std::fs::remove_dir_all(&test_dir)?;
            crate::logging::info("Cleared test memory storage");
        }
        Ok(())
    }

    fn get_project_dir(&self) -> Option<PathBuf> {
        self.project_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
    }

    fn project_memory_path(&self) -> Result<Option<PathBuf>> {
        // In test mode, use test directory
        if self.test_mode {
            let test_dir = storage::neura_dir()?.join("memory").join("test");
            std::fs::create_dir_all(&test_dir)?;
            return Ok(Some(test_dir.join("test_project.json")));
        }

        let project_dir = match self.get_project_dir() {
            Some(d) => d,
            None => return Ok(None),
        };

        let project_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            project_dir.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        let memory_dir = storage::neura_dir()?.join("memory").join("projects");
        Ok(Some(memory_dir.join(format!("{}.json", project_hash))))
    }

    fn legacy_notes_path(&self) -> Result<Option<PathBuf>> {
        if self.test_mode {
            let test_dir = storage::neura_dir()?.join("notes").join("test");
            std::fs::create_dir_all(&test_dir)?;
            return Ok(Some(test_dir.join("test_notes.json")));
        }

        let project_dir = match self.get_project_dir() {
            Some(d) => d,
            None => return Ok(None),
        };

        let project_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            project_dir.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        Ok(Some(
            storage::neura_dir()?
                .join("notes")
                .join(format!("{}.json", project_hash)),
        ))
    }

    fn normalize_graph_search_text(graph: &mut MemoryGraph) -> bool {
        let mut changed = false;
        for memory in graph.memories.values_mut() {
            let expected = normalize_memory_search_text(&memory.content, &memory.tags);
            if memory.search_text != expected {
                memory.search_text = expected;
                changed = true;
            }
        }
        changed
    }

    fn import_legacy_notes_into_graph(&self, graph: &mut MemoryGraph) -> Result<bool> {
        let Some(path) = self.legacy_notes_path()? else {
            return Ok(false);
        };
        if !path.exists() {
            return Ok(false);
        }

        let legacy: LegacyNotesFile = storage::read_json(&path)?;
        if legacy.entries.is_empty() {
            return Ok(false);
        }

        let mut changed = false;
        for note in legacy.entries {
            if graph.memories.contains_key(&note.id) {
                continue;
            }

            let mut entry = MemoryEntry::new(
                MemoryCategory::Custom(LEGACY_NOTE_CATEGORY.to_string()),
                note.content,
            );
            entry.id = note.id;
            entry.created_at = note.created_at;
            entry.updated_at = note.created_at;
            entry.source = Some("legacy_remember_migration".to_string());
            if let Some(tag) = note.tag {
                entry.tags.push(tag);
            }
            entry.ensure_embedding();
            graph.add_memory(entry);
            changed = true;
        }

        Ok(changed)
    }

    fn global_memory_path(&self) -> Result<PathBuf> {
        if self.test_mode {
            let test_dir = storage::neura_dir()?.join("memory").join("test");
            std::fs::create_dir_all(&test_dir)?;
            Ok(test_dir.join("test_global.json"))
        } else {
            Ok(storage::neura_dir()?.join("memory").join("global.json"))
        }
    }

    pub fn load_project(&self) -> Result<MemoryStore> {
        match self.project_memory_path()? {
            Some(path) if path.exists() => storage::read_json(&path),
            _ => Ok(MemoryStore::new()),
        }
    }

    pub fn load_global(&self) -> Result<MemoryStore> {
        let path = self.global_memory_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(MemoryStore::new())
        }
    }

    pub fn save_project(&self, store: &MemoryStore) -> Result<()> {
        if let Some(path) = self.project_memory_path()? {
            storage::write_json(&path, store)?;
        }
        Ok(())
    }

    pub fn save_global(&self, store: &MemoryStore) -> Result<()> {
        let path = self.global_memory_path()?;
        storage::write_json(&path, store)
    }

    /// Similarity threshold for storage-layer dedup.
    /// Memories above this threshold are considered duplicates and reinforced instead.
    const STORAGE_DEDUP_THRESHOLD: f32 = 0.85;

    pub fn remember_project(&self, entry: MemoryEntry) -> Result<String> {
        let mut entry = entry;
        crate::neura_memory::apply_sensitive_tags(&mut entry.tags, &entry.content);
        entry.refresh_search_text();
        if self.should_generate_embedding_for_entry(&entry) {
            entry.ensure_embedding();
        }

        let mut graph = self.load_project_graph()?;

        if let Some(ref emb) = entry.embedding {
            if let Some(existing_id) =
                Self::find_duplicate_in_graph(&graph, emb, Self::STORAGE_DEDUP_THRESHOLD)
                && let Some(existing) = graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("dedup"), 0);
                self.save_project_graph(&graph)?;
                return Ok(existing_id);
            }

            // Cross-store dedup: also check global graph
            if let Ok(mut global_graph) = self.load_global_graph()
                && let Some(existing_id) =
                    Self::find_duplicate_in_graph(&global_graph, emb, Self::STORAGE_DEDUP_THRESHOLD)
                && let Some(existing) = global_graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("cross-dedup"), 0);
                self.save_global_graph(&global_graph)?;
                return Ok(existing_id);
            }
        }

        let id = graph.add_memory(entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    pub fn remember_global(&self, entry: MemoryEntry) -> Result<String> {
        let mut entry = entry;
        crate::neura_memory::apply_sensitive_tags(&mut entry.tags, &entry.content);
        entry.refresh_search_text();
        if self.should_generate_embedding_for_entry(&entry) {
            entry.ensure_embedding();
        }

        let mut graph = self.load_global_graph()?;

        if let Some(ref emb) = entry.embedding {
            if let Some(existing_id) =
                Self::find_duplicate_in_graph(&graph, emb, Self::STORAGE_DEDUP_THRESHOLD)
                && let Some(existing) = graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("dedup"), 0);
                self.save_global_graph(&graph)?;
                return Ok(existing_id);
            }

            // Cross-store dedup: also check project graph
            if let Ok(mut project_graph) = self.load_project_graph()
                && let Some(existing_id) = Self::find_duplicate_in_graph(
                    &project_graph,
                    emb,
                    Self::STORAGE_DEDUP_THRESHOLD,
                )
                && let Some(existing) = project_graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("cross-dedup"), 0);
                self.save_project_graph(&project_graph)?;
                return Ok(existing_id);
            }
        }

        let id = graph.add_memory(entry);
        self.save_global_graph(&graph)?;
        Ok(id)
    }

    /// Insert or update a memory with a stable ID in the project graph.
    /// Preserves existing inbound/outbound graph relationships while refreshing
    /// content and tags.
    pub fn upsert_project_memory(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_project_graph()?;
        let id = self.upsert_memory_in_graph(&mut graph, entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    /// Insert or update a memory with a stable ID in the global graph.
    /// Preserves existing inbound/outbound graph relationships while refreshing
    /// content and tags.
    pub fn upsert_global_memory(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_global_graph()?;
        let id = self.upsert_memory_in_graph(&mut graph, entry);
        self.save_global_graph(&graph)?;
        Ok(id)
    }

    fn upsert_memory_in_graph(
        &self,
        graph: &mut crate::memory_graph::MemoryGraph,
        mut entry: MemoryEntry,
    ) -> String {
        crate::neura_memory::apply_sensitive_tags(&mut entry.tags, &entry.content);
        entry.refresh_search_text();
        let id = entry.id.clone();
        let should_generate_embedding = self.should_generate_embedding_for_entry(&entry);
        if should_generate_embedding {
            entry.ensure_embedding();
        }

        let Some(existing_snapshot) = graph.get_memory(&id).cloned() else {
            return graph.add_memory(entry);
        };

        let old_tags: std::collections::HashSet<String> =
            existing_snapshot.tags.iter().cloned().collect();
        let new_tags: std::collections::HashSet<String> = entry.tags.iter().cloned().collect();

        for tag in old_tags.difference(&new_tags) {
            graph.untag_memory(&id, tag);
        }
        for tag in new_tags.difference(&old_tags) {
            graph.tag_memory(&id, tag);
        }

        if let Some(existing) = graph.get_memory_mut(&id) {
            let content_changed = existing.content != entry.content;
            existing.category = entry.category;
            existing.content = entry.content;
            existing.tags = entry.tags;
            existing.updated_at = entry.updated_at;
            existing.source = entry.source;
            existing.trust = entry.trust;
            existing.active = entry.active;
            existing.superseded_by = entry.superseded_by;
            existing.confidence = entry.confidence;
            if content_changed && should_generate_embedding {
                existing.embedding = None;
                existing.ensure_embedding();
            } else if content_changed {
                existing.embedding = None;
            }
        }

        id
    }

    fn should_generate_embedding_for_entry(&self, entry: &MemoryEntry) -> bool {
        if self.test_mode {
            return false;
        }

        #[cfg(test)]
        if std::env::var_os("NEURA_TEST_ALLOW_MEMORY_EMBEDDINGS").is_none() {
            return false;
        }

        !matches!(&entry.category, MemoryCategory::Custom(category) if category == "goal")
    }

    fn find_duplicate_in_graph(
        graph: &crate::memory_graph::MemoryGraph,
        query_emb: &[f32],
        threshold: f32,
    ) -> Option<String> {
        let mut best: Option<(String, f32)> = None;
        for entry in graph.active_memories() {
            if let Some(ref emb) = entry.embedding {
                let sim = crate::embedding::cosine_similarity(query_emb, emb);
                if sim >= threshold && best.as_ref().map(|(_, s)| sim > *s).unwrap_or(true) {
                    best = Some((entry.id.clone(), sim));
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Find memories similar to the given text using embedding search
    /// Returns memories with similarity above threshold, sorted by similarity
    pub fn find_similar(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        // Generate embedding for query text
        let query_embedding = match crate::embedding::embed(text) {
            Ok(emb) => emb,
            Err(e) => {
                crate::logging::info(&format!(
                    "Embedding failed, falling back to keyword search: {}",
                    e
                ));
                return Ok(Vec::new());
            }
        };

        self.find_similar_with_embedding(&query_embedding, threshold, limit)
    }

    pub fn find_similar_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let query_embedding = match crate::embedding::embed(text) {
            Ok(emb) => emb,
            Err(e) => {
                crate::logging::info(&format!(
                    "Embedding failed, falling back to keyword search: {}",
                    e
                ));
                return Ok(Vec::new());
            }
        };

        self.find_similar_with_embedding_scoped(&query_embedding, threshold, limit, scope)
    }

    /// Find memories similar to the given embedding
    pub fn find_similar_with_embedding(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let entries_with_emb = self.collect_all_memories_with_embeddings()?;
        Self::score_and_filter(entries_with_emb, query_embedding, "", threshold, limit)
    }

    pub fn find_similar_with_embedding_scoped(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let entries_with_emb = self.collect_memories_with_embeddings_scoped(scope)?;
        Self::score_and_filter(entries_with_emb, query_embedding, "", threshold, limit)
    }

    fn collect_all_memories_with_embeddings(&self) -> Result<Vec<MemoryEntry>> {
        self.collect_memories_with_embeddings_scoped(MemoryScope::All)
    }

    fn collect_memories_with_embeddings_scoped(
        &self,
        scope: MemoryScope,
    ) -> Result<Vec<MemoryEntry>> {
        let mut entries: Vec<MemoryEntry> = Vec::new();
        if scope.includes_project()
            && let Ok(project) = self.load_project_graph()
        {
            entries.extend(
                project
                    .active_memories()
                    .filter(|m| m.embedding.is_some())
                    .cloned(),
            );
        }
        if scope.includes_global()
            && let Ok(global) = self.load_global_graph()
        {
            entries.extend(
                global
                    .active_memories()
                    .filter(|m| m.embedding.is_some())
                    .cloned(),
            );
        }
        Ok(entries)
    }

    fn collect_memories_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut entries = Vec::new();
        if scope.includes_project()
            && let Ok(project) = self.load_project_graph()
        {
            entries.extend(project.all_memories().cloned());
        }
        if scope.includes_global()
            && let Ok(global) = self.load_global_graph()
        {
            entries.extend(global.all_memories().cloned());
        }
        Ok(entries)
    }

    fn synthetic_skill_entries(&self) -> Vec<MemoryEntry> {
        if !self.include_skills {
            return Vec::new();
        }

        crate::skill::SkillRegistry::shared_snapshot()
            .list()
            .into_iter()
            .map(|skill| skill.as_memory_entry())
            .collect()
    }

    fn collect_retrieval_candidates_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut entries = self.collect_memories_scoped(scope)?;
        if scope.includes_global() {
            entries.extend(self.synthetic_skill_entries());
        }
        Ok(entries)
    }

    fn collect_retrieval_candidates_with_embeddings_scoped(
        &self,
        scope: MemoryScope,
    ) -> Result<Vec<MemoryEntry>> {
        let mut entries = self.collect_memories_with_embeddings_scoped(scope)?;
        if scope.includes_global() {
            entries.extend(
                self.synthetic_skill_entries()
                    .into_iter()
                    .filter_map(|mut entry| entry.ensure_embedding().then_some(entry)),
            );
        }
        Ok(entries)
    }

    fn find_retrieval_candidates_similar_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let query_embedding = match crate::embedding::embed(text) {
            Ok(emb) => emb,
            Err(e) => {
                crate::logging::info(&format!(
                    "Embedding failed for retrieval candidates, falling back to keyword search: {}",
                    e
                ));
                return Ok(Vec::new());
            }
        };

        let entries = self.collect_retrieval_candidates_with_embeddings_scoped(scope)?;
        Self::score_and_filter(entries, &query_embedding, text, threshold, limit)
    }

    fn score_and_filter(
        entries: Vec<MemoryEntry>,
        query_embedding: &[f32],
        query_text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let mut filtered_entries = Vec::with_capacity(entries.len());
        let mut skipped_missing_embeddings = 0usize;
        for entry in entries {
            if entry.embedding.is_some() {
                filtered_entries.push(entry);
            } else {
                skipped_missing_embeddings += 1;
            }
        }
        if skipped_missing_embeddings > 0 {
            crate::logging::warn(&format!(
                "Skipped {} retrieval candidate(s) without embeddings during similarity scoring",
                skipped_missing_embeddings
            ));
        }
        if filtered_entries.is_empty() {
            return Ok(Vec::new());
        }
        let emb_refs: Vec<&[f32]> = filtered_entries
            .iter()
            .filter_map(|entry| entry.embedding.as_deref())
            .collect();
        let scores = crate::embedding::batch_cosine_similarity(query_embedding, &emb_refs);
        let skill_query_terms = collect_skill_query_terms(query_text);
        let explicit_sensitive_recall = crate::neura_memory::explicit_sensitive_recall(query_text);
        let explicit_contradiction_recall =
            crate::neura_memory::explicit_contradiction_recall(query_text);

        let scored = top_k_by_score(
            filtered_entries
                .into_iter()
                .zip(scores)
                .map(|(entry, sim)| {
                    let adjusted = sim
                        + skill_retrieval_bonus(&entry, &skill_query_terms)
                        + crate::neura_memory::keyword_overlap_bonus(&entry, query_text)
                        + crate::neura_memory::truth_stability_bonus(&entry);
                    (entry, adjusted)
                })
                .filter(|(entry, _)| {
                    explicit_sensitive_recall || !crate::neura_memory::memory_is_sensitive(entry)
                })
                .filter(|(entry, _)| {
                    explicit_contradiction_recall
                        || !crate::neura_memory::memory_is_contradictory(entry)
                })
                .filter(|(entry, sim)| {
                    crate::neura_memory::passes_relevance_gate(entry, query_text, *sim)
                })
                .filter(|(_, sim)| *sim >= threshold),
            limit,
        );

        let scored = Self::apply_gap_filter(scored);

        Ok(scored)
    }

    /// Drop trailing low-relevance results by detecting natural gaps in the
    /// score distribution. If the top hit is 0.85 and the next cluster is
    /// 0.40-0.42, the 0.15+ gap tells us those lower results are noise.
    ///
    /// Algorithm: walk the sorted scores and cut when the drop from one score
    /// to the next exceeds `GAP_FACTOR` of the range (top - floor_threshold).
    fn apply_gap_filter(scored: Vec<(MemoryEntry, f32)>) -> Vec<(MemoryEntry, f32)> {
        if scored.len() <= 1 {
            return scored;
        }

        const GAP_FACTOR: f32 = 0.25;
        const MIN_KEEP: usize = 1;

        let top_score = scored[0].1;
        let range = (top_score - EMBEDDING_SIMILARITY_THRESHOLD).max(0.01);
        let max_gap = range * GAP_FACTOR;

        let mut keep = scored.len();
        for i in 1..scored.len() {
            let drop = scored[i - 1].1 - scored[i].1;
            if drop > max_gap && i >= MIN_KEEP {
                keep = i;
                break;
            }
        }

        scored.into_iter().take(keep).collect()
    }

    /// Ensure all memories have embeddings (backfill for existing memories)
    pub fn backfill_embeddings(&self) -> Result<(usize, usize)> {
        let mut generated = 0;
        let mut failed = 0;

        // Process project memories
        if let Ok(mut graph) = self.load_project_graph() {
            let mut changed = false;
            for entry in graph.memories.values_mut() {
                if entry.embedding.is_none() {
                    if entry.ensure_embedding() {
                        generated += 1;
                        changed = true;
                    } else {
                        failed += 1;
                    }
                }
            }
            if changed {
                self.save_project_graph(&graph)?;
            }
        }

        // Process global memories
        if let Ok(mut graph) = self.load_global_graph() {
            let mut changed = false;
            for entry in graph.memories.values_mut() {
                if entry.embedding.is_none() {
                    if entry.ensure_embedding() {
                        generated += 1;
                        changed = true;
                    } else {
                        failed += 1;
                    }
                }
            }
            if changed {
                self.save_global_graph(&graph)?;
            }
        }

        Ok((generated, failed))
    }

    fn touch_entries(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let id_set: std::collections::HashSet<&str> = ids.iter().map(|id| id.as_str()).collect();

        let mut project = self.load_project_graph()?;
        let mut project_changed = false;
        for entry in project.memories.values_mut() {
            if id_set.contains(entry.id.as_str()) {
                entry.touch();
                project_changed = true;
            }
        }
        if project_changed {
            self.save_project_graph(&project)?;
        }

        let mut global = self.load_global_graph()?;
        let mut global_changed = false;
        for entry in global.memories.values_mut() {
            if id_set.contains(entry.id.as_str()) {
                entry.touch();
                global_changed = true;
            }
        }
        if global_changed {
            self.save_global_graph(&global)?;
        }

        Ok(())
    }

    pub fn get_prompt_memories(&self, limit: usize) -> Option<String> {
        self.get_prompt_memories_scoped(limit, MemoryScope::All)
    }

    pub fn get_prompt_memories_scoped(&self, limit: usize, scope: MemoryScope) -> Option<String> {
        let all_entries: Vec<_> = top_k_by_ord(
            self.collect_memories_scoped(scope)
                .ok()?
                .into_iter()
                .filter(|entry| !crate::neura_memory::memory_is_sensitive(entry))
                .map(|entry| {
                    let updated_at = entry.updated_at.timestamp_millis();
                    (entry, updated_at)
                }),
            limit,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

        if all_entries.is_empty() {
            return None;
        }

        format_entries_for_prompt(&all_entries, limit)
    }

    pub async fn relevant_prompt_for_messages(
        &self,
        messages: &[crate::message::Message],
    ) -> Result<Option<String>> {
        let context = format_context_for_relevance(messages);
        if context.is_empty() {
            return Ok(None);
        }
        self.relevant_prompt_for_context(
            &context,
            MEMORY_RELEVANCE_MAX_CANDIDATES,
            MEMORY_RELEVANCE_MAX_RESULTS,
        )
        .await
    }

    pub async fn relevant_prompt_for_context(
        &self,
        context: &str,
        max_candidates: usize,
        limit: usize,
    ) -> Result<Option<String>> {
        let relevant = self
            .get_relevant_for_context(context, max_candidates)
            .await?;
        if relevant.is_empty() {
            return Ok(None);
        }
        Ok(format_relevant_prompt(&relevant, limit))
    }

    pub fn search(&self, query: &str) -> Result<Vec<MemoryEntry>> {
        self.search_scoped(query, MemoryScope::All)
    }

    pub fn search_scoped(&self, query: &str, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let query_lower = normalize_search_text(query);
        if query_lower.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for memory in self.collect_memories_scoped(scope)? {
            if memory_matches_search(&memory, &query_lower) {
                results.push(memory);
            }
        }

        Ok(results)
    }

    pub fn list_all(&self) -> Result<Vec<MemoryEntry>> {
        self.list_all_scoped(MemoryScope::All)
    }

    pub fn list_all_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut all = self.collect_memories_scoped(scope)?;
        all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(all)
    }

    pub fn forget(&self, id: &str) -> Result<bool> {
        // Try graph-based removal first (new format)
        let mut project_graph = self.load_project_graph()?;
        if project_graph.remove_memory(id).is_some() {
            self.save_project_graph(&project_graph)?;
            return Ok(true);
        }

        let mut global_graph = self.load_global_graph()?;
        if global_graph.remove_memory(id).is_some() {
            self.save_global_graph(&global_graph)?;
            return Ok(true);
        }

        Ok(false)
    }

    // === Sidecar Integration ===

    /// Extract memories from a session transcript using the Haiku sidecar
    pub async fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        if !memory_sidecar_enabled() {
            crate::logging::info("Memory transcript extraction skipped: memory sidecar disabled");
            return Ok(Vec::new());
        }

        let sidecar = Sidecar::new();
        let extracted = sidecar.extract_memories(transcript).await?;

        let mut ids = Vec::new();
        for memory in extracted {
            let category: MemoryCategory = memory.category.parse().unwrap_or(MemoryCategory::Fact);
            let trust = match memory.trust.as_str() {
                "high" => TrustLevel::High,
                "medium" => TrustLevel::Medium,
                _ => TrustLevel::Low,
            };

            let entry = MemoryEntry::new(category, memory.content)
                .with_source(session_id)
                .with_trust(trust);

            // Store in project scope by default
            let id = self.remember_project(entry)?;
            ids.push(id);
        }

        Ok(ids)
    }

    /// Check if stored memories are relevant to the current context
    /// Returns memories that the sidecar deems relevant
    pub async fn get_relevant_for_context(
        &self,
        context: &str,
        max_candidates: usize,
    ) -> Result<Vec<MemoryEntry>> {
        // Get top candidate memories by score
        let candidates: Vec<_> = top_k_by_score(
            self.collect_retrieval_candidates_scoped(MemoryScope::All)?
                .into_iter()
                .filter(|entry| entry.active)
                .map(|entry| {
                    let score = memory_score(&entry) as f32;
                    (entry, score)
                }),
            max_candidates,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Update activity state - checking memories
        set_state(MemoryState::SidecarChecking {
            count: candidates.len(),
        });
        add_event(MemoryEventKind::SidecarStarted);

        let sidecar = Sidecar::new();
        let mut relevant = Vec::new();
        let mut relevant_ids = Vec::new();

        for memory in candidates {
            let start = Instant::now();
            match sidecar.check_relevance(&memory.content, context).await {
                Ok((is_relevant, _reason)) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    add_event(MemoryEventKind::SidecarComplete { latency_ms });

                    if is_relevant {
                        let preview = if memory.content.len() > 30 {
                            format!("{}...", crate::util::truncate_str(&memory.content, 30))
                        } else {
                            memory.content.clone()
                        };
                        add_event(MemoryEventKind::SidecarRelevant {
                            memory_preview: preview,
                        });
                        relevant_ids.push(memory.id.clone());
                        relevant.push(memory);
                    } else {
                        add_event(MemoryEventKind::SidecarNotRelevant);
                    }
                }
                Err(e) => {
                    add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                    crate::logging::error(&format!("Sidecar relevance check failed: {}", e));
                }
            }
        }

        let _ = self.touch_entries(&relevant_ids);

        // Update final state
        if relevant.is_empty() {
            set_state(MemoryState::Idle);
        } else {
            set_state(MemoryState::FoundRelevant {
                count: relevant.len(),
            });
        }

        Ok(relevant)
    }

    /// Simple relevance check without sidecar (keyword-based)
    /// Use this for quick checks when sidecar is not needed
    pub fn get_relevant_keywords(
        &self,
        keywords: &[&str],
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let normalized_keywords: Vec<String> = keywords
            .iter()
            .map(|keyword| normalize_search_text(keyword))
            .filter(|keyword| !keyword.is_empty())
            .collect();
        if normalized_keywords.is_empty() {
            return Ok(Vec::new());
        }

        let matches: Vec<_> = top_k_by_ord(
            self.collect_memories_scoped(MemoryScope::All)?
                .into_iter()
                .filter(|entry| {
                    let content_lower = normalize_search_text(&entry.content);
                    normalized_keywords
                        .iter()
                        .any(|kw| content_lower.contains(kw))
                })
                .map(|entry| {
                    let updated_at = entry.updated_at.timestamp_millis();
                    (entry, updated_at)
                }),
            limit,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

        Ok(matches)
    }

    // === Async Memory Checking ===

    /// Spawn a background task to check memory relevance for a specific session.
    /// Results are stored in PENDING_MEMORY keyed by session_id and can be retrieved
    /// with take_pending_memory(session_id).
    /// This method returns immediately and never blocks the caller.
    /// Only ONE memory check runs at a time per session - additional calls are ignored.
    pub fn spawn_relevance_check(
        &self,
        session_id: &str,
        messages: std::sync::Arc<[crate::message::Message]>,
        event_tx: Option<MemoryEventSink>,
    ) {
        let sid = session_id.to_string();

        if !begin_memory_check(&sid) {
            return;
        }

        let manager = self.clone();

        tokio::spawn(async move {
            let manager = if manager.project_dir.is_none() {
                MemoryManager {
                    project_dir: std::env::current_dir().ok(),
                    ..manager
                }
            } else {
                manager
            };

            match manager
                .get_relevant_parallel(&sid, &messages, event_tx.clone())
                .await
            {
                Ok((Some(prompt), memory_ids, display_prompt)) => {
                    let count = prompt
                        .lines()
                        .map(str::trim_start)
                        .filter(|line| {
                            line.starts_with("- ")
                                || line
                                    .split_once(". ")
                                    .map(|(prefix, _)| {
                                        !prefix.is_empty()
                                            && prefix.chars().all(|c| c.is_ascii_digit())
                                    })
                                    .unwrap_or(false)
                        })
                        .count()
                        .max(1);
                    set_pending_memory_with_ids_and_display(
                        &sid,
                        prompt,
                        count,
                        memory_ids,
                        display_prompt,
                    );
                    if memory_sidecar_enabled() {
                        add_event(MemoryEventKind::SidecarComplete { latency_ms: 0 });
                    }
                    emit_memory_activity(event_tx.as_ref());
                }
                Ok((None, _, _)) => {
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                }
                Err(e) => {
                    crate::logging::error(&format!("Background memory check failed: {}", e));
                    add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                }
            }

            finish_memory_check(&sid);
        });
    }

    /// Get relevant memories using embedding search + sidecar verification.
    ///
    /// 1. Embed the context (fast, local, ~30ms)
    /// 2. Find similar memories by embedding (instant)
    /// 3. Only call sidecar for embedding hits (1-5 calls instead of 30)
    ///
    /// Returns `(formatted_prompt, memory_ids, display_prompt)` on success.
    pub async fn get_relevant_parallel(
        &self,
        session_id: &str,
        messages: &[crate::message::Message],
        event_tx: Option<MemoryEventSink>,
    ) -> Result<(Option<String>, Vec<String>, Option<String>)> {
        let context = format_context_for_relevance(messages);
        if context.is_empty() {
            return Ok((None, Vec::new(), None));
        }

        // Start pipeline tracking
        pipeline_start();

        // Step 1: Embedding search (fast, local)
        set_state(MemoryState::Embedding);
        add_event(MemoryEventKind::EmbeddingStarted);
        pipeline_update(|p| p.search = StepStatus::Running);
        emit_memory_activity(event_tx.as_ref());

        let embedding_start = Instant::now();
        let candidates = match self.find_retrieval_candidates_similar_scoped(
            &context,
            EMBEDDING_SIMILARITY_THRESHOLD,
            EMBEDDING_MAX_HITS,
            MemoryScope::All,
        ) {
            Ok(hits) => {
                let latency_ms = embedding_start.elapsed().as_millis() as u64;
                if hits.is_empty() {
                    add_event(MemoryEventKind::EmbeddingComplete {
                        latency_ms,
                        hits: 0,
                    });
                    pipeline_update(|p| {
                        p.search = StepStatus::Done;
                        p.search_result = Some(StepResult {
                            summary: "0 hits".to_string(),
                            latency_ms,
                        });
                        p.verify = StepStatus::Skipped;
                        p.inject = StepStatus::Skipped;
                        p.maintain = StepStatus::Skipped;
                    });
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                    return Ok((None, Vec::new(), None));
                }
                pipeline_update(|p| {
                    p.search = StepStatus::Done;
                    p.search_result = Some(StepResult {
                        summary: format!("{} hits", hits.len()),
                        latency_ms,
                    });
                });
                add_event(MemoryEventKind::EmbeddingComplete {
                    latency_ms,
                    hits: hits.len(),
                });
                hits
            }
            Err(e) => {
                crate::logging::info(&format!("Embedding search failed, falling back: {}", e));
                add_event(MemoryEventKind::Error {
                    message: e.to_string(),
                });
                pipeline_update(|p| {
                    p.search = StepStatus::Error;
                    p.search_result = Some(StepResult {
                        summary: "fallback".to_string(),
                        latency_ms: embedding_start.elapsed().as_millis() as u64,
                    });
                });
                emit_memory_activity(event_tx.as_ref());

                top_k_by_score(
                    self.collect_retrieval_candidates_scoped(MemoryScope::All)?
                        .into_iter()
                        .filter(|entry| entry.active)
                        .map(|entry| {
                            let score = memory_score(&entry) as f32;
                            (entry, score)
                        }),
                    MEMORY_RELEVANCE_MAX_CANDIDATES,
                )
                .into_iter()
                .map(|(entry, _)| (entry, 0.0))
                .collect()
            }
        };

        // Filter out memories that have already been injected in this session
        let pre_filter_count = candidates.len();
        let candidates: Vec<_> = candidates
            .into_iter()
            .filter(|(entry, _)| !is_memory_injected_any(&entry.id))
            .collect();
        if candidates.len() < pre_filter_count {
            crate::logging::info(&format!(
                "Filtered out {} already-injected memories ({} -> {} candidates)",
                pre_filter_count - candidates.len(),
                pre_filter_count,
                candidates.len()
            ));
        }

        if candidates.is_empty() {
            pipeline_update(|p| {
                p.verify = StepStatus::Skipped;
                p.inject = StepStatus::Skipped;
                p.maintain = StepStatus::Skipped;
            });
            set_state(MemoryState::Idle);
            emit_memory_activity(event_tx.as_ref());
            return Ok((None, Vec::new(), None));
        }

        if !memory_sidecar_enabled() {
            let relevant: Vec<_> = candidates
                .into_iter()
                .take(MEMORY_RELEVANCE_MAX_RESULTS)
                .map(|(entry, _)| entry)
                .collect();
            let relevant_ids: Vec<String> = relevant.iter().map(|entry| entry.id.clone()).collect();
            let _ = self.touch_entries(&relevant_ids);

            if relevant.is_empty() {
                pipeline_update(|p| {
                    p.verify = StepStatus::Skipped;
                    p.verify_result = Some(StepResult {
                        summary: "semantic only".to_string(),
                        latency_ms: 0,
                    });
                    p.inject = StepStatus::Skipped;
                    p.maintain = StepStatus::Skipped;
                });
                set_state(MemoryState::Idle);
                emit_memory_activity(event_tx.as_ref());
                return Ok((None, Vec::new(), None));
            }

            pipeline_update(|p| {
                p.verify = StepStatus::Skipped;
                p.verify_result = Some(StepResult {
                    summary: format!("semantic {}", relevant.len()),
                    latency_ms: 0,
                });
                p.inject = StepStatus::Running;
            });

            set_state(MemoryState::FoundRelevant {
                count: relevant.len(),
            });
            emit_memory_activity(event_tx.as_ref());

            let prompt = format_relevant_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);
            let display_prompt =
                format_relevant_display_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);

            pipeline_update(|p| {
                p.inject = StepStatus::Done;
                p.inject_result = Some(StepResult {
                    summary: format!("{} memories", relevant.len()),
                    latency_ms: 0,
                });
            });
            emit_memory_activity(event_tx.as_ref());

            return Ok((prompt, relevant_ids, display_prompt));
        }

        // Step 2: Sidecar verification (only for embedding hits - much fewer calls!)
        let total_candidates = candidates.len();
        set_state(MemoryState::SidecarChecking {
            count: total_candidates,
        });
        add_event(MemoryEventKind::SidecarStarted);
        pipeline_update(|p| {
            p.verify = StepStatus::Running;
            p.verify_progress = Some((0, total_candidates));
        });
        emit_memory_activity(event_tx.as_ref());

        let sidecar = Sidecar::new();
        let mut relevant = Vec::new();
        let mut relevant_ids = Vec::new();

        // Process in parallel batches
        const BATCH_SIZE: usize = 5;
        for batch in candidates.chunks(BATCH_SIZE) {
            let futures: Vec<_> = batch
                .iter()
                .map(|(memory, _sim)| {
                    let sidecar = sidecar.clone();
                    let content = memory.content.clone();
                    let ctx = context.clone();
                    async move {
                        let start = Instant::now();
                        let result = sidecar.check_relevance(&content, &ctx).await;
                        (result, start.elapsed())
                    }
                })
                .collect();

            let results = futures::future::join_all(futures).await;

            for ((memory, sim), (result, elapsed)) in batch.iter().zip(results) {
                match result {
                    Ok((is_relevant, _reason)) => {
                        add_event(MemoryEventKind::SidecarComplete {
                            latency_ms: elapsed.as_millis() as u64,
                        });

                        if is_relevant {
                            let preview = if memory.content.len() > 30 {
                                format!("{}...", crate::util::truncate_str(&memory.content, 30))
                            } else {
                                memory.content.clone()
                            };
                            add_event(MemoryEventKind::SidecarRelevant {
                                memory_preview: preview,
                            });
                            relevant_ids.push(memory.id.clone());
                            relevant.push(memory.clone());
                            crate::logging::info(&format!(
                                "[{}] Memory relevant (sim={:.2}): {}",
                                session_id,
                                sim,
                                crate::util::truncate_str(&memory.content, 50)
                            ));
                        } else {
                            add_event(MemoryEventKind::SidecarNotRelevant);
                        }
                    }
                    Err(e) => {
                        add_event(MemoryEventKind::Error {
                            message: e.to_string(),
                        });
                        crate::logging::info(&format!("Sidecar check failed: {}", e));
                    }
                }
                // Update verify progress
                let checked = relevant.len()
                    + batch.len().saturating_sub(
                        batch.len(), // approximate
                    );
                let _ = checked; // Progress updated below per-batch
            }
            // Update pipeline verify progress after each batch
            pipeline_update(|p| {
                p.verify_progress = Some((
                    relevant_ids.len()
                        + (total_candidates - candidates.len().min(total_candidates)),
                    total_candidates,
                ));
            });
            emit_memory_activity(event_tx.as_ref());
        }

        let verify_latency_ms = embedding_start.elapsed().as_millis() as u64;
        let _ = self.touch_entries(&relevant_ids);

        if relevant.is_empty() {
            pipeline_update(|p| {
                p.verify = StepStatus::Done;
                p.verify_result = Some(StepResult {
                    summary: "0 relevant".to_string(),
                    latency_ms: verify_latency_ms,
                });
                p.inject = StepStatus::Skipped;
                p.maintain = StepStatus::Skipped;
            });
            set_state(MemoryState::Idle);
            emit_memory_activity(event_tx.as_ref());
            return Ok((None, Vec::new(), None));
        }

        pipeline_update(|p| {
            p.verify = StepStatus::Done;
            p.verify_result = Some(StepResult {
                summary: format!("{} relevant", relevant.len()),
                latency_ms: verify_latency_ms,
            });
            p.inject = StepStatus::Running;
        });

        set_state(MemoryState::FoundRelevant {
            count: relevant.len(),
        });
        emit_memory_activity(event_tx.as_ref());

        let prompt = format_relevant_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);
        let display_prompt =
            format_relevant_display_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);

        // Mark inject as done - the prompt is ready for injection
        pipeline_update(|p| {
            p.inject = StepStatus::Done;
            p.inject_result = Some(StepResult {
                summary: format!("{} memories", relevant.len()),
                latency_ms: 0,
            });
        });
        emit_memory_activity(event_tx.as_ref());

        Ok((prompt, relevant_ids, display_prompt))
    }

    // ==================== Graph-Based Operations ====================

    /// Load project memories as a MemoryGraph with automatic migration
    pub fn load_project_graph(&self) -> Result<MemoryGraph> {
        let Some(path) = self.project_memory_path()? else {
            return Ok(MemoryGraph::new());
        };

        if !self.test_mode
            && let Some(mut graph) = cached_graph(&path)
        {
            if Self::normalize_graph_search_text(&mut graph) {
                cache_graph(path.clone(), &graph);
            }
            return Ok(graph);
        }

        if path.exists() {
            // Try loading as MemoryGraph first
            if let Ok(graph) = storage::read_json::<MemoryGraph>(&path)
                && graph.graph_version == GRAPH_VERSION
            {
                let mut graph = graph;
                let normalized = Self::normalize_graph_search_text(&mut graph);
                if self.import_legacy_notes_into_graph(&mut graph)? {
                    self.save_project_graph(&graph)?;
                } else if normalized {
                    storage::write_json(&path, &graph)?;
                }
                if !self.test_mode {
                    cache_graph(path, &graph);
                }
                return Ok(graph);
            }

            // Fall back to legacy MemoryStore and migrate
            let store: MemoryStore = storage::read_json(&path)?;
            let mut graph = MemoryGraph::from_legacy_store(store);
            let _ = self.import_legacy_notes_into_graph(&mut graph)?;

            // Save migrated format (create backup first)
            let backup_path = path.with_extension("json.bak");
            if !backup_path.exists() {
                let _ = std::fs::copy(&path, &backup_path);
            }
            storage::write_json(&path, &graph)?;

            crate::logging::info(&format!(
                "Migrated memory store to graph format: {}",
                path.display()
            ));
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        } else {
            let mut graph = MemoryGraph::new();
            if self.import_legacy_notes_into_graph(&mut graph)? {
                self.save_project_graph(&graph)?;
            }
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        }
    }

    /// Load global memories as a MemoryGraph with automatic migration
    pub fn load_global_graph(&self) -> Result<MemoryGraph> {
        let path = self.global_memory_path()?;
        if !self.test_mode
            && let Some(mut graph) = cached_graph(&path)
        {
            if Self::normalize_graph_search_text(&mut graph) {
                cache_graph(path.clone(), &graph);
            }
            return Ok(graph);
        }

        if path.exists() {
            // Try loading as MemoryGraph first
            if let Ok(graph) = storage::read_json::<MemoryGraph>(&path)
                && graph.graph_version == GRAPH_VERSION
            {
                let mut graph = graph;
                if Self::normalize_graph_search_text(&mut graph) {
                    storage::write_json(&path, &graph)?;
                }
                if !self.test_mode {
                    cache_graph(path, &graph);
                }
                return Ok(graph);
            }

            // Fall back to legacy MemoryStore and migrate
            let store: MemoryStore = storage::read_json(&path)?;
            let graph = MemoryGraph::from_legacy_store(store);

            // Save migrated format (create backup first)
            let backup_path = path.with_extension("json.bak");
            if !backup_path.exists() {
                let _ = std::fs::copy(&path, &backup_path);
            }
            storage::write_json(&path, &graph)?;

            crate::logging::info(&format!(
                "Migrated global memory store to graph format: {}",
                path.display()
            ));
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        } else {
            let graph = MemoryGraph::new();
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        }
    }

    /// Save project memories as a MemoryGraph
    pub fn save_project_graph(&self, graph: &MemoryGraph) -> Result<()> {
        if let Some(path) = self.project_memory_path()? {
            storage::write_json(&path, graph)?;
            if !self.test_mode {
                cache_graph(path, graph);
            }
        }
        Ok(())
    }

    /// Save global memories as a MemoryGraph
    pub fn save_global_graph(&self, graph: &MemoryGraph) -> Result<()> {
        let path = self.global_memory_path()?;
        storage::write_json(&path, graph)?;
        if !self.test_mode {
            cache_graph(path, graph);
        }
        Ok(())
    }

    /// Add a tag to a memory
    pub fn tag_memory(&self, memory_id: &str, tag: &str) -> Result<()> {
        // Try project first
        let mut graph = self.load_project_graph()?;
        if graph.memories.contains_key(memory_id) {
            graph.tag_memory(memory_id, tag);
            return self.save_project_graph(&graph);
        }

        // Try global
        let mut graph = self.load_global_graph()?;
        if graph.memories.contains_key(memory_id) {
            graph.tag_memory(memory_id, tag);
            return self.save_global_graph(&graph);
        }

        Err(anyhow::anyhow!("Memory not found: {}", memory_id))
    }

    /// Link two memories with a RelatesTo edge
    pub fn link_memories(&self, from_id: &str, to_id: &str, weight: f32) -> Result<()> {
        // Try project first
        let mut graph = self.load_project_graph()?;
        if graph.memories.contains_key(from_id) && graph.memories.contains_key(to_id) {
            graph.link_memories(from_id, to_id, weight);
            return self.save_project_graph(&graph);
        }

        // Try global
        let mut graph = self.load_global_graph()?;
        if graph.memories.contains_key(from_id) && graph.memories.contains_key(to_id) {
            graph.link_memories(from_id, to_id, weight);
            return self.save_global_graph(&graph);
        }

        // Cross-store links not supported for now
        Err(anyhow::anyhow!(
            "Both memories must be in the same store (project or global)"
        ))
    }

    /// Hebbian reinforcement of a co-relevance association between two
    /// memories. Strengthens (or creates) a symmetric `RelatesTo` link.
    pub fn reinforce_link(&self, from_id: &str, to_id: &str, delta: f32) -> Result<()> {
        let mut graph = self.load_project_graph()?;
        if graph.memories.contains_key(from_id) && graph.memories.contains_key(to_id) {
            graph.reinforce_link(from_id, to_id, delta);
            return self.save_project_graph(&graph);
        }
        let mut graph = self.load_global_graph()?;
        if graph.memories.contains_key(from_id) && graph.memories.contains_key(to_id) {
            graph.reinforce_link(from_id, to_id, delta);
            return self.save_global_graph(&graph);
        }
        Err(anyhow::anyhow!(
            "Both memories must be in the same store (project or global)"
        ))
    }

    /// Grow associations from shared tags on both stores. Returns total pairs
    /// linked/strengthened.
    pub fn bootstrap_cooccurrence_links(&self, min_overlap: f32) -> Result<usize> {
        let mut total = 0usize;
        let mut project = self.load_project_graph()?;
        let p = project.bootstrap_cooccurrence_links(min_overlap);
        if p > 0 {
            self.save_project_graph(&project)?;
            total += p;
        }
        let mut global = self.load_global_graph()?;
        let g = global.bootstrap_cooccurrence_links(min_overlap);
        if g > 0 {
            self.save_global_graph(&global)?;
            total += g;
        }
        Ok(total)
    }

    /// Fade unreinforced associations on both stores. Returns `(weakened, pruned)`.
    pub fn decay_relates_to(&self, factor: f32, floor: f32) -> Result<(usize, usize)> {
        let mut project = self.load_project_graph()?;
        let (pw, pp) = project.decay_relates_to(factor, floor);
        if pw > 0 {
            self.save_project_graph(&project)?;
        }
        let mut global = self.load_global_graph()?;
        let (gw, gp) = global.decay_relates_to(factor, floor);
        if gw > 0 {
            self.save_global_graph(&global)?;
        }
        Ok((pw + gw, pp + gp))
    }

    /// Run one offline consolidation ("sleep") pass over both stores and
    /// persist the results. Returns the combined report.
    pub fn run_sleep_cycle(&self) -> Result<crate::memory_graph::SleepReport> {
        use crate::memory_graph::{SleepConfig, SleepReport};
        let cfg = SleepConfig::default();
        let mut total = SleepReport::default();

        for is_project in [true, false] {
            let mut graph = if is_project {
                self.load_project_graph()?
            } else {
                self.load_global_graph()?
            };
            if graph.memory_count() == 0 {
                continue;
            }
            let r = graph.run_sleep_cycle(cfg);
            total.linked += r.linked;
            total.weakened += r.weakened;
            total.pruned += r.pruned;
            total.communities += r.communities;
            total.confidence_decayed += r.confidence_decayed;
            if is_project {
                self.save_project_graph(&graph)?;
            } else {
                self.save_global_graph(&graph)?;
            }
        }
        Ok(total)
    }

    /// Full cognitive-maintenance sleep cycle (Phase 4). Runs the deterministic
    /// graph steps (Hebbian links, association decay, edge pruning, community
    /// detection, importance-aware confidence decay, stats) and then the
    /// LLM/embedder steps (semantic consolidation, contradiction review, concept
    /// embedding refresh). Persists results and stores the report on the graph.
    pub async fn run_full_sleep_cycle(&self) -> Result<crate::memory_graph::SleepReport> {
        use crate::memory_graph::SleepConfig;
        let cfg = SleepConfig::default();
        let sidecar_on = memory_sidecar_enabled();
        let embed_on = crate::embedding::is_model_available() && !self.test_mode;
        let mut total = crate::memory_graph::SleepReport::default();

        for is_project in [true, false] {
            let mut graph = if is_project {
                self.load_project_graph()?
            } else {
                self.load_global_graph()?
            };
            // Unified knowledge layer: refresh registered sources first so
            // graph maintenance (links, communities, consolidation, concept
            // embeddings) runs over up-to-date repository knowledge, and fold
            // queued tool-outcome evidence into the affected concepts.
            let mut knowledge_refreshed = 0usize;
            if is_project && !self.test_mode {
                let opts = crate::knowledge::IngestOptions::default();
                for (source_id, r) in
                    crate::knowledge::refresh_sources_in_graph(&mut graph, opts).await
                {
                    knowledge_refreshed += r.concepts_created + r.concepts_updated;
                    crate::logging::info(&format!(
                        "sleep: knowledge refresh {}",
                        r.render(&source_id)
                    ));
                }
            }

            if graph.memory_count() == 0 {
                continue;
            }

            // Deterministic graph steps: links, decay, prune, community, confidence.
            let mut report = graph.run_sleep_cycle(cfg);
            report.knowledge_concepts_refreshed = knowledge_refreshed;

            // --- Semantic consolidation (LLM merge text, graph-side idempotent) ---
            let groups = graph.consolidation_candidates(2);
            let mut consolidated = 0usize;
            for group in groups {
                let contents: Vec<String> = group
                    .iter()
                    .filter_map(|id| graph.get_memory(id))
                    .map(|m| m.content.clone())
                    .collect();
                if contents.len() < 2 {
                    continue;
                }
                let concept = dominant_group_tag(&group, &graph);
                let summary = summarize_group_with_sidecar(&contents, sidecar_on).await;
                if let Some(summary) = summary
                    && graph.apply_consolidation(&group, &summary, &concept).is_some()
                {
                    consolidated += 1;
                }
            }

            // --- Contradiction review (LLM, graph knowledge only) ---
            let mut contradictions = 0usize;
            if sidecar_on {
                let pairs = graph.contradiction_candidates(0.6, 24);
                // Snapshot contents up front to avoid borrowing across the await.
                let mut jobs: Vec<(String, String, String, String)> = Vec::new();
                for (a, b) in pairs {
                    if let (Some(ma), Some(mb)) = (graph.get_memory(&a), graph.get_memory(&b)) {
                        jobs.push((a.clone(), b.clone(), ma.content.clone(), mb.content.clone()));
                    }
                }
                for (a, b, ta, tb) in jobs {
                    if let Some(reason) = review_contradiction_with_sidecar(&ta, &tb).await {
                        graph.apply_contradiction(&a, &b, &reason);
                        contradictions += 1;
                    }
                }
            }

            // --- Concept (graph-neighborhood) embedding refresh ---
            let refreshed = if embed_on {
                graph.refresh_concept_embeddings(|t| crate::embedding::embed(t).ok())
            } else {
                0
            };

            report.consolidated = consolidated;
            report.contradictions_found = contradictions;
            report.concept_embeddings_refreshed = refreshed;
            report.at = Some(Utc::now());
            graph.metadata.last_sleep = Some(report.clone());

            if is_project {
                self.save_project_graph(&graph)?;
            } else {
                self.save_global_graph(&graph)?;
            }

            total.linked += report.linked;
            total.weakened += report.weakened;
            total.pruned += report.pruned;
            total.communities += report.communities;
            total.consolidated += report.consolidated;
            total.contradictions_found += report.contradictions_found;
            total.concept_embeddings_refreshed += report.concept_embeddings_refreshed;
            total.confidence_decayed += report.confidence_decayed;
            total.knowledge_concepts_refreshed += report.knowledge_concepts_refreshed;
        }
        total.at = Some(Utc::now());
        Ok(total)
    }

    /// Validate graph integrity across both stores (Phase 6). Returns
    /// `(project_issues, global_issues)`.
    pub fn validate_graphs(
        &self,
    ) -> Result<(
        Vec<crate::memory_graph::GraphIssue>,
        Vec<crate::memory_graph::GraphIssue>,
    )> {
        let project = self.load_project_graph()?.validate();
        let global = self.load_global_graph()?.validate();
        Ok((project, global))
    }

    /// Recompute concept communities on both stores (label propagation) and
    /// persist. Returns total communities formed.
    pub fn recompute_communities(&self, min_size: usize) -> Result<usize> {
        let mut total = 0usize;
        for is_project in [true, false] {
            let mut graph = if is_project {
                self.load_project_graph()?
            } else {
                self.load_global_graph()?
            };
            if graph.memory_count() == 0 {
                continue;
            }
            let n = graph.detect_communities(min_size, 8);
            if is_project {
                self.save_project_graph(&graph)?;
            } else {
                self.save_global_graph(&graph)?;
            }
            total += n;
        }
        Ok(total)
    }

    /// Get memories related to a given memory via graph traversal
    pub fn get_related(&self, memory_id: &str, depth: usize) -> Result<Vec<MemoryEntry>> {
        // Find which store contains the memory
        let (mut graph, _is_project) = {
            let project_graph = self.load_project_graph()?;
            if project_graph.memories.contains_key(memory_id) {
                (project_graph, true)
            } else {
                let global_graph = self.load_global_graph()?;
                if global_graph.memories.contains_key(memory_id) {
                    (global_graph, false)
                } else {
                    return Err(anyhow::anyhow!("Memory not found: {}", memory_id));
                }
            }
        };

        // Use cascade retrieval to find related memories
        let results = graph.cascade_retrieve(&[memory_id.to_string()], &[1.0], depth, 20);

        // Collect memory entries (excluding the seed)
        let entries: Vec<MemoryEntry> = results
            .into_iter()
            .filter(|(id, _)| id != memory_id)
            .filter_map(|(id, _)| graph.get_memory(&id).cloned())
            .collect();

        Ok(entries)
    }

    /// Find similar memories with cascade retrieval through the graph
    ///
    /// This extends the basic embedding search by also traversing through
    /// tags to find related memories that might not have direct embedding similarity.
    pub fn find_similar_with_cascade(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        self.find_similar_with_cascade_scoped(text, threshold, limit, MemoryScope::All)
    }

    pub fn find_similar_with_cascade_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        // First, do basic embedding search
        let embedding_hits = self.find_similar_scoped(text, threshold, limit, scope)?;

        if embedding_hits.is_empty() {
            return Ok(Vec::new());
        }

        // Get seed IDs and scores
        let seed_ids: Vec<String> = embedding_hits.iter().map(|(e, _)| e.id.clone()).collect();
        let seed_scores: Vec<f32> = embedding_hits.iter().map(|(_, s)| *s).collect();

        // Load graphs and perform cascade retrieval
        let mut project_graph = if scope.includes_project() {
            Some(self.load_project_graph()?)
        } else {
            None
        };
        let mut global_graph = if scope.includes_global() {
            Some(self.load_global_graph()?)
        } else {
            None
        };

        // Cascade through project graph
        let project_cascade = project_graph
            .as_mut()
            .map(|graph| graph.cascade_retrieve(&seed_ids, &seed_scores, 2, limit * 2))
            .unwrap_or_default();

        // Cascade through global graph
        let global_cascade = global_graph
            .as_mut()
            .map(|graph| graph.cascade_retrieve(&seed_ids, &seed_scores, 2, limit * 2))
            .unwrap_or_default();

        // Merge results, keeping highest score for each memory
        let mut merged: std::collections::HashMap<String, f32> = std::collections::HashMap::new();

        for (id, score) in embedding_hits.iter() {
            merged.insert(id.id.clone(), *score);
        }
        for (id, score) in project_cascade {
            let existing = merged.get(&id).copied().unwrap_or(0.0);
            if score > existing {
                merged.insert(id, score);
            }
        }
        for (id, score) in global_cascade {
            let existing = merged.get(&id).copied().unwrap_or(0.0);
            if score > existing {
                merged.insert(id, score);
            }
        }

        // Look up entries and keep only the top-scoring results
        let results: Vec<(MemoryEntry, f32)> = top_k_by_score(
            merged.into_iter().filter_map(|(id, score)| {
                project_graph
                    .as_ref()
                    .and_then(|graph| graph.get_memory(&id))
                    .or_else(|| {
                        global_graph
                            .as_ref()
                            .and_then(|graph| graph.get_memory(&id))
                    })
                    .cloned()
                    .map(|entry| (entry, score))
            }),
            limit,
        );

        Ok(results)
    }

    /// Expand a seed set of memory IDs by walking the graph (tags, `RelatesTo`
    /// links, clusters) and return *related* memories that are NOT already
    /// seeds. This is the live wiring for graph-native retrieval: embedding
    /// search finds the seeds, then the graph pulls in neighbours the raw
    /// vector search would miss (e.g. two memories that share a tag or were
    /// linked by co-relevance on a prior turn).
    ///
    /// Runs against both the project and global graphs, persisting each so the
    /// graph's `retrieval_count` reflects real usage. Results are deduped by id
    /// (best score wins) and truncated to `max_results`.
    pub fn cascade_expand(
        &self,
        seed_ids: &[String],
        seed_scores: &[f32],
        depth: usize,
        max_results: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        if seed_ids.is_empty() || max_results == 0 {
            return Ok(Vec::new());
        }

        let seed_set: std::collections::HashSet<&str> =
            seed_ids.iter().map(String::as_str).collect();
        let mut best: std::collections::HashMap<String, (MemoryEntry, f32)> =
            std::collections::HashMap::new();

        for is_project in [true, false] {
            let mut graph = if is_project {
                self.load_project_graph()?
            } else {
                self.load_global_graph()?
            };

            // Only seeds that live in this graph can start a walk here.
            let mut ids: Vec<String> = Vec::new();
            let mut scores: Vec<f32> = Vec::new();
            for (id, score) in seed_ids.iter().zip(seed_scores.iter()) {
                if graph.memories.contains_key(id) {
                    ids.push(id.clone());
                    scores.push(*score);
                }
            }
            if ids.is_empty() {
                continue;
            }

            let cascade = graph.cascade_retrieve(&ids, &scores, depth, max_results * 2);
            for (id, score) in cascade {
                if seed_set.contains(id.as_str()) {
                    continue;
                }
                if let Some(entry) = graph.get_memory(&id)
                    && entry.active
                {
                    best.entry(id)
                        .and_modify(|(_, s)| {
                            if score > *s {
                                *s = score;
                            }
                        })
                        .or_insert_with(|| (entry.clone(), score));
                }
            }

            // Persist so retrieval_count / graph metadata reflect real usage.
            let _ = if is_project {
                self.save_project_graph(&graph)
            } else {
                self.save_global_graph(&graph)
            };
        }

        let mut merged: Vec<(MemoryEntry, f32)> = best.into_values().collect();
        merged.sort_by(|a, b| b.1.total_cmp(&a.1));
        merged.truncate(max_results);
        Ok(merged)
    }

    /// Get graph statistics for display
    pub fn graph_stats(&self) -> Result<(usize, usize, usize, usize)> {
        let project = self.load_project_graph()?;
        let global = self.load_global_graph()?;

        let memories = project.memories.len() + global.memories.len();
        let tags = project.tags.len() + global.tags.len();
        let edges = project.edge_count() + global.edge_count();
        let clusters = project.clusters.len() + global.clusters.len();

        Ok((memories, tags, edges, clusters))
    }
}

/// Maximal Marginal Relevance selection: pick `k` entries that balance
/// relevance (their existing order — most relevant first) against novelty
/// (low embedding similarity to already-picked entries). `lambda` weights
/// relevance vs. diversity (1.0 = pure relevance, 0.0 = pure diversity).
///
/// Prevents injecting several near-duplicate memories. Returns the selection in
/// the original relevance order. A no-op when `items.len() <= k`.
pub fn mmr_select(items: Vec<MemoryEntry>, k: usize, lambda: f32) -> Vec<MemoryEntry> {
    if k == 0 {
        return Vec::new();
    }
    if items.len() <= k {
        return items;
    }

    let n = items.len();
    // Relevance proxy from the incoming order (sidecar already ranked these).
    let rel: Vec<f32> = (0..n).map(|i| 1.0 - (i as f32) / (n as f32)).collect();

    let sim = |a: &MemoryEntry, b: &MemoryEntry| -> f32 {
        match (&a.embedding, &b.embedding) {
            (Some(ea), Some(eb)) => crate::embedding::cosine_similarity(ea, eb).max(0.0),
            _ => 0.0,
        }
    };

    let mut selected: Vec<usize> = Vec::with_capacity(k);
    let mut remaining: Vec<usize> = (0..n).collect();

    while selected.len() < k && !remaining.is_empty() {
        let mut best_pos = 0usize;
        let mut best_score = f32::MIN;
        for (pos, &c) in remaining.iter().enumerate() {
            let max_sim = selected
                .iter()
                .map(|&s| sim(&items[c], &items[s]))
                .fold(0.0_f32, f32::max);
            let score = lambda * rel[c] - (1.0 - lambda) * max_sim;
            if score > best_score {
                best_score = score;
                best_pos = pos;
            }
        }
        selected.push(remaining.remove(best_pos));
    }

    // Preserve original relevance ordering among the winners.
    selected.sort_unstable();
    let mut items = items;
    let keep: std::collections::HashSet<usize> = selected.into_iter().collect();
    let mut out = Vec::with_capacity(keep.len());
    for (i, entry) in items.drain(..).enumerate() {
        if keep.contains(&i) {
            out.push(entry);
        }
    }
    out
}

/// Embedding similarity threshold (0.0 - 1.0)
/// Lower = more candidates, higher = fewer but more relevant
pub const EMBEDDING_SIMILARITY_THRESHOLD: f32 = 0.5;

/// Maximum embedding hits to verify with sidecar
pub const EMBEDDING_MAX_HITS: usize = 10;

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
