use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LATENT_SCHEMA_VERSION: u32 = 1;
pub const LATENT_DIMENSIONS: usize = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalEvent {
    pub kind: String,
    pub outcome: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_weight")]
    pub weight: f32,
    #[serde(default)]
    pub timestamp_ms: u64,
}

fn default_weight() -> f32 {
    1.0
}

impl OperationalEvent {
    pub fn new(kind: impl Into<String>, outcome: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            outcome: outcome.into(),
            tool: None,
            provider: None,
            tags: Vec::new(),
            weight: 1.0,
            timestamp_ms: now_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentVector {
    pub schema_version: u32,
    pub dimensions: Vec<f32>,
}

impl Default for LatentVector {
    fn default() -> Self {
        Self {
            schema_version: LATENT_SCHEMA_VERSION,
            dimensions: vec![0.0; LATENT_DIMENSIONS],
        }
    }
}

impl LatentVector {
    pub fn magnitude(&self) -> f32 {
        self.dimensions.iter().map(|v| v * v).sum::<f32>().sqrt()
    }

    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        let dot = self
            .dimensions
            .iter()
            .zip(other.dimensions.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>();
        let denom = self.magnitude() * other.magnitude();
        if denom <= f32::EPSILON {
            0.0
        } else {
            (dot / denom).clamp(-1.0, 1.0)
        }
    }

    pub fn drift_from(&self, other: &Self) -> f32 {
        1.0 - self.cosine_similarity(other)
    }

    pub fn normalize(&mut self) {
        let mag = self.magnitude();
        if mag > 1.0 {
            for value in &mut self.dimensions {
                *value /= mag;
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InvariantKind {
    Safety,
    Validation,
    Provenance,
    Reversibility,
    UserIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationalInvariant {
    pub id: String,
    pub kind: InvariantKind,
    pub source_expression: String,
    pub canonical_expression: String,
    pub required_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InvariantTranslation {
    pub invariant_id: String,
    pub matched: bool,
    pub confidence: f32,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemporalMemoryEntry {
    pub event: OperationalEvent,
    pub latent: LatentVector,
    pub provenance: ProvenanceRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub source: String,
    pub captured_at_ms: u64,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfluenceGateDecision {
    pub accepted: bool,
    pub score: f32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AntiSludgeReport {
    pub duplicate_ratio: f32,
    pub low_signal_ratio: f32,
    pub recommendation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatentOperationalState {
    pub schema_version: u32,
    pub vector: LatentVector,
    pub previous_vector: Option<LatentVector>,
    pub events_seen: u64,
    pub invariants: Vec<OperationalInvariant>,
    pub temporal_memory: Vec<TemporalMemoryEntry>,
    pub updated_at_ms: u64,
}

impl Default for LatentOperationalState {
    fn default() -> Self {
        Self {
            schema_version: LATENT_SCHEMA_VERSION,
            vector: LatentVector::default(),
            previous_vector: None,
            events_seen: 0,
            invariants: default_invariants(),
            temporal_memory: Vec::new(),
            updated_at_ms: now_ms(),
        }
    }
}

impl LatentOperationalState {
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let raw = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&raw)?)
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

    pub fn observe(&mut self, event: OperationalEvent) -> InfluenceGateDecision {
        let encoded = encode_event(&event);
        let gate = influence_gate(&encoded, &self.vector, self.events_seen);
        if gate.accepted {
            self.previous_vector = Some(self.vector.clone());
            self.vector =
                recurrent_update(&self.vector, &encoded, 0.82, event.weight.clamp(0.0, 4.0));
            self.events_seen += 1;
            self.updated_at_ms = now_ms();
            self.temporal_memory.push(TemporalMemoryEntry {
                event: event.clone(),
                latent: encoded,
                provenance: ProvenanceRecord {
                    source: "kcode-latent-observe".to_string(),
                    captured_at_ms: now_ms(),
                    evidence: event.tags.clone(),
                },
            });
            if self.temporal_memory.len() > 512 {
                let drain = self.temporal_memory.len() - 512;
                self.temporal_memory.drain(0..drain);
            }
        }
        gate
    }

    pub fn drift(&self) -> f32 {
        self.previous_vector
            .as_ref()
            .map(|previous| self.vector.drift_from(previous))
            .unwrap_or(0.0)
    }

    pub fn anti_sludge_report(&self) -> AntiSludgeReport {
        anti_sludge(&self.temporal_memory)
    }
}

pub fn state_path() -> PathBuf {
    std::env::var_os("KCODE_LATENT_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".kcode").join("latent_operational_state.json")
        })
}

pub fn encode_event(event: &OperationalEvent) -> LatentVector {
    let mut dims = vec![0.0; LATENT_DIMENSIONS];
    let weight = event.weight.clamp(0.0, 4.0);
    apply_token(&mut dims, &event.kind, 0.31 * weight);
    apply_token(&mut dims, &event.outcome, 0.27 * weight);
    if let Some(tool) = &event.tool {
        apply_token(&mut dims, tool, 0.19 * weight);
    }
    if let Some(provider) = &event.provider {
        apply_token(&mut dims, provider, 0.13 * weight);
    }
    for tag in &event.tags {
        apply_token(&mut dims, tag, 0.11 * weight);
    }
    let success = matches!(
        event.outcome.as_str(),
        "success" | "ok" | "passed" | "complete"
    );
    let failure = matches!(
        event.outcome.as_str(),
        "failure" | "error" | "failed" | "blocked"
    );
    if success {
        dims[0] += 0.45 * weight;
        dims[5] += 0.20 * weight;
    }
    if failure {
        dims[1] += 0.55 * weight;
        dims[6] += 0.25 * weight;
    }
    if event.tags.iter().any(|t| t == "test" || t == "validation") {
        dims[2] += 0.35 * weight;
    }
    if event
        .tags
        .iter()
        .any(|t| t == "memory" || t == "provenance")
    {
        dims[3] += 0.30 * weight;
    }
    if event.tags.iter().any(|t| t == "risk" || t == "destructive") {
        dims[4] += 0.50 * weight;
    }
    let mut vector = LatentVector {
        schema_version: LATENT_SCHEMA_VERSION,
        dimensions: dims,
    };
    vector.normalize();
    vector
}

pub fn recurrent_update(
    previous: &LatentVector,
    incoming: &LatentVector,
    decay: f32,
    influence: f32,
) -> LatentVector {
    let decay = decay.clamp(0.0, 1.0);
    let influence = influence.clamp(0.0, 4.0) / 4.0;
    let dims = previous
        .dimensions
        .iter()
        .zip(incoming.dimensions.iter())
        .map(|(p, i)| p * decay + i * (1.0 - decay + influence * 0.18))
        .collect();
    let mut vector = LatentVector {
        schema_version: LATENT_SCHEMA_VERSION,
        dimensions: dims,
    };
    vector.normalize();
    vector
}

pub fn translate_invariants(
    event: &OperationalEvent,
    invariants: &[OperationalInvariant],
) -> Vec<InvariantTranslation> {
    let tags: BTreeSet<_> = event.tags.iter().map(|t| t.as_str()).collect();
    invariants
        .iter()
        .map(|invariant| {
            let required = invariant.required_tags.len().max(1) as f32;
            let matched_count = invariant
                .required_tags
                .iter()
                .filter(|tag| {
                    tags.contains(tag.as_str())
                        || event.kind.contains(tag.as_str())
                        || event.outcome.contains(tag.as_str())
                })
                .count() as f32;
            let confidence = (matched_count / required).clamp(0.0, 1.0);
            InvariantTranslation {
                invariant_id: invariant.id.clone(),
                matched: confidence >= 0.5,
                confidence,
                explanation: format!(
                    "{} -> {} ({matched_count}/{required} signals)",
                    invariant.source_expression, invariant.canonical_expression
                ),
            }
        })
        .collect()
}

pub fn influence_gate(
    incoming: &LatentVector,
    current: &LatentVector,
    events_seen: u64,
) -> InfluenceGateDecision {
    let magnitude = incoming.magnitude();
    if magnitude < 0.05 {
        return InfluenceGateDecision {
            accepted: false,
            score: magnitude,
            reason: "low-signal latent event rejected".to_string(),
        };
    }
    if events_seen > 0 && incoming.cosine_similarity(current) > 0.995 {
        return InfluenceGateDecision {
            accepted: false,
            score: 0.0,
            reason: "near-duplicate latent influence rejected".to_string(),
        };
    }
    InfluenceGateDecision {
        accepted: true,
        score: magnitude.min(1.0),
        reason: "latent influence accepted".to_string(),
    }
}

pub fn remap_vector(vector: &LatentVector, target_schema_version: u32) -> LatentVector {
    if target_schema_version == vector.schema_version {
        return vector.clone();
    }
    let mut remapped = vector.clone();
    remapped.schema_version = target_schema_version;
    remapped.normalize();
    remapped
}

pub fn anti_sludge(memory: &[TemporalMemoryEntry]) -> AntiSludgeReport {
    if memory.is_empty() {
        return AntiSludgeReport {
            duplicate_ratio: 0.0,
            low_signal_ratio: 0.0,
            recommendation: "no temporal memory yet".to_string(),
        };
    }
    let mut seen = BTreeSet::new();
    let mut duplicates = 0usize;
    let mut low_signal = 0usize;
    for entry in memory {
        let key = format!(
            "{}:{}:{:?}",
            entry.event.kind, entry.event.outcome, entry.event.tags
        );
        if !seen.insert(key) {
            duplicates += 1;
        }
        if entry.latent.magnitude() < 0.12 {
            low_signal += 1;
        }
    }
    let duplicate_ratio = duplicates as f32 / memory.len() as f32;
    let low_signal_ratio = low_signal as f32 / memory.len() as f32;
    let recommendation = if duplicate_ratio > 0.35 || low_signal_ratio > 0.35 {
        "compact or down-rank repetitive latent memories".to_string()
    } else {
        "temporal memory signal is healthy".to_string()
    };
    AntiSludgeReport {
        duplicate_ratio,
        low_signal_ratio,
        recommendation,
    }
}

pub fn default_invariants() -> Vec<OperationalInvariant> {
    vec![
        OperationalInvariant {
            id: "validate-before-done".to_string(),
            kind: InvariantKind::Validation,
            source_expression: "claims require validation".to_string(),
            canonical_expression: "run or explain tests before final success".to_string(),
            required_tags: vec!["test".to_string(), "validation".to_string()],
        },
        OperationalInvariant {
            id: "preserve-user-intent".to_string(),
            kind: InvariantKind::UserIntent,
            source_expression: "do what the user asked".to_string(),
            canonical_expression: "retain explicit user constraints across compaction".to_string(),
            required_tags: vec!["intent".to_string(), "memory".to_string()],
        },
        OperationalInvariant {
            id: "avoid-irreversible-actions".to_string(),
            kind: InvariantKind::Reversibility,
            source_expression: "dangerous actions need confirmation".to_string(),
            canonical_expression: "gate destructive operations".to_string(),
            required_tags: vec!["risk".to_string(), "destructive".to_string()],
        },
        OperationalInvariant {
            id: "track-provenance".to_string(),
            kind: InvariantKind::Provenance,
            source_expression: "remember why a fact is trusted".to_string(),
            canonical_expression: "attach source and timestamp to durable memory".to_string(),
            required_tags: vec!["provenance".to_string(), "memory".to_string()],
        },
    ]
}

pub fn render_report(state: &LatentOperationalState) -> String {
    let sludge = state.anti_sludge_report();
    let mut top_dims: BTreeMap<String, f32> = BTreeMap::new();
    for (idx, value) in state.vector.dimensions.iter().enumerate() {
        top_dims.insert(format!("d{idx}"), *value);
    }
    format!(
        "# Latent Operational Recurrence Report\n\n\
Schema version: `{}`\n\
Events seen: `{}`\n\
Vector magnitude: `{:.4}`\n\
Drift from previous vector: `{:.4}`\n\
Temporal memories: `{}`\n\
Invariants: `{}`\n\n\
## Anti-sludge\n\n\
- duplicate ratio: `{:.3}`\n\
- low-signal ratio: `{:.3}`\n\
- recommendation: {}\n\n\
## Current vector\n\n```json\n{}\n```\n\n\
## Purpose\n\n\
This layer gives Kcode an inspectable, deterministic latent state for operational recurrence. It does not replace model reasoning. It summarizes recurring operational conditions, translates invariants into canonical policy signals, tracks temporal provenance, gates low-value influence, and reports drift.\n",
        state.schema_version,
        state.events_seen,
        state.vector.magnitude(),
        state.drift(),
        state.temporal_memory.len(),
        state.invariants.len(),
        sludge.duplicate_ratio,
        sludge.low_signal_ratio,
        sludge.recommendation,
        serde_json::to_string_pretty(&top_dims).unwrap_or_else(|_| "{}".to_string())
    )
}

fn apply_token(dims: &mut [f32], token: &str, weight: f32) {
    let hash = stable_hash(token.as_bytes());
    for (idx, dim) in dims.iter_mut().enumerate() {
        let phase = ((hash.rotate_left((idx as u32) % 31) % 10_000) as f32 / 10_000.0) * 2.0 * PI;
        *dim += phase.sin() * weight;
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_deterministic() {
        let mut event = OperationalEvent::new("tool", "success");
        event.tool = Some("bash".to_string());
        event.tags = vec!["test".to_string(), "validation".to_string()];
        assert_eq!(encode_event(&event), encode_event(&event));
    }

    #[test]
    fn recurrence_updates_and_reports_drift() {
        let mut state = LatentOperationalState::default();
        let mut event = OperationalEvent::new("build", "success");
        event.tags = vec!["test".into()];
        let gate = state.observe(event);
        assert!(gate.accepted);
        assert_eq!(state.events_seen, 1);
        assert!(state.vector.magnitude() > 0.0);
    }

    #[test]
    fn invariant_translation_matches_tags() {
        let mut event = OperationalEvent::new("validation", "passed");
        event.tags = vec!["test".into(), "validation".into()];
        let translations = translate_invariants(&event, &default_invariants());
        assert!(
            translations
                .iter()
                .any(|t| t.invariant_id == "validate-before-done" && t.matched)
        );
    }

    #[test]
    fn influence_gate_rejects_empty_signal() {
        let incoming = LatentVector::default();
        let current = LatentVector::default();
        assert!(!influence_gate(&incoming, &current, 0).accepted);
    }
}
