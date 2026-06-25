use crate::latent_operational_recurrence::{
    LatentOperationalState, LatentVector, OperationalEvent, anti_sludge, encode_event,
    translate_invariants,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const LEARNING_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentOutcomeScore {
    pub success: f32,
    pub validation: f32,
    pub safety: f32,
    pub efficiency: f32,
    pub novelty: f32,
}

impl LatentOutcomeScore {
    pub fn scalar(&self) -> f32 {
        (self.success * 0.32
            + self.validation * 0.26
            + self.safety * 0.22
            + self.efficiency * 0.10
            + self.novelty * 0.10)
            .clamp(-1.0, 1.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentLearningSample {
    pub event: OperationalEvent,
    pub encoded: LatentVector,
    pub score: LatentOutcomeScore,
    pub doctrine_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnedLatentVector {
    pub id: String,
    pub vector: LatentVector,
    pub support: u64,
    pub confidence: f32,
    pub doctrine_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentAttractor {
    pub id: String,
    pub centroid: LatentVector,
    pub support: u64,
    pub mean_score: f32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CounterfactualProbe {
    pub event_kind: String,
    pub baseline_score: f32,
    pub alternate_tags: Vec<String>,
    pub projected_score: f32,
    pub delta: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DoctrineBinding {
    pub id: String,
    pub invariant_id: String,
    pub strength: f32,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImmuneResponse {
    pub triggered: bool,
    pub reason: String,
    pub severity: f32,
    pub blocked_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentTopologyEdge {
    pub from: String,
    pub to: String,
    pub weight: f32,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConvergenceMetrics {
    pub attractor_count: usize,
    pub mean_confidence: f32,
    pub drift: f32,
    pub sludge_risk: f32,
    pub convergence_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentRemapPlan {
    pub from_schema: u32,
    pub to_schema: u32,
    pub preserve_dimensions: usize,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LatentLearningState {
    pub schema_version: u32,
    pub samples_seen: u64,
    pub learned_vectors: Vec<LearnedLatentVector>,
    pub attractors: Vec<LatentAttractor>,
    pub doctrine_bindings: Vec<DoctrineBinding>,
    pub immune_history: Vec<ImmuneResponse>,
    pub topology: Vec<LatentTopologyEdge>,
    pub last_convergence: Option<ConvergenceMetrics>,
}

impl Default for LatentLearningState {
    fn default() -> Self {
        Self {
            schema_version: LEARNING_SCHEMA_VERSION,
            samples_seen: 0,
            learned_vectors: Vec::new(),
            attractors: Vec::new(),
            doctrine_bindings: Vec::new(),
            immune_history: Vec::new(),
            topology: Vec::new(),
            last_convergence: None,
        }
    }
}

impl LatentLearningState {
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

    pub fn learn(
        &mut self,
        recurrence: &LatentOperationalState,
        event: OperationalEvent,
    ) -> LearningStep {
        let sample = sample_from_event(recurrence, event);
        let immune = immune_response(&sample);
        if immune.triggered {
            self.immune_history.push(immune.clone());
            trim(&mut self.immune_history, 128);
        } else {
            self.samples_seen += 1;
            upsert_learned_vector(self, &sample);
            self.attractors = derive_attractors(&self.learned_vectors);
            self.doctrine_bindings = derive_doctrine_bindings(recurrence, &sample);
            self.topology = derive_topology(&self.attractors, &self.doctrine_bindings);
            self.last_convergence = Some(convergence_metrics(self, recurrence));
        }
        LearningStep {
            sample,
            immune,
            convergence: self.last_convergence.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearningStep {
    pub sample: LatentLearningSample,
    pub immune: ImmuneResponse,
    pub convergence: Option<ConvergenceMetrics>,
}

pub fn learning_state_path() -> PathBuf {
    std::env::var_os("NEURA_LATENT_LEARNING_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".neura").join("latent_learning_state.json")
        })
}

pub fn sample_from_event(
    recurrence: &LatentOperationalState,
    event: OperationalEvent,
) -> LatentLearningSample {
    let encoded = encode_event(&event);
    let translations = translate_invariants(&event, &recurrence.invariants);
    let validation = translations
        .iter()
        .map(|t| t.confidence)
        .fold(0.0_f32, f32::max);
    let tags: BTreeSet<String> = event.tags.iter().cloned().collect();
    let success = match event.outcome.as_str() {
        "success" | "ok" | "passed" | "complete" => 1.0,
        "failure" | "error" | "failed" | "blocked" => -0.8,
        _ => 0.0,
    };
    let safety = if tags.contains("risk") || tags.contains("destructive") {
        -0.7
    } else {
        0.6
    };
    let efficiency = if tags.contains("slow") { -0.3 } else { 0.3 };
    let novelty = if recurrence.vector.cosine_similarity(&encoded) < 0.72 {
        0.5
    } else {
        0.1
    };
    LatentLearningSample {
        event,
        encoded,
        score: LatentOutcomeScore {
            success,
            validation,
            safety,
            efficiency,
            novelty,
        },
        doctrine_tags: translations
            .into_iter()
            .filter(|t| t.matched)
            .map(|t| t.invariant_id)
            .collect(),
    }
}

pub fn immune_response(sample: &LatentLearningSample) -> ImmuneResponse {
    let tags: BTreeSet<&str> = sample.event.tags.iter().map(String::as_str).collect();
    if tags.contains("destructive") && !tags.contains("confirmed") {
        return ImmuneResponse {
            triggered: true,
            reason: "destructive latent sample lacks confirmation".into(),
            severity: 0.95,
            blocked_tags: vec!["destructive".into()],
        };
    }
    if sample.score.scalar() < -0.55 {
        return ImmuneResponse {
            triggered: true,
            reason: "strongly negative operational outcome".into(),
            severity: sample.score.scalar().abs(),
            blocked_tags: sample.event.tags.clone(),
        };
    }
    ImmuneResponse {
        triggered: false,
        reason: "sample accepted for latent learning".into(),
        severity: 0.0,
        blocked_tags: Vec::new(),
    }
}

pub fn counterfactual_probe(
    recurrence: &LatentOperationalState,
    event: &OperationalEvent,
    alternate_tags: Vec<String>,
) -> CounterfactualProbe {
    let baseline = sample_from_event(recurrence, event.clone()).score.scalar();
    let mut alternate = event.clone();
    alternate.tags = alternate_tags.clone();
    let projected = sample_from_event(recurrence, alternate).score.scalar();
    CounterfactualProbe {
        event_kind: event.kind.clone(),
        baseline_score: baseline,
        alternate_tags,
        projected_score: projected,
        delta: projected - baseline,
    }
}

pub fn remap_plan(vector: &LatentVector, target_schema: u32) -> LatentRemapPlan {
    LatentRemapPlan {
        from_schema: vector.schema_version,
        to_schema: target_schema,
        preserve_dimensions: vector.dimensions.len(),
        rationale: "schema-only remap preserves deterministic latent coordinates for v1".into(),
    }
}

pub fn convergence_metrics(
    state: &LatentLearningState,
    recurrence: &LatentOperationalState,
) -> ConvergenceMetrics {
    let confidence_sum: f32 = state.learned_vectors.iter().map(|v| v.confidence).sum();
    let mean_confidence = if state.learned_vectors.is_empty() {
        0.0
    } else {
        confidence_sum / state.learned_vectors.len() as f32
    };
    let sludge = anti_sludge(&recurrence.temporal_memory);
    let sludge_risk = ((sludge.duplicate_ratio + sludge.low_signal_ratio) / 2.0).clamp(0.0, 1.0);
    let drift = recurrence.drift().clamp(0.0, 2.0) / 2.0;
    let convergence_score =
        (mean_confidence * 0.55 + (1.0 - sludge_risk) * 0.25 + (1.0 - drift) * 0.20)
            .clamp(0.0, 1.0);
    ConvergenceMetrics {
        attractor_count: state.attractors.len(),
        mean_confidence,
        drift: recurrence.drift(),
        sludge_risk,
        convergence_score,
    }
}

pub fn derive_attractors(vectors: &[LearnedLatentVector]) -> Vec<LatentAttractor> {
    let mut groups: BTreeMap<String, Vec<&LearnedLatentVector>> = BTreeMap::new();
    for vector in vectors {
        let key = vector
            .doctrine_tags
            .first()
            .cloned()
            .unwrap_or_else(|| "undifferentiated".into());
        groups.entry(key).or_default().push(vector);
    }
    groups
        .into_iter()
        .map(|(tag, group)| {
            let support = group.iter().map(|v| v.support).sum();
            let mean_score = if group.is_empty() {
                0.0
            } else {
                group.iter().map(|v| v.confidence).sum::<f32>() / group.len() as f32
            };
            let centroid = centroid(group.iter().map(|v| &v.vector));
            LatentAttractor {
                id: format!("attractor:{tag}"),
                centroid,
                support,
                mean_score,
                tags: vec![tag],
            }
        })
        .collect()
}

pub fn render_learning_report(
    state: &LatentLearningState,
    recurrence: &LatentOperationalState,
) -> String {
    let metrics = state
        .last_convergence
        .clone()
        .unwrap_or_else(|| convergence_metrics(state, recurrence));
    format!(
        "# Adaptive Latent Evolution Report\n\n\
Learning schema: `{}`\n\
Samples seen: `{}`\n\
Learned vectors: `{}`\n\
Attractors: `{}`\n\
Doctrine bindings: `{}`\n\
Immune responses: `{}`\n\
Topology edges: `{}`\n\n\
## Convergence\n\n```json\n{}\n```\n\n\
## Interpretation\n\n\
This report summarizes deterministic latent learning on top of operational recurrence. It scores outcomes, rejects unsafe samples with an immune gate, groups stable attractors, binds learned behavior back to invariants, exposes topology edges, and reports convergence without hiding state inside an opaque model.\n",
        state.schema_version,
        state.samples_seen,
        state.learned_vectors.len(),
        state.attractors.len(),
        state.doctrine_bindings.len(),
        state.immune_history.len(),
        state.topology.len(),
        serde_json::to_string_pretty(&metrics).unwrap_or_else(|_| "{}".into())
    )
}

fn upsert_learned_vector(state: &mut LatentLearningState, sample: &LatentLearningSample) {
    let id = sample
        .doctrine_tags
        .first()
        .cloned()
        .unwrap_or_else(|| sample.event.kind.clone());
    if let Some(existing) = state.learned_vectors.iter_mut().find(|v| v.id == id) {
        existing.support += 1;
        existing.confidence = ((existing.confidence * 0.85)
            + (sample.score.scalar().max(0.0) * 0.15))
            .clamp(0.0, 1.0);
        existing.vector = blend(&existing.vector, &sample.encoded, 0.20);
        existing.doctrine_tags = merge_tags(&existing.doctrine_tags, &sample.doctrine_tags);
    } else {
        state.learned_vectors.push(LearnedLatentVector {
            id,
            vector: sample.encoded.clone(),
            support: 1,
            confidence: sample.score.scalar().max(0.0),
            doctrine_tags: sample.doctrine_tags.clone(),
        });
    }
}

fn derive_doctrine_bindings(
    recurrence: &LatentOperationalState,
    sample: &LatentLearningSample,
) -> Vec<DoctrineBinding> {
    recurrence
        .invariants
        .iter()
        .filter(|inv| sample.doctrine_tags.contains(&inv.id))
        .map(|inv| DoctrineBinding {
            id: format!("binding:{}", inv.id),
            invariant_id: inv.id.clone(),
            strength: sample.score.scalar().max(0.0),
            rationale: format!(
                "learned operational support for {}",
                inv.canonical_expression
            ),
        })
        .collect()
}

fn derive_topology(
    attractors: &[LatentAttractor],
    bindings: &[DoctrineBinding],
) -> Vec<LatentTopologyEdge> {
    let mut edges = Vec::new();
    for attractor in attractors {
        for binding in bindings {
            if attractor
                .tags
                .iter()
                .any(|tag| tag == &binding.invariant_id)
            {
                edges.push(LatentTopologyEdge {
                    from: attractor.id.clone(),
                    to: binding.id.clone(),
                    weight: binding.strength,
                    relation: "supports-doctrine".into(),
                });
            }
        }
    }
    edges.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(Ordering::Equal));
    edges
}

fn centroid<'a>(vectors: impl Iterator<Item = &'a LatentVector>) -> LatentVector {
    let mut count = 0usize;
    let mut dims = Vec::<f32>::new();
    let mut schema = 1;
    for vector in vectors {
        schema = vector.schema_version;
        if dims.is_empty() {
            dims = vec![0.0; vector.dimensions.len()];
        }
        for (idx, value) in vector.dimensions.iter().enumerate() {
            dims[idx] += value;
        }
        count += 1;
    }
    if count > 0 {
        for dim in &mut dims {
            *dim /= count as f32;
        }
    }
    let mut vector = LatentVector {
        schema_version: schema,
        dimensions: dims,
    };
    vector.normalize();
    vector
}

fn blend(a: &LatentVector, b: &LatentVector, rate: f32) -> LatentVector {
    let dims = a
        .dimensions
        .iter()
        .zip(&b.dimensions)
        .map(|(x, y)| x * (1.0 - rate) + y * rate)
        .collect();
    let mut vector = LatentVector {
        schema_version: a.schema_version,
        dimensions: dims,
    };
    vector.normalize();
    vector
}

fn merge_tags(left: &[String], right: &[String]) -> Vec<String> {
    let mut tags: BTreeSet<String> = left.iter().cloned().collect();
    tags.extend(right.iter().cloned());
    tags.into_iter().collect()
}

fn trim<T>(items: &mut Vec<T>, max: usize) {
    if items.len() > max {
        items.drain(0..items.len() - max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latent_operational_recurrence::LatentOperationalState;

    #[test]
    fn learns_safe_validated_sample() {
        let recurrence = LatentOperationalState::default();
        let mut state = LatentLearningState::default();
        let mut event = OperationalEvent::new("build", "success");
        event.tags = vec!["test".into(), "validation".into()];
        let step = state.learn(&recurrence, event);
        assert!(!step.immune.triggered);
        assert_eq!(state.samples_seen, 1);
        assert!(!state.learned_vectors.is_empty());
    }

    #[test]
    fn immune_blocks_unconfirmed_destructive_sample() {
        let recurrence = LatentOperationalState::default();
        let mut state = LatentLearningState::default();
        let mut event = OperationalEvent::new("delete", "success");
        event.tags = vec!["destructive".into()];
        let step = state.learn(&recurrence, event);
        assert!(step.immune.triggered);
        assert_eq!(state.samples_seen, 0);
    }
}
