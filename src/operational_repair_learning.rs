//! Adaptive failure intelligence and operational repair learning.
//!
//! This module turns raw operational failures into compact, replayable repair
//! motifs. It is intentionally deterministic so the TUI, benchmarks, replay
//! gates, and compact prompt memory can share the same interpretation of a
//! failure without depending on model output.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    Build,
    Test,
    Runtime,
    Provider,
    Tooling,
    Auth,
    Network,
    Context,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepairOutcome {
    Succeeded,
    Failed,
    Partial,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayGate {
    Skip,
    Smoke,
    Focused,
    Full,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailureObservation {
    pub id: String,
    pub summary: String,
    pub stderr: String,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub touched_files: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepairAttempt {
    pub observation_id: String,
    pub action: String,
    pub outcome: RepairOutcome,
    pub validation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairMotif {
    pub signature: String,
    pub class: FailureClass,
    pub recurrence: u32,
    pub confidence: f32,
    pub replay_gate: ReplayGate,
    pub preferred_repair: Option<String>,
    pub last_validation: Option<String>,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OperationalRepairMemory {
    motifs: BTreeMap<String, RepairMotif>,
}

impl OperationalRepairMemory {
    pub fn observe_failure(&mut self, observation: &FailureObservation) -> RepairMotif {
        let class = classify_failure(observation);
        let signature = failure_signature(observation, &class);
        let motif = self
            .motifs
            .entry(signature.clone())
            .or_insert_with(|| RepairMotif {
                signature: signature.clone(),
                class: class.clone(),
                recurrence: 0,
                confidence: 0.2,
                replay_gate: ReplayGate::Smoke,
                preferred_repair: None,
                last_validation: None,
                evidence: Vec::new(),
            });

        motif.class = class;
        motif.recurrence = motif.recurrence.saturating_add(1);
        motif.replay_gate = replay_gate_for(&motif.class, motif.recurrence, observation);
        motif.confidence = calibrated_confidence(motif.confidence, motif.recurrence, None);
        push_limited(&mut motif.evidence, compact_evidence(observation), 5);
        motif.clone()
    }

    pub fn record_repair(
        &mut self,
        observation: &FailureObservation,
        repair: &RepairAttempt,
    ) -> RepairMotif {
        let class = classify_failure(observation);
        let signature = failure_signature(observation, &class);
        let motif = self
            .motifs
            .entry(signature.clone())
            .or_insert_with(|| RepairMotif {
                signature: signature.clone(),
                class: class.clone(),
                recurrence: 1,
                confidence: 0.2,
                replay_gate: replay_gate_for(&class, 1, observation),
                preferred_repair: None,
                last_validation: None,
                evidence: vec![compact_evidence(observation)],
            });

        motif.confidence =
            calibrated_confidence(motif.confidence, motif.recurrence, Some(&repair.outcome));
        motif.last_validation = repair.validation.clone();
        if matches!(
            repair.outcome,
            RepairOutcome::Succeeded | RepairOutcome::Partial
        ) {
            motif.preferred_repair = Some(repair.action.clone());
        }
        motif.clone()
    }

    pub fn motifs(&self) -> impl Iterator<Item = &RepairMotif> {
        self.motifs.values()
    }

    pub fn compact_prompt_memory(&self, max_motifs: usize) -> String {
        let mut motifs: Vec<_> = self.motifs.values().collect();
        motifs.sort_by(|a, b| {
            b.recurrence
                .cmp(&a.recurrence)
                .then_with(|| b.confidence.total_cmp(&a.confidence))
                .then_with(|| a.signature.cmp(&b.signature))
        });

        let mut lines = vec!["Operational repair memory:".to_string()];
        for motif in motifs.into_iter().take(max_motifs) {
            let repair = motif
                .preferred_repair
                .as_deref()
                .unwrap_or("no learned repair yet");
            lines.push(format!(
                "- {:?} {} x{} conf {:.2} gate {:?}: {}",
                motif.class,
                motif.signature,
                motif.recurrence,
                motif.confidence,
                motif.replay_gate,
                repair
            ));
        }
        if lines.len() == 1 {
            lines.push("- no recurring repair motifs learned yet".to_string());
        }
        lines.join("\n")
    }
}

pub fn classify_failure(observation: &FailureObservation) -> FailureClass {
    let haystack = format!(
        "{}\n{}\n{}",
        observation.summary,
        observation.stderr,
        observation.command.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();

    if contains_any(
        &haystack,
        &["unauthorized", "forbidden", "401", "403", "api key", "auth"],
    ) {
        FailureClass::Auth
    } else if contains_any(
        &haystack,
        &[
            "timeout",
            "dns",
            "connection refused",
            "network",
            "tls",
            "socket",
        ],
    ) {
        FailureClass::Network
    } else if contains_any(
        &haystack,
        &[
            "provider",
            "rate limit",
            "429",
            "model not found",
            "invalid model",
        ],
    ) {
        FailureClass::Provider
    } else if contains_any(
        &haystack,
        &[
            "test result: failed",
            "assertion failed",
            "panicked at",
            "left:",
            "right:",
        ],
    ) {
        FailureClass::Test
    } else if contains_any(
        &haystack,
        &[
            "error[e",
            "could not compile",
            "cargo check",
            "build failed",
            "linker",
        ],
    ) {
        FailureClass::Build
    } else if contains_any(
        &haystack,
        &[
            "context",
            "token limit",
            "too many tokens",
            "compaction",
            "overflow",
        ],
    ) {
        FailureClass::Context
    } else if contains_any(
        &haystack,
        &[
            "command not found",
            "permission denied",
            "no such file",
            "tool",
            "executable",
        ],
    ) {
        FailureClass::Tooling
    } else if observation.exit_code.unwrap_or(0) != 0 {
        FailureClass::Runtime
    } else {
        FailureClass::Unknown
    }
}

pub fn replay_gate_for(
    class: &FailureClass,
    recurrence: u32,
    observation: &FailureObservation,
) -> ReplayGate {
    if observation.exit_code == Some(0) && matches!(class, FailureClass::Unknown) {
        return ReplayGate::Skip;
    }
    match class {
        FailureClass::Build | FailureClass::Test => {
            if recurrence >= 3 {
                ReplayGate::Full
            } else {
                ReplayGate::Focused
            }
        }
        FailureClass::Runtime | FailureClass::Context => ReplayGate::Focused,
        FailureClass::Provider
        | FailureClass::Network
        | FailureClass::Auth
        | FailureClass::Tooling => ReplayGate::Smoke,
        FailureClass::Unknown => ReplayGate::Smoke,
    }
}

fn calibrated_confidence(previous: f32, recurrence: u32, outcome: Option<&RepairOutcome>) -> f32 {
    let recurrence_boost = (recurrence as f32 * 0.08).min(0.32);
    let outcome_delta = match outcome {
        Some(RepairOutcome::Succeeded) => 0.30,
        Some(RepairOutcome::Partial) => 0.12,
        Some(RepairOutcome::Failed) => -0.18,
        _ => 0.0,
    };
    (previous + recurrence_boost + outcome_delta).clamp(0.05, 0.95)
}

fn failure_signature(observation: &FailureObservation, class: &FailureClass) -> String {
    let command = observation
        .command
        .as_deref()
        .and_then(|cmd| cmd.split_whitespace().next())
        .unwrap_or("unknown");
    let file_hint = observation
        .touched_files
        .first()
        .map(|path| path.rsplit('/').next().unwrap_or(path))
        .unwrap_or("no-file");
    let phrase = first_signal_phrase(&observation.stderr)
        .unwrap_or_else(|| first_signal_phrase(&observation.summary).unwrap_or("unspecified"));
    format!("{:?}:{}:{}:{}", class, command, file_hint, phrase)
}

fn first_signal_phrase(text: &str) -> Option<&str> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("warning:"))
        .map(|line| line.split_at(line.len().min(80)).0)
}

fn compact_evidence(observation: &FailureObservation) -> String {
    let command = observation.command.as_deref().unwrap_or("unknown command");
    let signal = first_signal_phrase(&observation.stderr)
        .or_else(|| first_signal_phrase(&observation.summary))
        .unwrap_or("no signal");
    format!("{} -> {}", command, signal)
}

fn push_limited(values: &mut Vec<String>, value: String, max: usize) {
    if values.last() != Some(&value) {
        values.push(value);
    }
    while values.len() > max {
        values.remove(0);
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(stderr: &str, command: &str) -> FailureObservation {
        FailureObservation {
            id: "obs-1".into(),
            summary: "failed".into(),
            stderr: stderr.into(),
            command: Some(command.into()),
            exit_code: Some(101),
            touched_files: vec!["src/lib.rs".into()],
        }
    }

    #[test]
    fn classifies_build_and_test_failures() {
        assert_eq!(
            classify_failure(&obs("error[E0425]: cannot find value", "cargo check")),
            FailureClass::Build
        );
        assert_eq!(
            classify_failure(&obs("thread panicked at assertion failed", "cargo test")),
            FailureClass::Test
        );
    }

    #[test]
    fn recurrence_escalates_replay_gate() {
        let mut memory = OperationalRepairMemory::default();
        let observation = obs("error[E0425]: cannot find value", "cargo check");
        assert_eq!(
            memory.observe_failure(&observation).replay_gate,
            ReplayGate::Focused
        );
        memory.observe_failure(&observation);
        assert_eq!(
            memory.observe_failure(&observation).replay_gate,
            ReplayGate::Full
        );
    }

    #[test]
    fn successful_repair_becomes_prompt_memory() {
        let mut memory = OperationalRepairMemory::default();
        let observation = obs("model not found", "neura run");
        memory.observe_failure(&observation);
        memory.record_repair(
            &observation,
            &RepairAttempt {
                observation_id: observation.id.clone(),
                action: "refresh model catalog and retry selected fallback".into(),
                outcome: RepairOutcome::Succeeded,
                validation: Some("smoke retry passed".into()),
            },
        );
        let prompt = memory.compact_prompt_memory(4);
        assert!(prompt.contains("Operational repair memory"));
        assert!(prompt.contains("refresh model catalog"));
        assert!(prompt.contains("Provider"));
    }
}
