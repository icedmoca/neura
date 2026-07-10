//! Local semantic memory packing utilities.
//!
//! This module is intentionally small and dependency-free. It mirrors the
//! sidecar-facing part of a Subtext-style memory pipeline: normalize memory
//! snippets, derive lightweight semantic hints, estimate token cost, dedupe
//! near-identical memories, then greedily pack the highest-value memories into
//! a fixed local context budget.

use std::collections::{HashMap, HashSet};

/// A memory snippet prepared for inclusion in local context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMemoryCandidate {
    pub text: String,
    /// Higher values win when the token budget is tight.
    pub priority: i32,
    /// Optional semantic hints supplied by retrieval. Empty hints are derived
    /// from the text during packing.
    pub semantic_hints: Vec<String>,
}

impl LocalMemoryCandidate {
    pub fn new(text: impl Into<String>, priority: i32) -> Self {
        Self {
            text: text.into(),
            priority,
            semantic_hints: Vec::new(),
        }
    }

    pub fn with_hints(
        text: impl Into<String>,
        priority: i32,
        hints: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            text: text.into(),
            priority,
            semantic_hints: hints.into_iter().map(Into::into).collect(),
        }
    }
}

/// A packed memory with its local token estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedLocalMemory {
    pub text: String,
    pub priority: i32,
    pub token_estimate: usize,
    pub semantic_hints: Vec<String>,
}

/// Estimate tokens for sidecar/local model context budgeting.
///
/// Prefers the shared exact tokenizer used by the interlang/ctx-vault pipeline
/// so budgeting is consistent across Neura. When the tokenizer model is not
/// available it falls back to the deterministic heuristic below: ASCII word-ish
/// runs count as ~4 chars/token, punctuation is a token, and non-ASCII chars are
/// grouped more tightly, keeping packing stable regardless of the local model.
pub fn estimate_local_tokens(text: &str) -> usize {
    if let Some(exact) = crate::interlang::exact_token_count(text) {
        return exact;
    }
    heuristic_local_tokens(text)
}

/// Deterministic, dependency-free token estimate used when no exact tokenizer
/// is loaded. Exposed for tests so the heuristic stays stable.
pub fn heuristic_local_tokens(text: &str) -> usize {
    let mut tokens = 0usize;
    let mut current_run_chars = 0usize;
    let mut current_ascii = true;

    let flush = |tokens: &mut usize, chars: &mut usize, ascii: bool| {
        if *chars == 0 {
            return;
        }
        let divisor = if ascii { 4 } else { 2 };
        *tokens += (*chars).div_ceil(divisor).max(1);
        *chars = 0;
    };

    for ch in text.chars() {
        if ch.is_whitespace() {
            flush(&mut tokens, &mut current_run_chars, current_ascii);
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            if !current_ascii {
                flush(&mut tokens, &mut current_run_chars, current_ascii);
            }
            current_ascii = true;
            current_run_chars += 1;
        } else if ch.is_alphanumeric() {
            if current_ascii && current_run_chars > 0 {
                flush(&mut tokens, &mut current_run_chars, current_ascii);
            }
            current_ascii = false;
            current_run_chars += 1;
        } else {
            flush(&mut tokens, &mut current_run_chars, current_ascii);
            tokens += 1;
        }
    }
    flush(&mut tokens, &mut current_run_chars, current_ascii);
    tokens
}

/// Derive stable semantic hints from text.
pub fn derive_semantic_hints(text: &str) -> Vec<String> {
    let stop: HashSet<&str> = [
        "about", "after", "again", "also", "and", "are", "because", "been", "but", "can", "could",
        "for", "from", "have", "how", "into", "not", "that", "the", "then", "this", "what", "when",
        "where", "with", "would", "your",
    ]
    .into_iter()
    .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut current = String::new();
    for ch in text.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '_' || ch == '-' {
            current.push(ch);
        } else if !current.is_empty() {
            if current.len() >= 3 && !stop.contains(current.as_str()) {
                *counts.entry(std::mem::take(&mut current)).or_default() += 1;
            } else {
                current.clear();
            }
        }
    }
    if current.len() >= 3 && !stop.contains(current.as_str()) {
        *counts.entry(current).or_default() += 1;
    }

    let mut terms: Vec<_> = counts.into_iter().collect();
    terms.sort_by(|(a_term, a_count), (b_term, b_count)| {
        b_count.cmp(a_count).then_with(|| a_term.cmp(b_term))
    });
    terms.into_iter().take(8).map(|(term, _)| term).collect()
}

fn normalize_for_dedupe(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Dedupe and pack memories into a token budget.
pub fn pack_local_memories(
    candidates: impl IntoIterator<Item = LocalMemoryCandidate>,
    max_tokens: usize,
) -> Vec<PackedLocalMemory> {
    let mut by_key: HashMap<String, LocalMemoryCandidate> = HashMap::new();
    for candidate in candidates {
        let text = candidate.text.trim();
        if text.is_empty() {
            continue;
        }
        let key = normalize_for_dedupe(text);
        match by_key.get(&key) {
            Some(existing) if existing.priority >= candidate.priority => {}
            _ => {
                by_key.insert(
                    key,
                    LocalMemoryCandidate {
                        text: text.to_string(),
                        ..candidate
                    },
                );
            }
        }
    }

    let mut prepared: Vec<PackedLocalMemory> = by_key
        .into_values()
        .map(|candidate| {
            let mut hints = candidate.semantic_hints;
            if hints.is_empty() {
                hints = derive_semantic_hints(&candidate.text);
            }
            hints.sort();
            hints.dedup();
            PackedLocalMemory {
                token_estimate: estimate_local_tokens(&candidate.text),
                text: candidate.text,
                priority: candidate.priority,
                semantic_hints: hints,
            }
        })
        .collect();

    prepared.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.token_estimate.cmp(&b.token_estimate))
            .then_with(|| a.text.cmp(&b.text))
    });

    let mut used = 0usize;
    let mut packed = Vec::new();
    for memory in prepared {
        if memory.token_estimate == 0 {
            continue;
        }
        if used + memory.token_estimate <= max_tokens {
            used += memory.token_estimate;
            packed.push(memory);
        }
    }
    packed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_ascii_and_punctuation_tokens() {
        // Assert against the deterministic heuristic so the test does not depend
        // on whether the shared exact tokenizer model is present on disk.
        assert_eq!(heuristic_local_tokens("hello"), 2);
        assert_eq!(heuristic_local_tokens("hello, world!"), 6);
        assert!(heuristic_local_tokens("memory tokenization sidecar") >= 6);
    }

    #[test]
    fn derives_semantic_hints_without_stop_words() {
        let hints =
            derive_semantic_hints("The local memory sidecar packs memory with token budgets");
        assert!(hints.contains(&"memory".to_string()));
        assert!(hints.contains(&"sidecar".to_string()));
        assert!(!hints.contains(&"the".to_string()));
    }

    #[test]
    fn packs_best_deduped_memories_within_budget() {
        let packed = pack_local_memories(
            [
                LocalMemoryCandidate::new("Remember the user prefers concise answers", 10),
                LocalMemoryCandidate::new("Remember   the user prefers concise answers", 2),
                LocalMemoryCandidate::new("Low priority detail that is too verbose to fit", 1),
            ],
            16,
        );
        assert_eq!(packed.len(), 1);
        assert_eq!(packed[0].priority, 10);
        assert!(packed[0].semantic_hints.contains(&"concise".to_string()));
    }
}
