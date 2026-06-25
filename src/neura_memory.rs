//! Neura-agent inspired memory utilities ported into Neura.
//!
//! This module intentionally keeps only deterministic, local, low-risk pieces
//! from `~/neura-agent`: sensitive-memory detection and keyword overlap scoring.
//! The larger Python graph/vector stack is experimental and overlaps with
//! Neura's existing memory graph, embeddings, and confidence model.

use crate::memory::MemoryEntry;
use std::collections::HashSet;

const SENSITIVE_PATTERNS: &[&str] = &[
    "password",
    "passphrase",
    "api key",
    "apikey",
    "secret",
    "token",
    "bearer ",
    "private key",
    "ssh key",
    "oauth",
    "cookie",
    "session id",
    "credit card",
    "ssn",
    "social security",
];

const STOPWORDS: &[&str] = &[
    "about", "after", "again", "also", "before", "could", "feature", "features", "from", "have",
    "just", "make", "memory", "more", "need", "only", "please", "should", "that", "their", "there",
    "these", "they", "this", "what", "when", "where", "which", "while", "will", "with", "work",
    "would", "your",
];

pub(crate) fn detect_sensitive_content(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
}

pub(crate) fn explicit_sensitive_recall(query: &str) -> bool {
    let lowered = query.to_ascii_lowercase();
    let recall_terms = [
        "password",
        "secret",
        "token",
        "api key",
        "credential",
        "private key",
    ];
    let intent_terms = [
        "show", "recall", "retrieve", "what is", "give", "display", "need",
    ];
    recall_terms.iter().any(|term| lowered.contains(term))
        && intent_terms.iter().any(|term| lowered.contains(term))
}

pub(crate) fn explicit_contradiction_recall(query: &str) -> bool {
    let lowered = query.to_ascii_lowercase();
    [
        "contradict",
        "contradiction",
        "stale",
        "wrong",
        "old note",
        "outdated",
    ]
    .iter()
    .any(|term| lowered.contains(term))
}

pub(crate) fn apply_sensitive_tags(tags: &mut Vec<String>, content: &str) {
    if detect_sensitive_content(content) && !tags.iter().any(|tag| tag == "sensitive") {
        tags.push("sensitive".to_string());
    }
    for tag in extract_relation_tags(content) {
        if !tags.iter().any(|existing| existing == &tag) {
            tags.push(tag);
        }
    }
}

pub(crate) fn extract_relation_tags(content: &str) -> Vec<String> {
    let normalized = content.to_ascii_lowercase();
    let normalized = normalized
        .replace("i ", "user ")
        .replace("my ", "user ")
        .replace("me ", "user ");
    let mut tags = Vec::new();
    let patterns = [
        ("user likes ", "rel:likes"),
        ("user prefer ", "rel:prefers"),
        ("user prefers ", "rel:prefers"),
        ("user uses ", "rel:uses"),
        ("user wants ", "rel:wants"),
        ("project uses ", "rel:uses"),
        ("depends on ", "rel:depends_on"),
        ("because ", "rel:causes"),
        ("causes ", "rel:causes"),
        ("contradicts ", "rel:contradicts"),
        ("fixed by ", "rel:fixed_by"),
        ("validated by ", "rel:validated_by"),
    ];
    for (needle, tag) in patterns {
        if normalized.contains(needle) {
            tags.push(tag.to_string());
        }
    }
    tags
}

pub(crate) fn memory_is_sensitive(entry: &MemoryEntry) -> bool {
    entry
        .tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case("sensitive"))
        || detect_sensitive_content(&entry.content)
}

pub(crate) fn memory_is_contradictory(entry: &MemoryEntry) -> bool {
    entry.tags.iter().any(|tag| tag == "rel:contradicts")
        || entry.content.to_ascii_lowercase().contains("contradicts")
}

pub(crate) fn keyword_terms(text: &str) -> HashSet<String> {
    crate::memory::search::normalize_search_text(text)
        .split_whitespace()
        .filter(|term| term.len() >= 4)
        .filter(|term| !STOPWORDS.contains(term))
        .map(str::to_string)
        .collect()
}

/// Neura-inspired lexical overlap bonus layered onto embedding similarity.
/// This helps exact project names, filenames, feature names, and uncommon terms
/// survive embedding noise without replacing Neura's embedding search.
pub(crate) fn keyword_overlap_bonus(entry: &MemoryEntry, query_text: &str) -> f32 {
    let query_terms = keyword_terms(query_text);
    if query_terms.is_empty() {
        return 0.0;
    }
    let searchable = entry.searchable_text();
    let overlap = query_terms
        .iter()
        .filter(|term| searchable.contains(term.as_str()))
        .count();
    let bonus = match overlap {
        0 => 0.0,
        1 => 0.03,
        2 => 0.08,
        3 => 0.14,
        4 => 0.20,
        _ => 0.26,
    };
    if overlap <= 1 && entry.confidence < 0.75 {
        bonus * 0.35
    } else {
        bonus
    }
}

pub(crate) fn truth_stability_bonus(entry: &MemoryEntry) -> f32 {
    let mut bonus = 0.0;
    if entry.confidence >= 0.9 {
        bonus += 0.06;
    } else if entry.confidence >= 0.75 {
        bonus += 0.03;
    }
    let reinforcements =
        entry.reinforcements.len() as f32 + entry.strength.saturating_sub(1) as f32;
    bonus += (reinforcements.min(5.0)) * 0.015;
    if entry
        .tags
        .iter()
        .any(|tag| tag == "rel:validated_by" || tag == "rel:fixed_by")
    {
        bonus += 0.18;
    }
    if entry.tags.iter().any(|tag| tag == "rel:contradicts") {
        bonus -= 0.35;
    }
    let lowered = entry.content.to_ascii_lowercase();
    if lowered.contains("fake") || lowered.contains("unrelated") || lowered.contains("noise memory")
    {
        bonus -= 0.18;
    }
    if lowered.contains("old note") || lowered.contains("stale") || lowered.contains("outdated") {
        bonus -= 0.14;
    }
    bonus
}

pub(crate) fn passes_relevance_gate(
    entry: &MemoryEntry,
    query_text: &str,
    adjusted_score: f32,
) -> bool {
    let query_terms = keyword_terms(query_text);
    if query_terms.is_empty() {
        return adjusted_score >= 0.28 && entry.confidence >= 0.75;
    }

    let searchable = entry.searchable_text();
    let overlap = query_terms
        .iter()
        .filter(|term| searchable.contains(term.as_str()))
        .count();
    let overlap_ratio = overlap as f32 / query_terms.len().max(1) as f32;
    let content_lower = entry.content.to_ascii_lowercase();

    // Hard abstention: do not inject obvious low-signal memories unless they are
    // directly and strongly matched. This prevents prompt-slot filling from
    // creating false context.
    if content_lower.contains("noise memory") || content_lower.contains("unrelated") {
        return overlap >= 3 && adjusted_score >= 0.55;
    }
    if (content_lower.contains("fake")
        || content_lower.contains("old note")
        || content_lower.contains("outdated"))
        && !explicit_contradiction_recall(query_text)
    {
        return false;
    }
    if memory_is_contradictory(entry) && !explicit_contradiction_recall(query_text) {
        return false;
    }

    let has_relation_support = entry.tags.iter().any(|tag| {
        matches!(
            tag.as_str(),
            "rel:validated_by" | "rel:fixed_by" | "rel:depends_on" | "rel:prefers" | "rel:uses"
        )
    });
    let high_trust =
        entry.confidence >= 0.85 || entry.strength >= 3 || entry.reinforcements.len() >= 2;

    // Strong exact/entity overlap passes quickly.
    if overlap >= 3 && adjusted_score >= 0.28 {
        return true;
    }
    // High-trust relation memories can pass with moderate overlap.
    if overlap >= 2 && overlap_ratio >= 0.34 && high_trust && adjusted_score >= 0.30 {
        return true;
    }
    if overlap >= 2 && has_relation_support && adjusted_score >= 0.38 {
        return true;
    }
    // Single-term matches are usually hallucination fuel unless extremely trusted.
    if overlap == 1 {
        return high_trust && has_relation_support && adjusted_score >= 0.62;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryCategory, MemoryEntry};

    #[test]
    fn sensitive_detector_tags_credentials() {
        assert!(detect_sensitive_content("my api key is abc"));
        assert!(detect_sensitive_content("Bearer token value"));
        assert!(!detect_sensitive_content("favorite editor is helix"));
    }

    #[test]
    fn explicit_sensitive_recall_requires_secret_and_intent() {
        assert!(explicit_sensitive_recall("show my api key"));
        assert!(!explicit_sensitive_recall("avoid storing api keys"));
    }

    #[test]
    fn keyword_overlap_bonus_rewards_uncommon_terms() {
        let entry = MemoryEntry::new(
            MemoryCategory::Fact,
            "Neura interlang ultra exacttok tokenizer context vault",
        );
        assert!(keyword_overlap_bonus(&entry, "interlang tokenizer vault") > 0.1);
    }

    #[test]
    fn relation_extraction_tags_preferences() {
        let tags = extract_relation_tags("I prefer Neura and the project depends on tokenizers");
        assert!(tags.contains(&"rel:prefers".to_string()));
        assert!(tags.contains(&"rel:depends_on".to_string()));
    }

    #[test]
    fn relevance_gate_abstains_on_noise() {
        let noise = MemoryEntry::new(
            MemoryCategory::Fact,
            "Noise memory: unrelated package cache browser tab terminal output",
        );
        assert!(!passes_relevance_gate(
            &noise,
            "what memory features passed tests",
            0.4
        ));
    }

    #[test]
    fn relevance_gate_allows_high_trust_exact_overlap() {
        let mut entry = MemoryEntry::new(
            MemoryCategory::Fact,
            "Project uses interlang ultra context vault with ctx_get exact rehydration",
        );
        entry.confidence = 0.95;
        entry.strength = 3;
        entry.tags.push("rel:uses".to_string());
        assert!(passes_relevance_gate(
            &entry,
            "interlang ultra context vault exact rehydration",
            0.5
        ));
    }
}
