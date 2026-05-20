use crate::latent_learning::{LatentLearningState, LearningStep, learning_state_path};
use crate::latent_operational_recurrence::{LatentOperationalState, OperationalEvent, state_path};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

pub const BACKGROUND_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeLearningSample {
    pub id: String,
    pub event: OperationalEvent,
    pub source: String,
    pub captured_at_ms: u64,
    pub consumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundLearningStatus {
    pub schema_version: u32,
    pub paused: bool,
    pub total_samples: usize,
    pub pending_samples: usize,
    pub consumed_samples: usize,
    pub last_cycle_ms: Option<u64>,
    pub last_cycle_consumed: usize,
    pub state_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundCycleResult {
    pub consumed: usize,
    pub skipped: usize,
    pub immune_rejections: usize,
    pub learning_steps: Vec<LearningStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeSummary {
    pub total: usize,
    pub success: usize,
    pub failure: usize,
    pub other: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DoctrineSummary {
    pub learned_bindings: usize,
    pub topology_edges: usize,
    pub immune_responses: usize,
    pub convergence_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ControlState {
    schema_version: u32,
    paused: bool,
    last_cycle_ms: Option<u64>,
    last_cycle_consumed: usize,
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            schema_version: BACKGROUND_SCHEMA_VERSION,
            paused: false,
            last_cycle_ms: None,
            last_cycle_consumed: 0,
        }
    }
}

pub fn learning_dir() -> PathBuf {
    std::env::var_os("KCODE_LATENT_LEARNING_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".kcode").join("latent_learning")
        })
}

pub fn ingest_runtime_event(
    mut event: OperationalEvent,
    source: impl Into<String>,
) -> anyhow::Result<RuntimeLearningSample> {
    if event.timestamp_ms == 0 {
        event.timestamp_ms = crate::latent_operational_recurrence::now_ms();
    }
    let sample = RuntimeLearningSample {
        id: sample_id(&event),
        captured_at_ms: crate::latent_operational_recurrence::now_ms(),
        source: source.into(),
        event,
        consumed: false,
    };
    append_sample(&sample)?;
    Ok(sample)
}

pub fn command_event(
    kind: impl Into<String>,
    outcome: impl Into<String>,
    tags: Vec<String>,
    tool: Option<String>,
) -> OperationalEvent {
    let mut event = OperationalEvent::new(kind, outcome);
    event.tags = tags;
    event.tool = tool;
    event
}

pub fn run_background_cycle(limit: usize) -> anyhow::Result<BackgroundCycleResult> {
    let mut control = load_control()?;
    if control.paused {
        return Ok(BackgroundCycleResult {
            consumed: 0,
            skipped: 0,
            immune_rejections: 0,
            learning_steps: Vec::new(),
        });
    }

    let mut samples = load_samples()?;
    let mut recurrence = LatentOperationalState::load_or_default(&state_path())?;
    let mut learning = LatentLearningState::load_or_default(&learning_state_path())?;
    let mut result = BackgroundCycleResult {
        consumed: 0,
        skipped: 0,
        immune_rejections: 0,
        learning_steps: Vec::new(),
    };

    for sample in samples.iter_mut().filter(|s| !s.consumed).take(limit) {
        let gate = recurrence.observe(sample.event.clone());
        if !gate.accepted {
            result.skipped += 1;
            sample.consumed = true;
            continue;
        }
        let step = learning.learn(&recurrence, sample.event.clone());
        if step.immune.triggered {
            result.immune_rejections += 1;
        }
        result.learning_steps.push(step);
        sample.consumed = true;
        result.consumed += 1;
    }

    recurrence.save(&state_path())?;
    learning.save(&learning_state_path())?;
    save_samples(&samples)?;
    control.last_cycle_ms = Some(crate::latent_operational_recurrence::now_ms());
    control.last_cycle_consumed = result.consumed;
    save_control(&control)?;
    Ok(result)
}

pub fn status() -> anyhow::Result<BackgroundLearningStatus> {
    let control = load_control()?;
    let samples = load_samples()?;
    let consumed = samples.iter().filter(|s| s.consumed).count();
    Ok(BackgroundLearningStatus {
        schema_version: BACKGROUND_SCHEMA_VERSION,
        paused: control.paused,
        total_samples: samples.len(),
        pending_samples: samples.len().saturating_sub(consumed),
        consumed_samples: consumed,
        last_cycle_ms: control.last_cycle_ms,
        last_cycle_consumed: control.last_cycle_consumed,
        state_dir: learning_dir(),
    })
}

pub fn samples() -> anyhow::Result<Vec<RuntimeLearningSample>> {
    load_samples()
}

pub fn outcome_summary() -> anyhow::Result<OutcomeSummary> {
    let samples = load_samples()?;
    let mut out = OutcomeSummary {
        total: samples.len(),
        success: 0,
        failure: 0,
        other: 0,
    };
    for sample in samples {
        match sample.event.outcome.as_str() {
            "success" | "ok" | "passed" | "complete" => out.success += 1,
            "failure" | "error" | "failed" | "blocked" => out.failure += 1,
            _ => out.other += 1,
        }
    }
    Ok(out)
}

pub fn doctrine_summary() -> anyhow::Result<DoctrineSummary> {
    let learning = LatentLearningState::load_or_default(&learning_state_path())?;
    Ok(DoctrineSummary {
        learned_bindings: learning.doctrine_bindings.len(),
        topology_edges: learning.topology.len(),
        immune_responses: learning.immune_history.len(),
        convergence_score: learning
            .last_convergence
            .map(|m| m.convergence_score)
            .unwrap_or(0.0),
    })
}

pub fn set_paused(paused: bool) -> anyhow::Result<BackgroundLearningStatus> {
    let mut control = load_control()?;
    control.paused = paused;
    save_control(&control)?;
    status()
}

fn append_sample(sample: &RuntimeLearningSample) -> anyhow::Result<()> {
    fs::create_dir_all(learning_dir())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(samples_path())?;
    writeln!(file, "{}", serde_json::to_string(sample)?)?;
    Ok(())
}

fn load_samples() -> anyhow::Result<Vec<RuntimeLearningSample>> {
    let path = samples_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut samples = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        samples.push(serde_json::from_str(&line)?);
    }
    Ok(samples)
}

fn save_samples(samples: &[RuntimeLearningSample]) -> anyhow::Result<()> {
    fs::create_dir_all(learning_dir())?;
    let mut file = fs::File::create(samples_path())?;
    for sample in samples {
        writeln!(file, "{}", serde_json::to_string(sample)?)?;
    }
    Ok(())
}

fn load_control() -> anyhow::Result<ControlState> {
    let path = control_path();
    if path.exists() {
        Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
    } else {
        Ok(ControlState::default())
    }
}

fn save_control(control: &ControlState) -> anyhow::Result<()> {
    fs::create_dir_all(learning_dir())?;
    fs::write(control_path(), serde_json::to_string_pretty(control)?)?;
    Ok(())
}

fn samples_path() -> PathBuf {
    learning_dir().join("samples.jsonl")
}
fn control_path() -> PathBuf {
    learning_dir().join("background_control.json")
}

fn sample_id(event: &OperationalEvent) -> String {
    format!(
        "{}-{}-{}",
        event.timestamp_ms,
        sanitize(&event.kind),
        sanitize(&event.outcome)
    )
}
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ingests_and_cycles_sample() {
        let dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("KCODE_LATENT_LEARNING_DIR", dir.path()) };
        unsafe { std::env::set_var("KCODE_LATENT_STATE", dir.path().join("latent.json")) };
        unsafe {
            std::env::set_var(
                "KCODE_LATENT_LEARNING_STATE",
                dir.path().join("learning.json"),
            )
        };
        ingest_runtime_event(
            command_event(
                "build",
                "success",
                vec!["test".into(), "validation".into()],
                Some("cargo".into()),
            ),
            "test",
        )
        .unwrap();
        assert_eq!(status().unwrap().pending_samples, 1);
        let result = run_background_cycle(8).unwrap();
        assert_eq!(result.consumed, 1);
        assert_eq!(status().unwrap().pending_samples, 0);
    }

    #[test]
    fn pause_blocks_cycle_consumption() {
        let dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("KCODE_LATENT_LEARNING_DIR", dir.path()) };
        unsafe { std::env::set_var("KCODE_LATENT_STATE", dir.path().join("latent.json")) };
        unsafe {
            std::env::set_var(
                "KCODE_LATENT_LEARNING_STATE",
                dir.path().join("learning.json"),
            )
        };
        ingest_runtime_event(
            command_event("build", "success", vec!["test".into()], None),
            "test",
        )
        .unwrap();
        set_paused(true).unwrap();
        assert_eq!(run_background_cycle(8).unwrap().consumed, 0);
        assert_eq!(status().unwrap().pending_samples, 1);
    }
}
