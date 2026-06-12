use crate::memory::{MemoryCategory, MemoryEntry, MemoryStore, TrustLevel};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvalCase {
    pub name: &'static str,
    pub query: &'static str,
    pub expected_contains: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvalResult {
    pub case: String,
    pub passed: bool,
    pub top_id: Option<String>,
    pub top_content: Option<String>,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEvalReport {
    pub passed: usize,
    pub total: usize,
    pub accuracy: f64,
    pub results: Vec<MemoryEvalResult>,
}

pub fn run_memory_eval() -> MemoryEvalReport {
    let mut store = MemoryStore::default();
    store.add(MemoryEntry::new(
        MemoryCategory::Correction,
        "Correction: build target/release/kcode after source edits so /reload sees a newer binary".to_string()
    ).with_trust(TrustLevel::High));
    store.add(MemoryEntry::new(
        MemoryCategory::Preference,
        "User prefers concise final answers but detailed autonomous implementation while working".to_string()
    ).with_trust(TrustLevel::High));
    store.add(MemoryEntry::new(
        MemoryCategory::Fact,
        "Kcode local sidecar model lives under ~/.kcode/models/gguf and should use gpt-oss/kcode GGUF, not phi3".to_string()
    ).with_trust(TrustLevel::High));
    store.add(MemoryEntry::new(
        MemoryCategory::Fact,
        "Old unrelated fact about deploying with a stale binary".to_string()
    ).with_trust(TrustLevel::Low));

    let cases = [
        MemoryEvalCase { name: "reload_binary", query: "reload no newer binary", expected_contains: "target/release/kcode" },
        MemoryEvalCase { name: "sidecar_model", query: "which model should memory sidecar use", expected_contains: "gguf" },
        MemoryEvalCase { name: "answer_style", query: "how should final answers be", expected_contains: "concise" },
    ];

    let mut results = Vec::new();
    for case in cases {
        let top = store.search_ranked(case.query, 1).into_iter().next();
        let passed = top.map(|m| m.content.to_lowercase().contains(case.expected_contains)).unwrap_or(false);
        let explanation = store.search_explained(case.query, 1).into_iter().next();
        results.push(MemoryEvalResult {
            case: case.name.to_string(),
            passed,
            top_id: top.map(|m| m.id.clone()),
            top_content: top.map(|m| m.content.clone()),
            score: explanation.map(|e| e.score),
        });
    }
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();
    MemoryEvalReport { passed, total, accuracy: if total == 0 { 0.0 } else { passed as f64 / total as f64 }, results }
}
