//! Long-horizon operational pressure and continuous cognition stress infrastructure.
//!
//! This module provides bounded, deterministic stress simulation over Neura's
//! existing `SelfModel` and semantic operational layer.  It is designed for
//! tests, benchmarks, telemetry snapshots, and future command surfaces.  It is
//! explicitly not an autonomous daemon: callers choose the horizon and sample
//! stream, and every run is finite.

use crate::self_model::{OperationalCognition, OperationalEvent, OperationalState};
use crate::semantic_operational_layer::{
    SemanticLabel, SemanticMetrics, SemanticOperationalState, VerbalizationBudget,
    VerbalizationMode, abstract_semantic_state,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Bounded stress scenario class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StressScenario {
    Baseline,
    ContextSaturation,
    ToolFailureBurst,
    ProviderLatency,
    MemoryStaleness,
    MixedLongHorizon,
}

impl fmt::Display for StressScenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Baseline => "baseline",
            Self::ContextSaturation => "context_saturation",
            Self::ToolFailureBurst => "tool_failure_burst",
            Self::ProviderLatency => "provider_latency",
            Self::MemoryStaleness => "memory_staleness",
            Self::MixedLongHorizon => "mixed_long_horizon",
        })
    }
}

/// One finite stress sample.  A sample maps to one or more operational events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressSample {
    pub step: u64,
    pub scenario: StressScenario,
    pub events: Vec<OperationalEvent>,
    pub note: Option<String>,
}

impl StressSample {
    pub fn new(step: u64, scenario: StressScenario, events: Vec<OperationalEvent>) -> Self {
        Self {
            step,
            scenario,
            events,
            note: None,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Configuration for a bounded horizon run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HorizonConfig {
    pub max_steps: usize,
    pub warning_drift: f32,
    pub warning_compression: f32,
    pub min_convergence: f32,
    pub fail_on_blocked: bool,
}

impl Default for HorizonConfig {
    fn default() -> Self {
        Self {
            max_steps: 128,
            warning_drift: 0.42,
            warning_compression: 0.68,
            min_convergence: 0.45,
            fail_on_blocked: true,
        }
    }
}

/// Per-step pressure reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PressureReading {
    pub step: u64,
    pub scenario: StressScenario,
    pub operational_state: OperationalState,
    pub semantic_label: SemanticLabel,
    pub metrics: SemanticMetrics,
    pub score: f32,
    pub warnings: Vec<String>,
}

impl PressureReading {
    fn from_semantic(
        step: u64,
        scenario: StressScenario,
        semantic: &SemanticOperationalState,
    ) -> Self {
        let mut warnings = Vec::new();
        if semantic.metrics.drift >= 0.42 {
            warnings.push(format!(
                "drift {:.2} exceeds watch threshold",
                semantic.metrics.drift
            ));
        }
        if semantic.metrics.compression >= 0.68 {
            warnings.push(format!(
                "compression {:.2} exceeds watch threshold",
                semantic.metrics.compression
            ));
        }
        if semantic.metrics.convergence < 0.45 {
            warnings.push(format!(
                "convergence {:.2} below minimum",
                semantic.metrics.convergence
            ));
        }
        if semantic.guidance.should_pause_for_repair {
            warnings.push("repair pause recommended".to_string());
        }
        Self {
            step,
            scenario,
            operational_state: semantic.operational_state,
            semantic_label: semantic.label,
            metrics: semantic.metrics,
            score: semantic.guidance.score,
            warnings,
        }
    }
}

/// Aggregate result for one bounded horizon run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongHorizonReport {
    pub steps_run: usize,
    pub blocked_steps: usize,
    pub repair_pause_steps: usize,
    pub max_drift: f32,
    pub max_compression: f32,
    pub min_convergence: f32,
    pub final_state: SemanticOperationalState,
    pub readings: Vec<PressureReading>,
}

impl LongHorizonReport {
    pub fn passed(&self, config: &HorizonConfig) -> bool {
        self.max_drift < config.warning_drift
            && self.max_compression < config.warning_compression
            && self.min_convergence >= config.min_convergence
            && (!config.fail_on_blocked || self.blocked_steps == 0)
    }

    pub fn markdown_summary(&self, config: &HorizonConfig) -> String {
        let status = if self.passed(config) { "PASS" } else { "WATCH" };
        let mut out = String::new();
        out.push_str(&format!(
            "# Long-Horizon Operational Pressure Report\n\nStatus: **{}**\n\n",
            status
        ));
        out.push_str(&format!("- steps run: {}\n", self.steps_run));
        out.push_str(&format!("- blocked steps: {}\n", self.blocked_steps));
        out.push_str(&format!(
            "- repair pause steps: {}\n",
            self.repair_pause_steps
        ));
        out.push_str(&format!("- max drift: {:.2}\n", self.max_drift));
        out.push_str(&format!("- max compression: {:.2}\n", self.max_compression));
        out.push_str(&format!("- min convergence: {:.2}\n", self.min_convergence));
        out.push_str("\n## Final Semantic State\n\n```text\n");
        out.push_str(&self.final_state.verbalize(
            VerbalizationMode::Diagnostic,
            VerbalizationBudget {
                max_domains: 8,
                max_reasons: 6,
                max_chars: 1600,
            },
        ));
        out.push_str("\n```\n\n## Recent Readings\n\n");
        for reading in self.readings.iter().rev().take(8).rev() {
            out.push_str(&format!(
                "- step {} `{}`: {:?} / {} score {:.2}, drift {:.2}, compression {:.2}, convergence {:.2}\n",
                reading.step,
                reading.scenario,
                reading.operational_state,
                reading.semantic_label,
                reading.score,
                reading.metrics.drift,
                reading.metrics.compression,
                reading.metrics.convergence
            ));
            for warning in &reading.warnings {
                out.push_str(&format!("  - warning: {}\n", warning));
            }
        }
        out
    }
}

/// Finite long-horizon runner.
#[derive(Debug, Clone)]
pub struct LongHorizonStressRunner {
    pub config: HorizonConfig,
    cognition: OperationalCognition,
    readings: Vec<PressureReading>,
}

impl LongHorizonStressRunner {
    pub fn new(config: HorizonConfig) -> Self {
        Self {
            config,
            cognition: OperationalCognition::new(),
            readings: Vec::new(),
        }
    }

    pub fn run<I>(mut self, samples: I) -> LongHorizonReport
    where
        I: IntoIterator<Item = StressSample>,
    {
        for sample in samples.into_iter().take(self.config.max_steps) {
            self.ingest(sample);
        }
        self.finish()
    }

    pub fn ingest(&mut self, sample: StressSample) {
        for event in sample.events {
            self.cognition.ingest(event);
        }
        let semantic = abstract_semantic_state(&self.cognition.self_model);
        self.readings.push(PressureReading::from_semantic(
            sample.step,
            sample.scenario,
            &semantic,
        ));
    }

    pub fn finish(self) -> LongHorizonReport {
        let final_state = abstract_semantic_state(&self.cognition.self_model);
        let blocked_steps = self
            .readings
            .iter()
            .filter(|r| r.operational_state == OperationalState::Blocked)
            .count();
        let repair_pause_steps = self
            .readings
            .iter()
            .filter(|r| r.warnings.iter().any(|w| w.contains("repair pause")))
            .count();
        let max_drift = self
            .readings
            .iter()
            .map(|r| r.metrics.drift)
            .fold(0.0, f32::max);
        let max_compression = self
            .readings
            .iter()
            .map(|r| r.metrics.compression)
            .fold(0.0, f32::max);
        let min_convergence = self
            .readings
            .iter()
            .map(|r| r.metrics.convergence)
            .fold(1.0, f32::min);
        LongHorizonReport {
            steps_run: self.readings.len(),
            blocked_steps,
            repair_pause_steps,
            max_drift,
            max_compression,
            min_convergence,
            final_state,
            readings: self.readings,
        }
    }
}

/// Deterministic built-in scenarios for tests, benchmarks, and docs.
pub fn generate_scenario(scenario: StressScenario, steps: usize) -> Vec<StressSample> {
    (0..steps)
        .map(|idx| {
            let step = idx as u64;
            match scenario {
                StressScenario::Baseline => StressSample::new(
                    step,
                    scenario,
                    vec![
                        OperationalEvent::ContextCompiled {
                            confidence: 0.94,
                            token_pressure: 0.22,
                            latency_ms: Some(350),
                        },
                        OperationalEvent::ToolRun {
                            success: true,
                            latency_ms: Some(180),
                        },
                    ],
                ),
                StressScenario::ContextSaturation => StressSample::new(
                    step,
                    scenario,
                    vec![OperationalEvent::ContextCompiled {
                        confidence: 0.78 - ramp(idx, steps) * 0.22,
                        token_pressure: 0.55 + ramp(idx, steps) * 0.4,
                        latency_ms: Some(900 + step * 18),
                    }],
                ),
                StressScenario::ToolFailureBurst => StressSample::new(
                    step,
                    scenario,
                    vec![OperationalEvent::ToolRun {
                        success: idx % 4 != 0,
                        latency_ms: Some(250 + (idx % 5) as u64 * 120),
                    }],
                ),
                StressScenario::ProviderLatency => StressSample::new(
                    step,
                    scenario,
                    vec![OperationalEvent::ProviderDecision {
                        confidence: 0.86,
                        latency_ms: Some(1_000 + step * 220),
                    }],
                ),
                StressScenario::MemoryStaleness => StressSample::new(
                    step,
                    scenario,
                    vec![OperationalEvent::MemoryLookup {
                        confidence: 0.82 - ramp(idx, steps) * 0.25,
                        stale_ratio: 0.18 + ramp(idx, steps) * 0.45,
                        latency_ms: Some(500 + step * 10),
                    }],
                ),
                StressScenario::MixedLongHorizon => {
                    let mut events = vec![OperationalEvent::ContextCompiled {
                        confidence: 0.86 - ramp(idx, steps) * 0.18,
                        token_pressure: 0.35 + ramp(idx, steps) * 0.42,
                        latency_ms: Some(600 + step * 22),
                    }];
                    if idx % 3 == 0 {
                        events.push(OperationalEvent::ToolRun {
                            success: idx % 9 != 0,
                            latency_ms: Some(300 + (idx % 7) as u64 * 140),
                        });
                    }
                    if idx % 5 == 0 {
                        events.push(OperationalEvent::ProviderDecision {
                            confidence: 0.84,
                            latency_ms: Some(1_200 + step * 160),
                        });
                    }
                    StressSample::new(step, scenario, events)
                }
            }
        })
        .collect()
}

fn ramp(idx: usize, steps: usize) -> f32 {
    if steps <= 1 {
        1.0
    } else {
        (idx as f32 / (steps - 1) as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_pressure_passes_default_thresholds() {
        let config = HorizonConfig::default();
        let report = LongHorizonStressRunner::new(config.clone())
            .run(generate_scenario(StressScenario::Baseline, 16));
        assert_eq!(report.steps_run, 16);
        assert_eq!(report.blocked_steps, 0);
        assert!(report.passed(&config));
    }

    #[test]
    fn mixed_horizon_records_pressure_without_exceeding_bound() {
        let config = HorizonConfig {
            max_steps: 12,
            ..Default::default()
        };
        let report = LongHorizonStressRunner::new(config.clone())
            .run(generate_scenario(StressScenario::MixedLongHorizon, 40));
        assert_eq!(report.steps_run, 12);
        assert!(!report.readings.is_empty());
        assert!(report.max_drift >= 0.0);
    }

    #[test]
    fn tool_failure_burst_produces_repair_pressure() {
        let config = HorizonConfig::default();
        let report = LongHorizonStressRunner::new(config)
            .run(generate_scenario(StressScenario::ToolFailureBurst, 20));
        assert!(report.repair_pause_steps > 0);
        assert!(
            report
                .readings
                .iter()
                .any(|r| r.warnings.iter().any(|w| w.contains("repair pause")))
        );
    }

    #[test]
    fn markdown_report_contains_final_semantic_state() {
        let config = HorizonConfig::default();
        let report = LongHorizonStressRunner::new(config.clone())
            .run(generate_scenario(StressScenario::ProviderLatency, 8));
        let markdown = report.markdown_summary(&config);
        assert!(markdown.contains("Long-Horizon Operational Pressure Report"));
        assert!(markdown.contains("Final Semantic State"));
        assert!(markdown.contains("Recent Readings"));
    }
}
