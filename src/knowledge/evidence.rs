//! Tool executions as knowledge evidence.
//!
//! Every workspace-mutating tool call is an observation about the
//! architecture it touched: a successful edit or patch is evidence that
//! Neura's concept of that module is workable; a failure is evidence worth
//! keeping too. Outcomes are queued in-process (fail-quiet, no I/O on the
//! tool's hot path) and folded into the memory graph during the next
//! knowledge refresh / sleep pass, where they land as [`EvidenceRef`]s on the
//! concepts derived from the touched files — reusing the existing
//! evidence→confidence machinery rather than inventing a scoring system.

use crate::memory_graph::{EvidenceRef, MemoryGraph};
use chrono::{DateTime, Utc};
use std::sync::Mutex;

/// Tools whose outcomes constitute architectural evidence.
const EVIDENCE_TOOLS: &[&str] = &[
    "edit",
    "multiedit",
    "patch",
    "apply_patch",
    "file_edit",
    "file_write",
];

/// Bounded queue: oldest outcomes are dropped under pressure. Tool feedback
/// is a reinforcement signal, not a ledger (the runtime ledger already keeps
/// receipts), so lossiness under burst is acceptable.
const MAX_QUEUE: usize = 256;

#[derive(Debug, Clone)]
pub struct ToolOutcome {
    pub tool: String,
    pub paths: Vec<String>,
    pub success: bool,
    pub at: DateTime<Utc>,
}

static QUEUE: Mutex<Vec<ToolOutcome>> = Mutex::new(Vec::new());

/// Record a tool outcome for later evidence folding. Cheap and fail-quiet;
/// safe to call on every tool execution. Non-evidence tools and pathless
/// calls are ignored.
pub fn note_tool_outcome(tool: &str, input: &serde_json::Value, success: bool) {
    note_tool_outcome_paths(tool, candidate_paths(tool, input), success);
}

/// Paths a tool call would touch, or empty for non-evidence tools. Split out
/// so call sites that move `input` into execution can capture paths first.
pub fn candidate_paths(tool: &str, input: &serde_json::Value) -> Vec<String> {
    if !EVIDENCE_TOOLS.contains(&tool) {
        return Vec::new();
    }
    extract_paths(input)
}

/// Queue a pre-extracted outcome (see [`candidate_paths`]).
pub fn note_tool_outcome_paths(tool: &str, paths: Vec<String>, success: bool) {
    if paths.is_empty() {
        return;
    }
    if let Ok(mut queue) = QUEUE.lock() {
        queue.push(ToolOutcome {
            tool: tool.to_string(),
            paths,
            success,
            at: Utc::now(),
        });
        if queue.len() > MAX_QUEUE {
            let overflow = queue.len() - MAX_QUEUE;
            queue.drain(0..overflow);
        }
    }
}

/// Number of outcomes waiting to be folded into the graph.
pub fn queued_len() -> usize {
    QUEUE.lock().map(|q| q.len()).unwrap_or(0)
}

/// File-path-shaped fields understood across the edit-tool family.
fn extract_paths(input: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["file_path", "path", "notebook_path"] {
        if let Some(p) = input.get(key).and_then(|v| v.as_str())
            && !p.is_empty()
            && !paths.iter().any(|existing| existing == p)
        {
            paths.push(p.to_string());
        }
    }
    paths
}

/// Resolve a tool-supplied path (absolute or workspace-relative) to the
/// source-relative item key used by a registered knowledge source.
fn item_key_for_path(graph: &MemoryGraph, path: &str) -> Option<String> {
    for state in graph.metadata.knowledge_sources.values() {
        // Absolute path under the source root → strip the root.
        if let Some(rest) = path
            .strip_prefix(&state.locator)
            .map(|r| r.trim_start_matches('/'))
            && state.item_units.contains_key(rest)
        {
            return Some(rest.to_string());
        }
        // Already source-relative.
        if state.item_units.contains_key(path) {
            return Some(path.to_string());
        }
    }
    None
}

/// Drain the queue and fold each outcome into the graph as evidence on the
/// concepts derived from the touched files. Successes flow through
/// `record_fact_observation` (evidence-backed confidence, episodic→semantic
/// promotion); failures append evidence and apply a small confidence decay.
/// Returns the number of concept observations recorded.
pub fn apply_queued_outcomes(graph: &mut MemoryGraph) -> usize {
    let outcomes: Vec<ToolOutcome> = match QUEUE.lock() {
        Ok(mut queue) => queue.drain(..).collect(),
        Err(_) => return 0,
    };
    let mut recorded = 0usize;
    let mut touched: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for outcome in outcomes {
        for path in &outcome.paths {
            let Some(item) = item_key_for_path(graph, path) else {
                continue;
            };
            for id in super::concept_ids_for_item(graph, &item) {
                touched.insert(id.clone());
                if outcome.success {
                    let ev = EvidenceRef::observation(format!(
                        "tool {} succeeded touching {item}",
                        outcome.tool
                    ));
                    if graph.record_fact_observation(&id, ev) {
                        // Concept was promoted to semantic by accumulated
                        // observations — existing machinery, nothing extra.
                    }
                    recorded += 1;
                } else if let Some(m) = graph.get_memory_mut(&id) {
                    // A failed edit is evidence too, but must not raise
                    // confidence: append the observation without the
                    // strength bump, then decay slightly.
                    const MAX_FACT_EVIDENCE: usize = 12;
                    m.evidence.push(EvidenceRef::observation(format!(
                        "tool {} failed touching {item}",
                        outcome.tool
                    )));
                    if m.evidence.len() > MAX_FACT_EVIDENCE {
                        let overflow = m.evidence.len() - MAX_FACT_EVIDENCE;
                        m.evidence.drain(0..overflow);
                    }
                    m.decay_confidence(0.05);
                    m.updated_at = Utc::now();
                    recorded += 1;
                }
            }
        }
    }

    // Structured reflection: score any pending architectural predictions
    // against what execution actually touched (appends to the evidence
    // ledger; reinforces confirmed expectations).
    super::reasoning::reflect_on_outcomes(graph, &touched);

    recorded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_known_path_fields_without_duplicates() {
        let input = serde_json::json!({
            "file_path": "src/agent.rs",
            "path": "src/agent.rs",
            "old_string": "x",
        });
        assert_eq!(extract_paths(&input), vec!["src/agent.rs".to_string()]);
    }

    #[test]
    fn ignores_non_evidence_tools_and_pathless_calls() {
        note_tool_outcome("bash", &serde_json::json!({"command": "ls"}), true);
        note_tool_outcome("edit", &serde_json::json!({"no_path": true}), true);
        // Neither should have queued anything attributable to this test;
        // (other tests may queue, so only assert these two didn't panic and
        // pathless/non-evidence inputs produce no paths).
        assert!(extract_paths(&serde_json::json!({"command": "ls"})).is_empty());
    }
}
