//! Operational cognition verbalization and semantic state abstraction.
//!
//! This layer sits above `self_model` and turns numeric operational cognition
//! into bounded, non-anthropomorphic semantic state.  It is intentionally
//! deterministic: command surfaces, telemetry, repair, replay, routing, and
//! benchmarks can all consume the same abstraction without prompt-only logic.

use crate::self_model::{CognitiveDomain, OperationalGuidance, OperationalState, SelfModel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Semantic category assigned to one cognitive domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticLabel {
    Stable,
    Monitoring,
    Compressed,
    Recovering,
    Blocked,
}

impl fmt::Display for SemanticLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stable => "stable",
            Self::Monitoring => "monitoring",
            Self::Compressed => "compressed",
            Self::Recovering => "recovering",
            Self::Blocked => "blocked",
        })
    }
}

/// Intent-safe verbalization mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerbalizationMode {
    /// Minimal state for status lines and slash command summaries.
    Compact,
    /// More detail for diagnostics, docs, and telemetry.
    Diagnostic,
    /// Explicitly machine-readable text with stable keys.
    Machine,
}

/// Runtime budget that bounds introspection verbosity and avoids runaway
/// self-description loops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerbalizationBudget {
    pub max_domains: usize,
    pub max_reasons: usize,
    pub max_chars: usize,
}

impl Default for VerbalizationBudget {
    fn default() -> Self {
        Self {
            max_domains: 6,
            max_reasons: 4,
            max_chars: 900,
        }
    }
}

/// Metrics that abstract the shape of the current operational state.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SemanticMetrics {
    pub coherence: f32,
    pub drift: f32,
    pub convergence: f32,
    pub compression: f32,
}

impl SemanticMetrics {
    pub fn from_model(model: &SelfModel) -> Self {
        if model.domains.is_empty() {
            return Self {
                coherence: 1.0,
                drift: 0.0,
                convergence: 1.0,
                compression: 0.0,
            };
        }

        let count = model.domains.len() as f32;
        let mean = model.domains.values().map(|a| a.score).sum::<f32>() / count;
        let variance = model
            .domains
            .values()
            .map(|a| (a.score - mean).powi(2))
            .sum::<f32>()
            / count;
        let spread = variance.sqrt().min(1.0);
        let degraded = model
            .domains
            .values()
            .filter(|a| a.state.severity() >= OperationalState::Degraded.severity())
            .count() as f32;
        let watch_or_worse = model
            .domains
            .values()
            .filter(|a| a.state.severity() >= OperationalState::Watch.severity())
            .count() as f32;
        let avg_load = model.domains.values().map(|a| a.load).sum::<f32>() / count;

        Self {
            coherence: clamp01(1.0 - spread - degraded * 0.08),
            drift: clamp01(watch_or_worse / count * 0.68 + (1.0 - model.global_score) * 0.32),
            convergence: clamp01(model.global_score * 0.72 + (1.0 - spread) * 0.28),
            compression: clamp01(avg_load * 0.7 + watch_or_worse / count * 0.3),
        }
    }
}

/// One semantic abstraction of a domain assessment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticDomainState {
    pub domain: CognitiveDomain,
    pub label: SemanticLabel,
    pub score: f32,
    pub reason: String,
}

/// Complete semantic state derived from the self-model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticOperationalState {
    pub label: SemanticLabel,
    pub operational_state: OperationalState,
    pub metrics: SemanticMetrics,
    pub domains: Vec<SemanticDomainState>,
    pub guidance: OperationalGuidance,
    pub reasons: Vec<String>,
}

impl SemanticOperationalState {
    pub fn from_model(model: &SelfModel) -> Self {
        let metrics = SemanticMetrics::from_model(model);
        let guidance = OperationalGuidance {
            state: model.global_state,
            score: model.global_score,
            routing_bias: model.routing_bias(),
            repair_hints: model.repair_hints(),
            should_pause_for_repair: model.global_state.severity()
                >= OperationalState::Degraded.severity(),
        };
        let mut domains = model
            .domains
            .values()
            .map(|assessment| SemanticDomainState {
                domain: assessment.domain,
                label: label_domain(
                    assessment.state,
                    assessment.load,
                    assessment.error_rate,
                    assessment.score,
                ),
                score: assessment.score,
                reason: domain_reason(
                    assessment.domain,
                    assessment.state,
                    assessment.load,
                    assessment.error_rate,
                    assessment.latency_ms,
                ),
            })
            .collect::<Vec<_>>();
        domains.sort_by(|a, b| {
            b.label
                .cmp(&a.label)
                .then_with(|| a.score.total_cmp(&b.score))
                .then_with(|| a.domain.cmp(&b.domain))
        });

        let label = label_global(model.global_state, metrics);
        let reasons = summarize_reasons(label, metrics, &domains, &guidance);
        Self {
            label,
            operational_state: model.global_state,
            metrics,
            domains,
            guidance,
            reasons,
        }
    }

    pub fn verbalize(&self, mode: VerbalizationMode, budget: VerbalizationBudget) -> String {
        let mut output = match mode {
            VerbalizationMode::Compact => self.verbalize_compact(&budget),
            VerbalizationMode::Diagnostic => self.verbalize_diagnostic(&budget),
            VerbalizationMode::Machine => self.verbalize_machine(&budget),
        };
        if output.len() > budget.max_chars {
            let mut boundary = budget.max_chars.saturating_sub(3);
            while boundary > 0 && !output.is_char_boundary(boundary) {
                boundary -= 1;
            }
            output.truncate(boundary);
            output.push_str("...");
        }
        output
    }

    fn verbalize_compact(&self, budget: &VerbalizationBudget) -> String {
        let reasons = self
            .reasons
            .iter()
            .take(budget.max_reasons)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "semantic_state={} operational_state={:?} score={:.2} coherence={:.2} drift={:.2} convergence={:.2}{}",
            self.label,
            self.operational_state,
            self.guidance.score,
            self.metrics.coherence,
            self.metrics.drift,
            self.metrics.convergence,
            if reasons.is_empty() {
                String::new()
            } else {
                format!(" reasons={}", reasons)
            }
        )
    }

    fn verbalize_diagnostic(&self, budget: &VerbalizationBudget) -> String {
        let mut lines = vec![format!(
            "Semantic operational state: {} ({:?}, score {:.2})",
            self.label, self.operational_state, self.guidance.score
        )];
        lines.push(format!(
            "Metrics: coherence {:.2}, drift {:.2}, convergence {:.2}, compression {:.2}",
            self.metrics.coherence,
            self.metrics.drift,
            self.metrics.convergence,
            self.metrics.compression
        ));
        for reason in self.reasons.iter().take(budget.max_reasons) {
            lines.push(format!("- {}", reason));
        }
        for domain in self.domains.iter().take(budget.max_domains) {
            lines.push(format!(
                "- {}: {} score {:.2} ({})",
                domain.domain, domain.label, domain.score, domain.reason
            ));
        }
        if self.guidance.should_pause_for_repair {
            lines.push(
                "Action: pause risky chaining and prefer repair/replay/context rebuild."
                    .to_string(),
            );
        }
        lines.join("\n")
    }

    fn verbalize_machine(&self, budget: &VerbalizationBudget) -> String {
        let mut fields = BTreeMap::new();
        fields.insert("a_semantic_state", self.label.to_string());
        fields.insert(
            "b_operational_state",
            format!("{:?}", self.operational_state),
        );
        fields.insert("c_score", format!("{:.3}", self.guidance.score));
        fields.insert("d_coherence", format!("{:.3}", self.metrics.coherence));
        fields.insert("e_drift", format!("{:.3}", self.metrics.drift));
        fields.insert("f_convergence", format!("{:.3}", self.metrics.convergence));
        fields.insert("g_compression", format!("{:.3}", self.metrics.compression));
        fields.insert(
            "h_pause_for_repair",
            self.guidance.should_pause_for_repair.to_string(),
        );
        fields.insert(
            "i_domains",
            self.domains
                .iter()
                .take(budget.max_domains)
                .map(|d| format!("{}:{}:{:.2}", d.domain, d.label, d.score))
                .collect::<Vec<_>>()
                .join(","),
        );
        fields
            .into_iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Public facade: abstract a self-model into semantic state.
pub fn abstract_semantic_state(model: &SelfModel) -> SemanticOperationalState {
    SemanticOperationalState::from_model(model)
}

/// Public facade: produce bounded operational cognition language.
pub fn verbalize_self_model(
    model: &SelfModel,
    mode: VerbalizationMode,
    budget: VerbalizationBudget,
) -> String {
    abstract_semantic_state(model).verbalize(mode, budget)
}

fn label_global(state: OperationalState, metrics: SemanticMetrics) -> SemanticLabel {
    match state {
        OperationalState::Blocked => SemanticLabel::Blocked,
        OperationalState::Degraded => SemanticLabel::Recovering,
        OperationalState::Watch if metrics.compression >= 0.62 => SemanticLabel::Compressed,
        OperationalState::Watch => SemanticLabel::Monitoring,
        OperationalState::Nominal if metrics.drift > 0.22 => SemanticLabel::Monitoring,
        OperationalState::Nominal => SemanticLabel::Stable,
    }
}

fn label_domain(state: OperationalState, load: f32, error_rate: f32, score: f32) -> SemanticLabel {
    match state {
        OperationalState::Blocked => SemanticLabel::Blocked,
        OperationalState::Degraded => SemanticLabel::Recovering,
        OperationalState::Watch if load >= 0.72 => SemanticLabel::Compressed,
        OperationalState::Watch => SemanticLabel::Monitoring,
        OperationalState::Nominal if error_rate > 0.15 || score < 0.9 => SemanticLabel::Monitoring,
        OperationalState::Nominal => SemanticLabel::Stable,
    }
}

fn domain_reason(
    domain: CognitiveDomain,
    state: OperationalState,
    load: f32,
    error_rate: f32,
    latency_ms: Option<u64>,
) -> String {
    let mut parts = vec![format!("{:?}", state)];
    if load >= 0.7 {
        parts.push("high load".to_string());
    }
    if error_rate >= 0.25 {
        parts.push("elevated errors".to_string());
    }
    if latency_ms.is_some_and(|ms| ms > 4_000) {
        parts.push("high latency".to_string());
    }
    format!("{} reports {}", domain, parts.join(", "))
}

fn summarize_reasons(
    label: SemanticLabel,
    metrics: SemanticMetrics,
    domains: &[SemanticDomainState],
    guidance: &OperationalGuidance,
) -> Vec<String> {
    let mut reasons = Vec::new();
    reasons.push(format!("global semantic label is {}", label));
    if metrics.drift >= 0.35 {
        reasons.push(format!("operational drift is {:.2}", metrics.drift));
    }
    if metrics.compression >= 0.55 {
        reasons.push(format!(
            "compression pressure is {:.2}",
            metrics.compression
        ));
    }
    if metrics.coherence < 0.72 {
        reasons.push(format!("coherence is {:.2}", metrics.coherence));
    }
    if guidance.should_pause_for_repair {
        reasons.push("repair pause is recommended".to_string());
    }
    for domain in domains.iter().take(3) {
        if domain.label >= SemanticLabel::Monitoring {
            reasons.push(format!("{} is {}", domain.domain, domain.label));
        }
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

fn clamp01(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::self_model::{CognitiveSignal, OperationalCognition, OperationalEvent};

    #[test]
    fn nominal_model_verbalizes_as_stable() {
        let mut model = SelfModel::new();
        model.observe(CognitiveSignal::new(CognitiveDomain::ContextAssembly));
        let semantic = abstract_semantic_state(&model);
        assert_eq!(semantic.label, SemanticLabel::Stable);
        assert!(semantic.metrics.coherence > 0.9);
        assert!(
            semantic
                .verbalize(VerbalizationMode::Compact, Default::default())
                .contains("semantic_state=stable")
        );
    }

    #[test]
    fn tool_failure_becomes_recovering_and_requests_repair() {
        let mut cognition = OperationalCognition::new();
        cognition.ingest(OperationalEvent::ToolRun {
            success: false,
            latency_ms: Some(500),
        });
        let semantic = abstract_semantic_state(&cognition.self_model);
        assert!(matches!(
            semantic.label,
            SemanticLabel::Recovering | SemanticLabel::Blocked
        ));
        assert!(semantic.guidance.should_pause_for_repair);
        assert!(
            semantic
                .verbalize(VerbalizationMode::Diagnostic, Default::default())
                .contains("pause risky chaining")
        );
    }

    #[test]
    fn high_context_load_becomes_compressed_or_monitoring() {
        let mut cognition = OperationalCognition::new();
        cognition.ingest(OperationalEvent::ContextCompiled {
            confidence: 0.72,
            token_pressure: 0.9,
            latency_ms: Some(1_100),
        });
        let semantic = abstract_semantic_state(&cognition.self_model);
        assert!(matches!(
            semantic.label,
            SemanticLabel::Compressed
                | SemanticLabel::Monitoring
                | SemanticLabel::Recovering
                | SemanticLabel::Blocked
        ));
        assert!(semantic.metrics.compression > 0.5);
    }

    #[test]
    fn machine_verbalization_respects_budget() {
        let mut cognition = OperationalCognition::new();
        cognition.ingest_many([
            OperationalEvent::ContextCompiled {
                confidence: 0.65,
                token_pressure: 0.8,
                latency_ms: Some(1_000),
            },
            OperationalEvent::ProviderDecision {
                confidence: 0.8,
                latency_ms: Some(8_000),
            },
        ]);
        let text = verbalize_self_model(
            &cognition.self_model,
            VerbalizationMode::Machine,
            VerbalizationBudget {
                max_domains: 1,
                max_reasons: 1,
                max_chars: 120,
            },
        );
        assert!(text.len() <= 120);
        assert!(text.contains("a_semantic_state="));
    }
}
