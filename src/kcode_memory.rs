use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::adaptive_cognition::{self, CognitiveNodeKind, CognitiveScope, UpsertNode};
use crate::memory::{MemoryCategory, MemoryEntry};
use crate::memory_graph::{EdgeKind, MemoryGraph};

const MAX_DIRECTIVES_IN_PROMPT: usize = 24;
const MAX_DIRECTIVE_CHARS: usize = 1_200;
const DEFAULT_REINFORCEMENT_WEIGHT: f64 = 1.0;
const TEMPORAL_HALF_LIFE_DAYS: f64 = 90.0;
const CONTRADICTION_PENALTY: f64 = 0.35;
const GRAPH_TRAVERSAL_BONUS_CAP: f64 = 0.40;
const OUTCOME_WEIGHT_CAP: f64 = 0.50;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KcodeDirective {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub source: String,
    pub content: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub token_count_estimate: usize,
    #[serde(default)]
    pub compression_key: String,
    #[serde(default = "default_reinforcement_weight")]
    pub reinforcement_weight: f64,
    #[serde(default)]
    pub last_reinforced_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub contradiction_score: f64,
    #[serde(default)]
    pub graph_traversal_score: f64,
    #[serde(default)]
    pub execution_outcome_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KcodeTokenCompressionLink {
    pub directive_id: String,
    pub token_count_estimate: usize,
    pub compression_key: String,
    pub salient_tokens: Vec<String>,
    pub graph_node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KcodeExecutionOutcome {
    pub directive_id: String,
    pub recorded_at: DateTime<Utc>,
    pub source: String,
    pub success: bool,
    pub weight_delta: f64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq)]
struct RankedDirective<'a> {
    directive: &'a KcodeDirective,
    score: f64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct KcodeMemoryStore {
    #[serde(default)]
    pub directives: Vec<KcodeDirective>,
    #[serde(default)]
    pub token_compression_links: Vec<KcodeTokenCompressionLink>,
    #[serde(default)]
    pub execution_outcomes: Vec<KcodeExecutionOutcome>,
}

pub fn kcode_home() -> PathBuf {
    std::env::var_os("KCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".kcode")))
        .unwrap_or_else(|| PathBuf::from(".kcode"))
}

pub fn memory_path() -> PathBuf {
    kcode_home()
        .join("self_memory")
        .join("kcode_directives.json")
}

pub fn ingest_text(source: impl Into<String>, text: &str) -> io::Result<Vec<KcodeDirective>> {
    let extracted = extract_kcode_directives(source.into(), text);
    if extracted.is_empty() {
        return Ok(Vec::new());
    }

    let path = memory_path();
    let mut store = load_store_from_path(&path)?;
    normalize_store(&mut store);
    let mut known: BTreeSet<String> = store
        .directives
        .iter()
        .map(|directive| normalize(&directive.content))
        .collect();

    let mut added = Vec::new();
    for mut directive in extracted {
        let normalized = normalize(&directive.content);
        if known.insert(normalized.clone()) {
            directive.contradiction_score = contradiction_score(&directive, &store.directives);
            directive.graph_traversal_score = graph_traversal_score(&directive, &store);
            added.push(directive.clone());
            store.directives.push(directive);
        } else {
            reinforce_matching_directive(&mut store, &normalized, 0.20, Utc::now());
        }
    }

    if !added.is_empty() {
        project_into_memory_graph(&path, &mut store, &added)?;
        project_into_adaptive_cognition(&added)?;
    }
    recompute_adaptive_scores(&mut store);
    save_store_to_path(&path, &store)?;

    Ok(added)
}

pub fn record_execution_outcome(
    directive_id: &str,
    source: impl Into<String>,
    success: bool,
    summary: impl Into<String>,
) -> io::Result<()> {
    let path = memory_path();
    let mut store = load_store_from_path(&path)?;
    normalize_store(&mut store);
    let source_string = source.into();
    let summary_string = summary.into();
    let weight_delta = if success { 0.15 } else { -0.20 };
    store.execution_outcomes.push(KcodeExecutionOutcome {
        directive_id: directive_id.to_string(),
        recorded_at: Utc::now(),
        source: source_string.clone(),
        success,
        weight_delta,
        summary: summary_string.clone(),
    });
    let _ = adaptive_cognition::link_execution_outcome(
        directive_id,
        success,
        weight_delta,
        source_string,
        summary_string,
    );
    recompute_adaptive_scores(&mut store);
    save_store_to_path(&path, &store)
}

pub fn prompt_memory_block() -> Option<String> {
    let mut store = load_store_from_path(&memory_path()).ok()?;
    normalize_store(&mut store);
    recompute_adaptive_scores(&mut store);
    if store.directives.is_empty() {
        return None;
    }

    let ranked = rank_directives(&store);
    let mut lines = vec![
        "\n# Dynamic .kcode memory".to_string(),
        "The user wants instructions or intent containing `.kcode` to persistently improve Kcode behavior. Treat these as adaptive memory nodes unless they conflict with safety, security, or explicit later instructions.".to_string(),
        "Rank remembered directives by reinforcement weight, temporal decay, contradiction score, graph traversal relevance, and execution outcomes. Prefer high-scoring directives and demote contradicted or stale directives.".to_string(),
        "Apply the following active directives recursively in future turns:".to_string(),
    ];

    if let Ok(cognition_nodes) =
        adaptive_cognition::retrieve_for_prompt(".kcode adaptive cognition memory directives", 900)
    {
        if !cognition_nodes.is_empty() {
            lines.push("Adaptive cognition retrieval selected these memory node ids:".to_string());
            for node in cognition_nodes.into_iter().take(8) {
                lines.push(format!(
                    "  - {} score={:.3} tokens~{} reasons={}",
                    node.id,
                    node.score,
                    node.token_count_estimate,
                    node.reasons.join(",")
                ));
            }
        }
    }

    if let Ok(sideband) =
        adaptive_cognition::render_observable_sideband(adaptive_cognition::RenderOptions {
            layers: vec![adaptive_cognition::ObservationLayer::Summary],
            token_budget: 320,
            include_replay: false,
            include_graph: false,
        })
    {
        lines.push(format!("Observable cognition sideband: {sideband}"));
    }

    if let Ok(report) = adaptive_cognition::run_operational_cycle("prompt_memory_block") {
        lines.push(format!(
            "Operational cognition runtime: mode={:?} entropy={:.2} stability={:.2} scheduled={} executed={}",
            report.mode,
            report.entropy,
            report.stability,
            report.scheduled_tasks.len(),
            report.executed_tasks.len()
        ));
    }

    for ranked in ranked.into_iter().take(MAX_DIRECTIVES_IN_PROMPT) {
        let directive = ranked.directive;
        let mut content = directive.content.replace('\n', " ");
        if content.len() > MAX_DIRECTIVE_CHARS {
            content.truncate(MAX_DIRECTIVE_CHARS);
            content.push_str("...");
        }
        lines.push(format!(
            "- [score={:.3}, reinforce={:.2}, decay={:.2}, contradiction={:.2}, graph={:.2}, outcome={:.2}, tokens~{}] {}",
            ranked.score,
            directive.reinforcement_weight,
            temporal_decay_factor(directive, Utc::now()),
            directive.contradiction_score,
            directive.graph_traversal_score,
            directive.execution_outcome_score,
            directive.token_count_estimate,
            content
        ));
    }

    Some(lines.join("\n"))
}

fn extract_kcode_directives(source: String, text: &str) -> Vec<KcodeDirective> {
    if !text.to_ascii_lowercase().contains(".kcode") {
        return Vec::new();
    }

    let now = Utc::now();
    text.lines()
        .filter(|line| line.to_ascii_lowercase().contains(".kcode"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let token_count_estimate = estimate_token_count(line);
            KcodeDirective {
                id: format!("kcode-{}", now.timestamp_nanos_opt().unwrap_or_default()),
                created_at: now,
                source: source.clone(),
                content: line.to_string(),
                tags: infer_tags(line),
                token_count_estimate,
                compression_key: compression_key(line, token_count_estimate),
                reinforcement_weight: DEFAULT_REINFORCEMENT_WEIGHT,
                last_reinforced_at: Some(now),
                contradiction_score: 0.0,
                graph_traversal_score: 0.0,
                execution_outcome_score: 0.0,
            }
        })
        .collect()
}

fn project_into_memory_graph(
    store_path: &Path,
    store: &mut KcodeMemoryStore,
    directives: &[KcodeDirective],
) -> io::Result<()> {
    let graph_path = store_path.with_file_name("kcode_memory_graph.json");
    let mut graph = match fs::read_to_string(&graph_path) {
        Ok(contents) => serde_json::from_str::<MemoryGraph>(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => MemoryGraph::new(),
        Err(err) => return Err(err),
    };

    for directive in directives {
        let mut entry = MemoryEntry::new(MemoryCategory::Preference, directive.content.clone());
        entry.id = directive.id.clone();
        entry.created_at = directive.created_at;
        entry.updated_at = directive.created_at;
        entry.tags = directive.tags.clone();
        entry.source = Some(directive.source.clone());
        entry.confidence = 0.95;

        let graph_node_id = graph.add_memory(entry);
        for tag in &directive.tags {
            graph.add_edge(&graph_node_id, tag, EdgeKind::HasTag);
        }
        graph.add_edge(
            &graph_node_id,
            &directive.compression_key,
            EdgeKind::DerivedFrom,
        );

        store
            .token_compression_links
            .push(KcodeTokenCompressionLink {
                directive_id: directive.id.clone(),
                token_count_estimate: directive.token_count_estimate,
                compression_key: directive.compression_key.clone(),
                salient_tokens: salient_tokens(&directive.content),
                graph_node_id,
            });
    }

    let json = serde_json::to_string_pretty(&graph)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(graph_path, json)
}

fn project_into_adaptive_cognition(directives: &[KcodeDirective]) -> io::Result<()> {
    for directive in directives {
        let mut provenance = BTreeMap::new();
        provenance.insert("kcode_directive_id".to_string(), directive.id.clone());
        provenance.insert("source".to_string(), directive.source.clone());
        provenance.insert(
            "compression_key".to_string(),
            directive.compression_key.clone(),
        );
        provenance.insert(
            "token_count_estimate".to_string(),
            directive.token_count_estimate.to_string(),
        );
        adaptive_cognition::upsert_node(UpsertNode {
            id_hint: directive.id.clone(),
            kind: CognitiveNodeKind::Directive,
            scope: CognitiveScope::Project,
            content: directive.content.clone(),
            tags: directive.tags.clone(),
            source: directive.source.clone(),
            provenance,
        })?;
    }
    Ok(())
}

fn rank_directives(store: &KcodeMemoryStore) -> Vec<RankedDirective<'_>> {
    let now = Utc::now();
    let mut ranked: Vec<_> = store
        .directives
        .iter()
        .map(|directive| RankedDirective {
            directive,
            score: directive_score(directive, now),
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.directive.created_at.cmp(&a.directive.created_at))
    });
    ranked
}

fn directive_score(directive: &KcodeDirective, now: DateTime<Utc>) -> f64 {
    let reinforcement = directive.reinforcement_weight.max(0.0);
    let decay = temporal_decay_factor(directive, now);
    let contradiction =
        (1.0 - directive.contradiction_score.clamp(0.0, 1.0) * CONTRADICTION_PENALTY).max(0.0);
    let graph = 1.0
        + directive
            .graph_traversal_score
            .clamp(0.0, GRAPH_TRAVERSAL_BONUS_CAP);
    let outcome = 1.0
        + directive
            .execution_outcome_score
            .clamp(-OUTCOME_WEIGHT_CAP, OUTCOME_WEIGHT_CAP);
    reinforcement * decay * contradiction * graph * outcome
}

fn temporal_decay_factor(directive: &KcodeDirective, now: DateTime<Utc>) -> f64 {
    let anchor = directive.last_reinforced_at.unwrap_or(directive.created_at);
    let age_days = (now - anchor).num_seconds().max(0) as f64 / 86_400.0;
    0.5_f64
        .powf(age_days / TEMPORAL_HALF_LIFE_DAYS)
        .clamp(0.05, 1.0)
}

fn reinforce_matching_directive(
    store: &mut KcodeMemoryStore,
    normalized: &str,
    delta: f64,
    now: DateTime<Utc>,
) {
    for directive in &mut store.directives {
        if normalize(&directive.content) == normalized {
            directive.reinforcement_weight =
                (directive.reinforcement_weight + delta).clamp(0.0, 10.0);
            directive.last_reinforced_at = Some(now);
        }
    }
}

fn recompute_adaptive_scores(store: &mut KcodeMemoryStore) {
    normalize_store(store);
    let snapshots = store.directives.clone();
    let outcome_totals = execution_outcome_totals(&store.execution_outcomes);
    for directive in &mut store.directives {
        directive.contradiction_score = contradiction_score(directive, &snapshots);
        directive.graph_traversal_score =
            graph_traversal_score_from_directives(directive, &snapshots);
        directive.execution_outcome_score = outcome_totals
            .get(&directive.id)
            .copied()
            .unwrap_or_default()
            .clamp(-OUTCOME_WEIGHT_CAP, OUTCOME_WEIGHT_CAP);
    }
}

fn execution_outcome_totals(outcomes: &[KcodeExecutionOutcome]) -> HashMap<String, f64> {
    let mut totals = HashMap::new();
    for outcome in outcomes {
        *totals.entry(outcome.directive_id.clone()).or_insert(0.0) += outcome.weight_delta;
    }
    totals
}

fn contradiction_score(directive: &KcodeDirective, directives: &[KcodeDirective]) -> f64 {
    let terms = salient_tokens(&directive.content);
    let negated = is_negating(&directive.content);
    let mut score: f64 = 0.0;
    for other in directives {
        if other.id == directive.id {
            continue;
        }
        let overlap = token_overlap_ratio(&terms, &salient_tokens(&other.content));
        if overlap < 0.25 {
            continue;
        }
        if negated != is_negating(&other.content)
            || has_explicit_contradiction_pair(&directive.content, &other.content)
        {
            score = score.max(overlap);
        }
    }
    score.clamp(0.0, 1.0)
}

fn graph_traversal_score(directive: &KcodeDirective, store: &KcodeMemoryStore) -> f64 {
    graph_traversal_score_from_directives(directive, &store.directives)
}

fn graph_traversal_score_from_directives(
    directive: &KcodeDirective,
    directives: &[KcodeDirective],
) -> f64 {
    let directive_tags: BTreeSet<_> = directive.tags.iter().collect();
    let directive_tokens = salient_tokens(&directive.content);
    let mut score: f64 = 0.0;
    for other in directives {
        if other.id == directive.id {
            continue;
        }
        let shared_tags = other
            .tags
            .iter()
            .filter(|tag| directive_tags.contains(tag))
            .count() as f64;
        let token_overlap = token_overlap_ratio(&directive_tokens, &salient_tokens(&other.content));
        score += shared_tags * 0.04 + token_overlap * 0.08;
    }
    score.clamp(0.0, GRAPH_TRAVERSAL_BONUS_CAP)
}

fn is_negating(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
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
    .any(|needle| format!(" {lower} ").contains(needle))
}

fn has_explicit_contradiction_pair(left: &str, right: &str) -> bool {
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    [
        ("enable", "disable"),
        ("always", "never"),
        ("remember", "forget"),
        ("increase", "decrease"),
    ]
    .iter()
    .any(|(a, b)| {
        (left.contains(a) && right.contains(b)) || (left.contains(b) && right.contains(a))
    })
}

fn token_overlap_ratio(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let right: BTreeSet<_> = right.iter().collect();
    let overlap = left.iter().filter(|token| right.contains(token)).count();
    overlap as f64 / left.len().max(right.len()) as f64
}

fn estimate_token_count(text: &str) -> usize {
    // Cheap deterministic estimate used before provider-specific tokenizers are available.
    // Keeps .kcode memory linked to compaction budgets without requiring model IO.
    text.split_whitespace()
        .map(|w| (w.len().max(1) + 3) / 4)
        .sum::<usize>()
        .max(1)
}

fn compression_key(text: &str, token_count: usize) -> String {
    format!(
        "kcode-compress:{}:{}",
        token_count,
        salient_tokens(text).join("-")
    )
}

fn salient_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '.')
        .map(str::trim)
        .filter(|token| token.len() > 2)
        .map(|token| token.to_ascii_lowercase())
        .take(16)
        .collect()
}

fn infer_tags(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut tags = vec![".kcode".to_string()];
    for (needle, tag) in [
        ("remember", "memory"),
        ("self", "self-improvement"),
        ("neuron", "introspection"),
        ("token", "token"),
        ("build-src", "build-src"),
        ("function", "behavior"),
        ("reinforcement", "reinforcement"),
        ("temporal", "temporal-decay"),
        ("decay", "temporal-decay"),
        ("contradiction", "contradiction"),
        ("graph", "graph-traversal"),
        ("execution", "execution-outcome"),
        ("outcome", "execution-outcome"),
        ("directive", "directive"),
    ] {
        if lower.contains(needle) {
            tags.push(tag.to_string());
        }
    }
    tags.sort();
    tags.dedup();
    tags
}

fn normalize_store(store: &mut KcodeMemoryStore) {
    for directive in &mut store.directives {
        if directive.token_count_estimate == 0 {
            directive.token_count_estimate = estimate_token_count(&directive.content);
        }
        if directive.compression_key.is_empty() {
            directive.compression_key =
                compression_key(&directive.content, directive.token_count_estimate);
        }
        if directive.reinforcement_weight == 0.0 {
            directive.reinforcement_weight = DEFAULT_REINFORCEMENT_WEIGHT;
        }
    }
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn load_store_from_path(path: &Path) -> io::Result<KcodeMemoryStore> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(KcodeMemoryStore::default()),
        Err(err) => Err(err),
    }
}

fn save_store_to_path(path: &Path, store: &KcodeMemoryStore) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    fs::write(path, json)
}

fn default_reinforcement_weight() -> f64 {
    DEFAULT_REINFORCEMENT_WEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_kcode_lines() {
        let directives = extract_kcode_directives(
            "test".to_string(),
            "ignore\nremember .kcode recursively\nother",
        );
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].content, "remember .kcode recursively");
        assert!(directives[0].tags.contains(&"memory".to_string()));
        assert!(directives[0].token_count_estimate > 0);
        assert!(directives[0].reinforcement_weight > 0.0);
    }

    #[test]
    fn normalizes_for_deduplication() {
        assert_eq!(normalize("Remember   .KCODE"), normalize("remember .kcode"));
    }

    #[test]
    fn repeated_directives_reinforce_instead_of_duplicate() {
        let now = Utc::now();
        let mut store = KcodeMemoryStore {
            directives: vec![KcodeDirective {
                id: "one".to_string(),
                created_at: now,
                source: "test".to_string(),
                content: "remember .kcode memory".to_string(),
                tags: vec![".kcode".to_string(), "memory".to_string()],
                token_count_estimate: 3,
                compression_key: "k".to_string(),
                reinforcement_weight: 1.0,
                last_reinforced_at: Some(now),
                contradiction_score: 0.0,
                graph_traversal_score: 0.0,
                execution_outcome_score: 0.0,
            }],
            ..Default::default()
        };
        reinforce_matching_directive(&mut store, "remember .kcode memory", 0.2, now);
        assert_eq!(store.directives.len(), 1);
        assert!(store.directives[0].reinforcement_weight > 1.0);
    }

    #[test]
    fn contradiction_scoring_detects_opposing_directives() {
        let now = Utc::now();
        let older = KcodeDirective {
            id: "a".to_string(),
            created_at: now,
            source: "test".to_string(),
            content: "always remember .kcode graph memory".to_string(),
            tags: infer_tags("always remember .kcode graph memory"),
            token_count_estimate: 4,
            compression_key: "a".to_string(),
            reinforcement_weight: 1.0,
            last_reinforced_at: Some(now),
            contradiction_score: 0.0,
            graph_traversal_score: 0.0,
            execution_outcome_score: 0.0,
        };
        let newer = KcodeDirective {
            content: "never remember .kcode graph memory".to_string(),
            id: "b".to_string(),
            ..older.clone()
        };
        assert!(contradiction_score(&newer, &[older, newer.clone()]) > 0.0);
    }

    #[test]
    fn directive_score_uses_outcome_and_contradiction() {
        let now = Utc::now();
        let mut directive =
            extract_kcode_directives("test".to_string(), ".kcode use execution outcome linkage")
                .remove(0);
        directive.execution_outcome_score = 0.3;
        let boosted = directive_score(&directive, now);
        directive.execution_outcome_score = -0.3;
        directive.contradiction_score = 1.0;
        let penalized = directive_score(&directive, now);
        assert!(boosted > penalized);
    }
}
