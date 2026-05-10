use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::memory::{MemoryCategory, MemoryEntry};
use crate::memory_graph::{EdgeKind, MemoryGraph};

const MAX_DIRECTIVES_IN_PROMPT: usize = 24;
const MAX_DIRECTIVE_CHARS: usize = 1_200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KcodeDirective {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub source: String,
    pub content: String,
    pub tags: Vec<String>,
    pub token_count_estimate: usize,
    pub compression_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KcodeTokenCompressionLink {
    pub directive_id: String,
    pub token_count_estimate: usize,
    pub compression_key: String,
    pub salient_tokens: Vec<String>,
    pub graph_node_id: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KcodeMemoryStore {
    pub directives: Vec<KcodeDirective>,
    pub token_compression_links: Vec<KcodeTokenCompressionLink>,
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
    let mut known: BTreeSet<String> = store
        .directives
        .iter()
        .map(|directive| normalize(&directive.content))
        .collect();

    let mut added = Vec::new();
    for directive in extracted {
        if known.insert(normalize(&directive.content)) {
            added.push(directive.clone());
            store.directives.push(directive);
        }
    }

    if !added.is_empty() {
        project_into_memory_graph(&path, &mut store, &added)?;
        save_store_to_path(&path, &store)?;
    }

    Ok(added)
}

pub fn prompt_memory_block() -> Option<String> {
    let store = load_store_from_path(&memory_path()).ok()?;
    if store.directives.is_empty() {
        return None;
    }

    let mut lines = vec![
        "\n# Dynamic .kcode memory".to_string(),
        "The user wants instructions or intent containing `.kcode` to persistently improve Kcode behavior. Treat these as user preferences unless they conflict with safety, security, or explicit later instructions.".to_string(),
        "Apply the following remembered directives recursively in future turns:".to_string(),
    ];

    for directive in store.directives.iter().rev().take(MAX_DIRECTIVES_IN_PROMPT) {
        let mut content = directive.content.replace('\n', " ");
        if content.len() > MAX_DIRECTIVE_CHARS {
            content.truncate(MAX_DIRECTIVE_CHARS);
            content.push_str("...");
        }
        lines.push(format!(
            "- [{}] {}",
            directive.created_at.to_rfc3339(),
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
    ] {
        if lower.contains(needle) {
            tags.push(tag.to_string());
        }
    }
    tags.sort();
    tags.dedup();
    tags
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
    }

    #[test]
    fn normalizes_for_deduplication() {
        assert_eq!(normalize("Remember   .KCODE"), normalize("remember .kcode"));
    }
}
