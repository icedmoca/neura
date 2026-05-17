//! Self-model integration and operational cognition tightening.
//!
//! This module provides a small, deterministic substrate that other cognitive
//! systems can use to describe what Kcode currently believes about its own
//! operating state.  It intentionally avoids provider-specific behavior and is
//! safe to run in tests, benchmarks, slash commands, replay, repair, and routing
//! code paths.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Functional area whose operational health can affect agent behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveDomain {
    ContextAssembly,
    MemoryRetrieval,
    ToolExecution,
    ProviderRouting,
    Repair,
    Replay,
    Benchmarking,
    UserInteraction,
}

impl fmt::Display for CognitiveDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::ContextAssembly => "context_assembly",
            Self::MemoryRetrieval => "memory_retrieval",
            Self::ToolExecution => "tool_execution",
            Self::ProviderRouting => "provider_routing",
            Self::Repair => "repair",
            Self::Replay => "replay",
            Self::Benchmarking => "benchmarking",
            Self::UserInteraction => "user_interaction",
        };
        f.write_str(name)
    }
}

/// Current state of a cognitive domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationalState {
    Nominal,
    Watch,
    Degraded,
    Blocked,
}

impl OperationalState {
    pub fn from_score(score: f32) -> Self {
        if score >= 0.82 {
            Self::Nominal
        } else if score >= 0.62 {
            Self::Watch
        } else if score >= 0.35 {
            Self::Degraded
        } else {
            Self::Blocked
        }
    }

    pub fn severity(self) -> u8 {
        match self {
            Self::Nominal => 0,
            Self::Watch => 1,
            Self::Degraded => 2,
            Self::Blocked => 3,
        }
    }
}

/// One observation that updates the self-model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CognitiveSignal {
    pub domain: CognitiveDomain,
    pub confidence: f32,
    pub load: f32,
    pub error_rate: f32,
    pub latency_ms: Option<u64>,
    pub note: Option<String>,
}

impl CognitiveSignal {
    pub fn new(domain: CognitiveDomain) -> Self {
        Self {
            domain,
            confidence: 1.0,
            load: 0.0,
            error_rate: 0.0,
            latency_ms: None,
            note: None,
        }
    }

    pub fn confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn load(mut self, load: f32) -> Self {
        self.load = load;
        self
    }

    pub fn error_rate(mut self, error_rate: f32) -> Self {
        self.error_rate = error_rate;
        self
    }

    pub fn latency_ms(mut self, latency_ms: u64) -> Self {
        self.latency_ms = Some(latency_ms);
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    fn normalized(&self) -> Self {
        let mut copy = self.clone();
        copy.confidence = clamp01(copy.confidence);
        copy.load = clamp01(copy.load);
        copy.error_rate = clamp01(copy.error_rate);
        copy
    }
}

/// Aggregated domain health.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainAssessment {
    pub domain: CognitiveDomain,
    pub state: OperationalState,
    pub score: f32,
    pub confidence: f32,
    pub load: f32,
    pub error_rate: f32,
    pub latency_ms: Option<u64>,
    pub observations: u64,
    pub last_note: Option<String>,
}

impl DomainAssessment {
    fn from_signal(signal: CognitiveSignal) -> Self {
        let signal = signal.normalized();
        let score = compute_score(
            signal.confidence,
            signal.load,
            signal.error_rate,
            signal.latency_ms,
        );
        Self {
            domain: signal.domain,
            state: OperationalState::from_score(score),
            score,
            confidence: signal.confidence,
            load: signal.load,
            error_rate: signal.error_rate,
            latency_ms: signal.latency_ms,
            observations: 1,
            last_note: signal.note,
        }
    }

    fn update(&mut self, signal: CognitiveSignal) {
        let signal = signal.normalized();
        let n = self.observations as f32;
        self.confidence = weighted_average(self.confidence, signal.confidence, n);
        self.load = weighted_average(self.load, signal.load, n);
        self.error_rate = weighted_average(self.error_rate, signal.error_rate, n);
        self.latency_ms = match (self.latency_ms, signal.latency_ms) {
            (Some(existing), Some(new)) => {
                Some(((existing as f32 * n + new as f32) / (n + 1.0)).round() as u64)
            }
            (None, Some(new)) => Some(new),
            (existing, None) => existing,
        };
        self.observations += 1;
        if signal.note.is_some() {
            self.last_note = signal.note;
        }
        self.score = compute_score(self.confidence, self.load, self.error_rate, self.latency_ms);
        self.state = OperationalState::from_score(self.score);
    }
}

/// A deterministic snapshot of Kcode's operational self-model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfModel {
    pub version: u32,
    pub domains: BTreeMap<CognitiveDomain, DomainAssessment>,
    pub global_state: OperationalState,
    pub global_score: f32,
    pub recommendations: Vec<String>,
}

impl Default for SelfModel {
    fn default() -> Self {
        Self::new()
    }
}

impl SelfModel {
    pub const VERSION: u32 = 1;

    pub fn new() -> Self {
        Self {
            version: Self::VERSION,
            domains: BTreeMap::new(),
            global_state: OperationalState::Nominal,
            global_score: 1.0,
            recommendations: Vec::new(),
        }
    }

    pub fn observe(&mut self, signal: CognitiveSignal) {
        self.domains
            .entry(signal.domain)
            .and_modify(|assessment| assessment.update(signal.clone()))
            .or_insert_with(|| DomainAssessment::from_signal(signal));
        self.recompute();
    }

    pub fn observe_many<I>(&mut self, signals: I)
    where
        I: IntoIterator<Item = CognitiveSignal>,
    {
        for signal in signals {
            self.observe(signal);
        }
    }

    pub fn assessment(&self, domain: CognitiveDomain) -> Option<&DomainAssessment> {
        self.domains.get(&domain)
    }

    pub fn is_operational(&self) -> bool {
        self.global_state.severity() < OperationalState::Blocked.severity()
    }

    pub fn routing_bias(&self) -> RoutingBias {
        let provider = self.assessment(CognitiveDomain::ProviderRouting);
        let context = self.assessment(CognitiveDomain::ContextAssembly);
        let tools = self.assessment(CognitiveDomain::ToolExecution);

        RoutingBias {
            prefer_low_latency: provider.is_some_and(|a| a.latency_ms.unwrap_or(0) > 4_000),
            prefer_high_context: context
                .is_some_and(|a| a.state.severity() >= OperationalState::Watch.severity()),
            avoid_tool_heavy_plan: tools
                .is_some_and(|a| a.state.severity() >= OperationalState::Degraded.severity()),
            global_score: self.global_score,
        }
    }

    pub fn repair_hints(&self) -> Vec<String> {
        let mut hints = Vec::new();
        for assessment in self.domains.values() {
            match assessment.domain {
                CognitiveDomain::ContextAssembly if assessment.state.severity() >= 1 => {
                    hints.push("rebuild context with stricter relevance filtering".to_string());
                }
                CognitiveDomain::MemoryRetrieval if assessment.state.severity() >= 1 => {
                    hints.push("refresh memory retrieval and verify stale summaries".to_string());
                }
                CognitiveDomain::ToolExecution if assessment.state.severity() >= 2 => {
                    hints.push(
                        "prefer smaller tool calls and validate outputs before chaining"
                            .to_string(),
                    );
                }
                CognitiveDomain::ProviderRouting if assessment.state.severity() >= 1 => {
                    hints.push(
                        "re-evaluate provider/model routing for latency or error pressure"
                            .to_string(),
                    );
                }
                CognitiveDomain::Replay if assessment.state.severity() >= 1 => {
                    hints.push("capture replay artifacts before further mutation".to_string());
                }
                _ => {}
            }
        }
        hints.sort();
        hints.dedup();
        hints
    }

    fn recompute(&mut self) {
        if self.domains.is_empty() {
            self.global_score = 1.0;
            self.global_state = OperationalState::Nominal;
            self.recommendations.clear();
            return;
        }

        let total: f32 = self.domains.values().map(|a| a.score).sum();
        self.global_score = total / self.domains.len() as f32;
        let worst_state = self
            .domains
            .values()
            .map(|a| a.state)
            .max_by_key(|state| state.severity())
            .unwrap_or(OperationalState::Nominal);
        self.global_state = if worst_state.severity() >= OperationalState::Degraded.severity() {
            worst_state
        } else {
            OperationalState::from_score(self.global_score)
        };
        self.recommendations = self.repair_hints();
    }
}

/// Provider/router-facing interpretation of the self-model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingBias {
    pub prefer_low_latency: bool,
    pub prefer_high_context: bool,
    pub avoid_tool_heavy_plan: bool,
    pub global_score: f32,
}

pub fn model_from_signals<I>(signals: I) -> SelfModel
where
    I: IntoIterator<Item = CognitiveSignal>,
{
    let mut model = SelfModel::new();
    model.observe_many(signals);
    model
}

fn weighted_average(existing: f32, incoming: f32, existing_count: f32) -> f32 {
    clamp01((existing * existing_count + incoming) / (existing_count + 1.0))
}

fn compute_score(confidence: f32, load: f32, error_rate: f32, latency_ms: Option<u64>) -> f32 {
    let latency_penalty = latency_ms
        .map(|ms| ((ms as f32 - 1_000.0).max(0.0) / 9_000.0).min(0.25))
        .unwrap_or(0.0);
    clamp01(confidence * 0.62 + (1.0 - load) * 0.18 + (1.0 - error_rate) * 0.20 - latency_penalty)
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

    #[test]
    fn nominal_signal_keeps_model_operational() {
        let model = model_from_signals([CognitiveSignal::new(CognitiveDomain::ContextAssembly)]);
        assert_eq!(model.global_state, OperationalState::Nominal);
        assert!(model.is_operational());
        assert!(model.recommendations.is_empty());
    }

    #[test]
    fn degraded_tool_signal_produces_repair_hint_and_routing_bias() {
        let model = model_from_signals([CognitiveSignal::new(CognitiveDomain::ToolExecution)
            .confidence(0.35)
            .load(0.9)
            .error_rate(0.55)
            .note("tool chain failures")]);

        assert!(matches!(
            model.global_state,
            OperationalState::Degraded | OperationalState::Blocked
        ));
        assert!(model.routing_bias().avoid_tool_heavy_plan);
        assert!(
            model
                .recommendations
                .iter()
                .any(|hint| hint.contains("smaller tool calls"))
        );
    }

    #[test]
    fn provider_latency_bias_prefers_low_latency() {
        let model = model_from_signals([CognitiveSignal::new(CognitiveDomain::ProviderRouting)
            .confidence(0.9)
            .load(0.2)
            .error_rate(0.0)
            .latency_ms(8_000)]);

        assert!(model.routing_bias().prefer_low_latency);
    }

    #[test]
    fn repeated_observations_are_averaged() {
        let mut model = SelfModel::new();
        model.observe(CognitiveSignal::new(CognitiveDomain::MemoryRetrieval).confidence(1.0));
        model.observe(CognitiveSignal::new(CognitiveDomain::MemoryRetrieval).confidence(0.0));
        let assessment = model.assessment(CognitiveDomain::MemoryRetrieval).unwrap();
        assert!((assessment.confidence - 0.5).abs() < 0.001);
        assert_eq!(assessment.observations, 2);
    }
}

/// High-level operational event accepted by the self-model integration facade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationalEvent {
    ContextCompiled {
        confidence: f32,
        token_pressure: f32,
        latency_ms: Option<u64>,
    },
    MemoryLookup {
        confidence: f32,
        stale_ratio: f32,
        latency_ms: Option<u64>,
    },
    ToolRun {
        success: bool,
        latency_ms: Option<u64>,
    },
    ProviderDecision {
        confidence: f32,
        latency_ms: Option<u64>,
    },
    RepairAttempt {
        success: bool,
    },
    ReplayCheck {
        deterministic: bool,
    },
    BenchmarkSample {
        pass_rate: f32,
        load: f32,
    },
    UserTurn {
        ambiguity: f32,
    },
}

impl OperationalEvent {
    pub fn into_signal(self) -> CognitiveSignal {
        match self {
            Self::ContextCompiled {
                confidence,
                token_pressure,
                latency_ms,
            } => with_latency(
                CognitiveSignal::new(CognitiveDomain::ContextAssembly)
                    .confidence(confidence)
                    .load(token_pressure),
                latency_ms,
            ),
            Self::MemoryLookup {
                confidence,
                stale_ratio,
                latency_ms,
            } => with_latency(
                CognitiveSignal::new(CognitiveDomain::MemoryRetrieval)
                    .confidence(confidence)
                    .error_rate(stale_ratio),
                latency_ms,
            ),
            Self::ToolRun {
                success,
                latency_ms,
            } => with_latency(
                CognitiveSignal::new(CognitiveDomain::ToolExecution)
                    .confidence(if success { 1.0 } else { 0.25 })
                    .error_rate(if success { 0.0 } else { 1.0 }),
                latency_ms,
            ),
            Self::ProviderDecision {
                confidence,
                latency_ms,
            } => with_latency(
                CognitiveSignal::new(CognitiveDomain::ProviderRouting).confidence(confidence),
                latency_ms,
            ),
            Self::RepairAttempt { success } => CognitiveSignal::new(CognitiveDomain::Repair)
                .confidence(if success { 1.0 } else { 0.35 })
                .error_rate(if success { 0.0 } else { 0.8 }),
            Self::ReplayCheck { deterministic } => CognitiveSignal::new(CognitiveDomain::Replay)
                .confidence(if deterministic { 1.0 } else { 0.45 })
                .error_rate(if deterministic { 0.0 } else { 0.6 }),
            Self::BenchmarkSample { pass_rate, load } => {
                CognitiveSignal::new(CognitiveDomain::Benchmarking)
                    .confidence(pass_rate)
                    .load(load)
                    .error_rate(1.0 - clamp01(pass_rate))
            }
            Self::UserTurn { ambiguity } => CognitiveSignal::new(CognitiveDomain::UserInteraction)
                .confidence(1.0 - clamp01(ambiguity))
                .load(ambiguity),
        }
    }
}

/// Operational cognition facade used by routing, repair, replay, benchmark, and
/// slash-command surfaces.  It gives those systems one stable place to submit
/// events and retrieve tightened operational guidance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationalCognition {
    pub self_model: SelfModel,
}

impl Default for OperationalCognition {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationalCognition {
    pub fn new() -> Self {
        Self {
            self_model: SelfModel::new(),
        }
    }

    pub fn ingest(&mut self, event: OperationalEvent) -> &SelfModel {
        self.self_model.observe(event.into_signal());
        &self.self_model
    }

    pub fn ingest_many<I>(&mut self, events: I) -> &SelfModel
    where
        I: IntoIterator<Item = OperationalEvent>,
    {
        for event in events {
            self.ingest(event);
        }
        &self.self_model
    }

    pub fn guidance(&self) -> OperationalGuidance {
        OperationalGuidance {
            state: self.self_model.global_state,
            score: self.self_model.global_score,
            routing_bias: self.self_model.routing_bias(),
            repair_hints: self.self_model.repair_hints(),
            should_pause_for_repair: self.self_model.global_state.severity()
                >= OperationalState::Degraded.severity(),
        }
    }
}

/// Compact guidance object that can be rendered by commands, telemetry, repair,
/// replay, benchmark, or provider-routing code without depending on internals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationalGuidance {
    pub state: OperationalState,
    pub score: f32,
    pub routing_bias: RoutingBias,
    pub repair_hints: Vec<String>,
    pub should_pause_for_repair: bool,
}

fn with_latency(mut signal: CognitiveSignal, latency_ms: Option<u64>) -> CognitiveSignal {
    if let Some(latency_ms) = latency_ms {
        signal = signal.latency_ms(latency_ms);
    }
    signal
}

#[cfg(test)]
mod operational_cognition_tests {
    use super::*;

    #[test]
    fn operational_events_update_domains_and_guidance() {
        let mut cognition = OperationalCognition::new();
        cognition.ingest_many([
            OperationalEvent::ContextCompiled {
                confidence: 0.74,
                token_pressure: 0.7,
                latency_ms: Some(1_200),
            },
            OperationalEvent::ProviderDecision {
                confidence: 0.88,
                latency_ms: Some(6_500),
            },
        ]);

        let guidance = cognition.guidance();
        assert!(guidance.routing_bias.prefer_high_context);
        assert!(guidance.routing_bias.prefer_low_latency);
        assert!(
            cognition
                .self_model
                .assessment(CognitiveDomain::ContextAssembly)
                .is_some()
        );
    }

    #[test]
    fn failed_tool_event_requests_repair_pause() {
        let mut cognition = OperationalCognition::new();
        cognition.ingest(OperationalEvent::ToolRun {
            success: false,
            latency_ms: Some(250),
        });

        let guidance = cognition.guidance();
        assert!(guidance.should_pause_for_repair);
        assert!(guidance.routing_bias.avoid_tool_heavy_plan);
    }
}
