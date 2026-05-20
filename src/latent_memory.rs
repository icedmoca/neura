use crate::latent_learning::LearningStep;
use crate::latent_operational_recurrence::{LatentVector, OperationalEvent};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

pub const LATENT_MEMORY_SCHEMA_VERSION: u32 = 1;
const DEFAULT_DRIFT_THRESHOLD: f32 = 0.18;
const MAX_MEMORIES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LatentMemoryKind {
    StableAttractor,
    NoisePattern,
    ValidationDoctrine,
    DriftSynthesis,
    OperationalLesson,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentMemoryEntry {
    pub id: String,
    pub kind: LatentMemoryKind,
    pub summary: String,
    pub ctx_block: String,
    pub vector: LatentVector,
    pub tags: Vec<String>,
    pub confidence: f32,
    #[serde(default)]
    pub usefulness_score: f32,
    #[serde(default)]
    pub influence_count: u64,
    #[serde(default)]
    pub positive_outcomes: u64,
    #[serde(default)]
    pub negative_outcomes: u64,
    #[serde(default)]
    pub drift_reduction_total: f32,
    pub support: u64,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentMemoryBank {
    pub schema_version: u32,
    pub drift_threshold: f32,
    pub entries: Vec<LatentMemoryEntry>,
    pub synthesis_records: Vec<LatentMemoryEntry>,
    #[serde(default)]
    pub attributions: Vec<LatentMemoryAttribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentMemoryAttribution {
    pub memory_id: Option<String>,
    pub action: LatentMemoryAction,
    pub decision_reason: String,
    pub event_kind: String,
    pub outcome: String,
    pub outcome_score: f32,
    pub drift_before: f32,
    pub drift_after: f32,
    pub drift_delta: f32,
    pub improved: bool,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentMemoryUsefulnessReport {
    pub total_attributions: usize,
    pub improved_count: usize,
    pub worsened_count: usize,
    pub repeated_failures_reduced: bool,
    pub retrieval_improved: bool,
    pub bad_provider_choices_avoided: bool,
    pub drift_reduced: bool,
    pub mean_outcome_score: f32,
    pub mean_drift_delta: f32,
    pub top_memories: Vec<(String, f32, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentMemoryDecision {
    pub action: LatentMemoryAction,
    pub rank: f32,
    pub drift: f32,
    pub matched_memory_id: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LatentMemoryAction {
    SuppressDuplicate,
    DownrankNoise,
    ApplyMeaningfulUpdate,
    SynthesizeUsefulDrift,
    AnchorToAttractor,
}

impl Default for LatentMemoryBank {
    fn default() -> Self {
        Self {
            schema_version: LATENT_MEMORY_SCHEMA_VERSION,
            drift_threshold: DEFAULT_DRIFT_THRESHOLD,
            entries: Vec::new(),
            synthesis_records: Vec::new(),
            attributions: Vec::new(),
        }
    }
}

impl LatentMemoryBank {
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn rank_event(
        &self,
        event: &OperationalEvent,
        vector: &LatentVector,
        current: &LatentVector,
    ) -> LatentMemoryDecision {
        let best = self
            .entries
            .iter()
            .map(|entry| (entry, entry.vector.cosine_similarity(vector)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
        let drift = vector.drift_from(current);
        if let Some((entry, similarity)) = best {
            if similarity > 0.985 {
                return LatentMemoryDecision {
                    action: LatentMemoryAction::SuppressDuplicate,
                    rank: 0.0,
                    drift,
                    matched_memory_id: Some(entry.id.clone()),
                    reason: "ctx-style latent memory recognized duplicate operational pattern"
                        .into(),
                };
            }
            if matches!(entry.kind, LatentMemoryKind::NoisePattern) && similarity > 0.82 {
                return LatentMemoryDecision {
                    action: LatentMemoryAction::DownrankNoise,
                    rank: 0.15,
                    drift,
                    matched_memory_id: Some(entry.id.clone()),
                    reason:
                        "latent memory classified this pattern as high-volume low-novelty noise"
                            .into(),
                };
            }
            if drift > self.drift_threshold && similarity < 0.55 && is_useful_drift(event) {
                return LatentMemoryDecision {
                    action: LatentMemoryAction::SynthesizeUsefulDrift,
                    rank: 0.85,
                    drift,
                    matched_memory_id: Some(entry.id.clone()),
                    reason:
                        "drift exceeds threshold but carries useful validation/provenance signal"
                            .into(),
                };
            }
            if drift > self.drift_threshold && similarity < 0.35 {
                return LatentMemoryDecision { action: LatentMemoryAction::AnchorToAttractor, rank: 0.35, drift, matched_memory_id: Some(entry.id.clone()), reason: "drift exceeds threshold without enough support; anchor toward latent memory".into() };
            }
        }
        let rank = event_rank(event, drift);
        LatentMemoryDecision {
            action: LatentMemoryAction::ApplyMeaningfulUpdate,
            rank,
            drift,
            matched_memory_id: None,
            reason: "meaningful latent update admitted".into(),
        }
    }

    pub fn absorb_learning_step(&mut self, step: &LearningStep) -> Option<LatentMemoryEntry> {
        if step.immune.triggered {
            return None;
        }
        let kind = classify_memory_kind(step);
        let id = memory_id(&step.sample.event, &kind);
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.id == id) {
            existing.support += 1;
            existing.confidence = (existing.confidence * 0.9
                + step.sample.score.scalar().max(0.0) * 0.1)
                .clamp(0.0, 1.0);
            existing.last_seen_ms = crate::latent_operational_recurrence::now_ms();
            existing.ctx_block = render_ctx_block(existing);
            return Some(existing.clone());
        }
        let mut entry = LatentMemoryEntry {
            id,
            kind,
            summary: summarize_event(&step.sample.event),
            ctx_block: String::new(),
            vector: step.sample.encoded.clone(),
            tags: step.sample.event.tags.clone(),
            confidence: step.sample.score.scalar().max(0.0),
            usefulness_score: step.sample.score.scalar().max(0.0),
            influence_count: 0,
            positive_outcomes: 0,
            negative_outcomes: 0,
            drift_reduction_total: 0.0,
            support: 1,
            last_seen_ms: crate::latent_operational_recurrence::now_ms(),
        };
        entry.ctx_block = render_ctx_block(&entry);
        if matches!(entry.kind, LatentMemoryKind::DriftSynthesis) {
            self.synthesis_records.push(entry.clone());
            trim(&mut self.synthesis_records, 128);
        }
        self.entries.push(entry.clone());
        self.entries.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(Ordering::Equal)
        });
        trim(&mut self.entries, MAX_MEMORIES);
        Some(entry)
    }

    pub fn ctx_blocks(&self, limit: usize) -> Vec<String> {
        self.entries
            .iter()
            .take(limit)
            .map(|entry| entry.ctx_block.clone())
            .collect()
    }

    pub fn record_attribution(
        &mut self,
        decision: &LatentMemoryDecision,
        event: &OperationalEvent,
        outcome_score: f32,
        drift_after: f32,
    ) -> LatentMemoryAttribution {
        let drift_delta = decision.drift - drift_after;
        let improved = outcome_score > 0.0 || drift_delta > 0.0;
        let attribution = LatentMemoryAttribution {
            memory_id: decision.matched_memory_id.clone(),
            action: decision.action.clone(),
            decision_reason: decision.reason.clone(),
            event_kind: event.kind.clone(),
            outcome: event.outcome.clone(),
            outcome_score,
            drift_before: decision.drift,
            drift_after,
            drift_delta,
            improved,
            timestamp_ms: crate::latent_operational_recurrence::now_ms(),
        };
        if let Some(id) = &decision.matched_memory_id {
            if let Some(entry) = self.entries.iter_mut().find(|entry| &entry.id == id) {
                entry.influence_count += 1;
                if improved {
                    entry.positive_outcomes += 1;
                } else {
                    entry.negative_outcomes += 1;
                }
                entry.drift_reduction_total += drift_delta.max(0.0);
                entry.usefulness_score = (entry.usefulness_score * 0.85
                    + outcome_score.max(0.0) * 0.10
                    + drift_delta.max(0.0).min(1.0) * 0.05)
                    .clamp(0.0, 1.0);
                entry.ctx_block = render_ctx_block(entry);
            }
        }
        self.attributions.push(attribution.clone());
        trim(&mut self.attributions, 512);
        attribution
    }

    pub fn usefulness_report(&self) -> LatentMemoryUsefulnessReport {
        let total = self.attributions.len();
        let improved_count = self.attributions.iter().filter(|a| a.improved).count();
        let worsened_count = total.saturating_sub(improved_count);
        let mean_outcome_score = if total == 0 {
            0.0
        } else {
            self.attributions
                .iter()
                .map(|a| a.outcome_score)
                .sum::<f32>()
                / total as f32
        };
        let mean_drift_delta = if total == 0 {
            0.0
        } else {
            self.attributions.iter().map(|a| a.drift_delta).sum::<f32>() / total as f32
        };
        let mut top = self
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.usefulness_score, e.influence_count))
            .collect::<Vec<_>>();
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
        top.truncate(12);
        LatentMemoryUsefulnessReport {
            total_attributions: total,
            improved_count,
            worsened_count,
            repeated_failures_reduced: self.attributions.iter().any(|a| {
                matches!(
                    a.action,
                    LatentMemoryAction::SuppressDuplicate | LatentMemoryAction::DownrankNoise
                ) && a.improved
            }),
            retrieval_improved: self
                .entries
                .iter()
                .any(|e| e.influence_count > 0 && e.usefulness_score > 0.5),
            bad_provider_choices_avoided: self
                .attributions
                .iter()
                .any(|a| a.event_kind.to_ascii_lowercase().contains("provider") && a.improved),
            drift_reduced: mean_drift_delta > 0.0,
            mean_outcome_score,
            mean_drift_delta,
            top_memories: top,
        }
    }

    pub fn rehydration_blocks(&self, limit: usize, min_confidence: f32) -> Vec<String> {
        let mut entries = self
            .entries
            .iter()
            .filter(|entry| entry.confidence >= min_confidence)
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            let ascore = a.confidence * 0.7 + (a.support as f32).ln_1p() * 0.3;
            let bscore = b.confidence * 0.7 + (b.support as f32).ln_1p() * 0.3;
            bscore.partial_cmp(&ascore).unwrap_or(Ordering::Equal)
        });
        entries
            .into_iter()
            .take(limit)
            .map(|entry| entry.ctx_block.clone())
            .collect()
    }
}

pub fn latent_memory_path() -> PathBuf {
    std::env::var_os("KCODE_LATENT_MEMORY_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".kcode").join("latent_memory_bank.json")
        })
}

pub fn render_memory_report(bank: &LatentMemoryBank) -> String {
    let ctx = bank.rehydration_blocks(12, 0.05).join("\n");
    format!(
        "# Latent Memory Bank Report\n\nEntries: `{}`\nSynthesis records: `{}`\nAttributions: `{}`\nDrift threshold: `{:.3}`\n\n## Top ctx-style blocks\n\n```text\n{}\n```\n",
        bank.entries.len(),
        bank.synthesis_records.len(),
        bank.attributions.len(),
        bank.drift_threshold,
        ctx
    )
}

fn render_ctx_block(entry: &LatentMemoryEntry) -> String {
    format!(
        "<ctx k=\"latent-memory\" id=\"{}\" kind=\"{:?}\" confidence=\"{:.3}\" support=\"{}\" tags=\"{}\">{}</ctx>",
        entry.id,
        entry.kind,
        entry.confidence,
        entry.support,
        entry.tags.join(","),
        entry.summary
    )
}

fn classify_memory_kind(step: &LearningStep) -> LatentMemoryKind {
    let tags = &step.sample.event.tags;
    if tags.iter().any(|t| t == "token" || t == "live-fabric") && step.sample.score.novelty < 0.2 {
        LatentMemoryKind::NoisePattern
    } else if tags.iter().any(|t| t == "validation" || t == "test") {
        LatentMemoryKind::ValidationDoctrine
    } else if step.sample.score.novelty > 0.45 {
        LatentMemoryKind::DriftSynthesis
    } else {
        LatentMemoryKind::OperationalLesson
    }
}

fn is_useful_drift(event: &OperationalEvent) -> bool {
    event.tags.iter().any(|t| {
        matches!(
            t.as_str(),
            "validation" | "test" | "provenance" | "memory" | "doctrine"
        )
    })
}
fn event_rank(event: &OperationalEvent, drift: f32) -> f32 {
    let base = if is_useful_drift(event) { 0.75 } else { 0.45 };
    (base + drift.min(0.25)).clamp(0.0, 1.0)
}
fn summarize_event(event: &OperationalEvent) -> String {
    format!(
        "{} -> {} via {:?} [{}]",
        event.kind,
        event.outcome,
        event.tool,
        event.tags.join(",")
    )
}
fn memory_id(event: &OperationalEvent, kind: &LatentMemoryKind) -> String {
    format!(
        "{:?}:{}:{}:{}",
        kind,
        sanitize(&event.kind),
        sanitize(&event.outcome),
        sanitize(&event.tags.join("-"))
    )
}
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
fn trim<T>(items: &mut Vec<T>, max: usize) {
    if items.len() > max {
        items.drain(0..items.len() - max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latent_learning::LatentLearningState;
    use crate::latent_operational_recurrence::{
        LatentOperationalState, OperationalEvent, encode_event,
    };

    #[test]
    fn creates_ctx_style_latent_memory() {
        let recurrence = LatentOperationalState::default();
        let mut learning = LatentLearningState::default();
        let mut event = OperationalEvent::new("build", "success");
        event.tags = vec!["test".into(), "validation".into()];
        let step = learning.learn(&recurrence, event);
        let mut bank = LatentMemoryBank::default();
        let entry = bank.absorb_learning_step(&step).unwrap();
        assert!(entry.ctx_block.contains("<ctx k=\"latent-memory\""));
        assert_eq!(bank.entries.len(), 1);
    }

    #[test]
    fn duplicate_memory_suppresses_repeated_vector() {
        let mut bank = LatentMemoryBank::default();
        let event = OperationalEvent::new("live::TokenUsage", "observed");
        let vector = encode_event(&event);
        bank.entries.push(LatentMemoryEntry {
            id: "n".into(),
            kind: LatentMemoryKind::NoisePattern,
            summary: "noise".into(),
            ctx_block: String::new(),
            vector: vector.clone(),
            tags: vec!["token".into()],
            confidence: 0.9,
            usefulness_score: 0.9,
            influence_count: 0,
            positive_outcomes: 0,
            negative_outcomes: 0,
            drift_reduction_total: 0.0,
            support: 9,
            last_seen_ms: 0,
        });
        let decision = bank.rank_event(&event, &vector, &LatentVector::default());
        assert_eq!(decision.action, LatentMemoryAction::SuppressDuplicate);
    }
}
