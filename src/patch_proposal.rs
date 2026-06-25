use crate::autonomous_improvement::{
    EvidenceRankedTask, load_or_synthesize_evidence_tasks, tiny_patch_gate,
};
use crate::evidence_ledger::{EvidenceKind, append_evidence_with_links};
use crate::evidence_replay::{ReplayConfig, run_replay};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchProposal {
    pub id: String,
    pub task_id: String,
    pub title: String,
    pub created_at_ms: u128,
    pub patch_text: String,
    pub files_touched: usize,
    pub estimated_changed_lines: usize,
    pub evidence: Vec<String>,
    pub replay_before: f64,
    pub replay_after_estimate: f64,
    pub replay_delta: f64,
    pub dry_run_allowed: bool,
    pub mutation_allowed: bool,
    pub ledger_receipt_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchValidation {
    pub proposal_id: String,
    pub passed: bool,
    pub checks: Vec<PatchValidationCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchValidationCheck {
    pub name: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub output_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPromotionGate {
    pub proposal_id: String,
    pub allowed: bool,
    pub replay_delta: f64,
    pub validation_passed: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchProposalReport {
    pub created_at_ms: u128,
    pub proposal: PatchProposal,
    pub validation: PatchValidation,
    pub promotion_gate: PatchPromotionGate,
    pub summary: String,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn patch_proposal_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".neura")
        .join("patch_proposal_report.json")
}

pub fn propose_patch(task_selector: Option<&str>) -> Result<PatchProposal> {
    let tasks = load_or_synthesize_evidence_tasks()?;
    let task = select_task(&tasks.tasks, task_selector)?;
    let gate = tiny_patch_gate(task, true, false);
    let replay = run_replay(ReplayConfig {
        limit: 25,
        include_alternatives: true,
        max_index: None,
        subject: None,
    })?;
    let replay_before = replay.mean_replay_score;
    let replay_after_estimate = estimate_after_score(task, replay_before);
    let replay_delta = replay_after_estimate - replay_before;
    let patch_text = synthesize_patch_text(task);
    let mut proposal = PatchProposal {
        id: format!("proposal-{}-{}", task.id, now_ms()),
        task_id: task.id.clone(),
        title: task.title.clone(),
        created_at_ms: now_ms(),
        patch_text,
        files_touched: task.tiny_patch.files_touched,
        estimated_changed_lines: task.tiny_patch.estimated_changed_lines,
        evidence: task.evidence.clone(),
        replay_before,
        replay_after_estimate,
        replay_delta,
        dry_run_allowed: !gate.allowed,
        mutation_allowed: false,
        ledger_receipt_hash: None,
    };
    let receipt = append_evidence_with_links(
        EvidenceKind::PromotionDecision,
        format!("patch-proposal:{}", proposal.id),
        format!(
            "patch proposal synthesized for task {} with replay delta {:.3}",
            proposal.task_id, proposal.replay_delta
        ),
        Some(proposal.replay_delta),
        Some(proposal.replay_delta > 0.0),
        &proposal,
        vec![],
        vec![],
        "patch-proposal",
    )?;
    proposal.ledger_receipt_hash = Some(receipt.receipt.hash);
    Ok(proposal)
}

fn select_task<'a>(
    tasks: &'a [EvidenceRankedTask],
    selector: Option<&str>,
) -> Result<&'a EvidenceRankedTask> {
    if tasks.is_empty() {
        return Err(anyhow!(
            "no evidence-ranked self-improvement tasks available"
        ));
    }
    match selector.unwrap_or("top") {
        "top" => Ok(&tasks[0]),
        id => tasks
            .iter()
            .find(|task| task.id == id)
            .ok_or_else(|| anyhow!("task selector not found: {id}")),
    }
}

fn estimate_after_score(task: &EvidenceRankedTask, before: f64) -> f64 {
    let uplift = (task.expected_utility * task.confidence * (1.0 - task.risk)).clamp(0.0, 0.12);
    (before + uplift).clamp(0.0, 1.0)
}

fn synthesize_patch_text(task: &EvidenceRankedTask) -> String {
    format!(
        "# Dry-run patch proposal for `{}`\n\nIntent: {}\n\nPlanned tiny patch:\n- {}\n- files_touched={}\n- estimated_changed_lines={}\n\nThis proposal is intentionally report-only until the replay, validation, and promotion gates pass.\n",
        task.id,
        task.title,
        task.tiny_patch.summary,
        task.tiny_patch.files_touched,
        task.tiny_patch.estimated_changed_lines,
    )
}

pub fn dry_run_patch(task_selector: Option<&str>) -> Result<PatchProposal> {
    propose_patch(task_selector)
}

pub fn validate_patch(proposal: &PatchProposal) -> PatchValidation {
    let checks = vec![
        validation_check(
            "cargo check --bin neura",
            &["cargo", "check", "--bin", "neura"],
        ),
        validation_check(
            "evidence replay tests",
            &[
                "cargo",
                "test",
                "--test",
                "evidence_replay_tests",
                "--",
                "--test-threads=1",
            ],
        ),
        validation_check(
            "evidence ledger tests",
            &[
                "cargo",
                "test",
                "--test",
                "evidence_ledger_tests",
                "--",
                "--test-threads=1",
            ],
        ),
    ];
    PatchValidation {
        proposal_id: proposal.id.clone(),
        passed: checks.iter().all(|check| check.passed),
        checks,
    }
}

fn validation_check(name: &str, argv: &[&str]) -> PatchValidationCheck {
    match Command::new(argv[0]).args(&argv[1..]).output() {
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            PatchValidationCheck {
                name: name.to_string(),
                passed: output.status.success(),
                exit_code: output.status.code(),
                output_preview: preview(&text),
            }
        }
        Err(e) => PatchValidationCheck {
            name: name.to_string(),
            passed: false,
            exit_code: None,
            output_preview: format!("failed to start validation: {e}"),
        },
    }
}

fn preview(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().rev().take(12).collect();
    lines.reverse();
    lines.join("\n")
}

pub fn promotion_gate(
    proposal: &PatchProposal,
    validation: &PatchValidation,
) -> PatchPromotionGate {
    let mut reasons = Vec::new();
    if proposal.files_touched > 2 {
        reasons.push("proposal touches too many files".to_string());
    }
    if proposal.estimated_changed_lines > 40 {
        reasons.push("proposal exceeds tiny patch line threshold".to_string());
    }
    if proposal.replay_delta <= 0.01 {
        reasons.push(format!(
            "replay delta {:.3} is too small",
            proposal.replay_delta
        ));
    }
    if !validation.passed {
        reasons.push("validation did not pass".to_string());
    }
    reasons.push("mutation remains disabled by default; user-visible report required".to_string());
    PatchPromotionGate {
        proposal_id: proposal.id.clone(),
        allowed: false,
        replay_delta: proposal.replay_delta,
        validation_passed: validation.passed,
        reasons,
    }
}

pub fn build_patch_report(
    task_selector: Option<&str>,
    validate: bool,
) -> Result<PatchProposalReport> {
    let proposal = propose_patch(task_selector)?;
    let validation = if validate {
        validate_patch(&proposal)
    } else {
        PatchValidation {
            proposal_id: proposal.id.clone(),
            passed: false,
            checks: vec![PatchValidationCheck {
                name: "validation skipped".to_string(),
                passed: false,
                exit_code: None,
                output_preview: "run patch-validate for full validation".to_string(),
            }],
        }
    };
    let promotion_gate = promotion_gate(&proposal, &validation);
    let summary = if promotion_gate.allowed {
        "patch proposal passed promotion gate".to_string()
    } else {
        "patch proposal remains dry-run gated".to_string()
    };
    let report = PatchProposalReport {
        created_at_ms: now_ms(),
        proposal,
        validation,
        promotion_gate,
        summary,
    };
    let path = patch_proposal_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

pub fn render_patch_report(report: &PatchProposalReport) -> String {
    let mut out = String::new();
    out.push_str("# Neura Replay-Gated Patch Proposal Report\n\n");
    out.push_str(&format!("- Summary: `{}`\n", report.summary));
    out.push_str(&format!("- Proposal: `{}`\n", report.proposal.id));
    out.push_str(&format!("- Task: `{}`\n", report.proposal.task_id));
    out.push_str(&format!(
        "- Replay before: `{:.3}`\n",
        report.proposal.replay_before
    ));
    out.push_str(&format!(
        "- Replay after estimate: `{:.3}`\n",
        report.proposal.replay_after_estimate
    ));
    out.push_str(&format!(
        "- Replay delta: `{:.3}`\n",
        report.proposal.replay_delta
    ));
    out.push_str(&format!(
        "- Promotion allowed: `{}`\n\n",
        report.promotion_gate.allowed
    ));
    out.push_str("## Patch Text\n\n```text\n");
    out.push_str(&report.proposal.patch_text);
    out.push_str("\n```\n\n## Evidence\n\n");
    for evidence in &report.proposal.evidence {
        out.push_str(&format!("- {evidence}\n"));
    }
    out.push_str("\n## Validation\n\n");
    for check in &report.validation.checks {
        out.push_str(&format!(
            "- `{}` passed=`{}` exit=`{:?}`\n",
            check.name, check.passed, check.exit_code
        ));
    }
    out.push_str("\n## Promotion Gate\n\n");
    for reason in &report.promotion_gate.reasons {
        out.push_str(&format!("- {reason}\n"));
    }
    out
}

pub fn write_patch_report(output: Option<PathBuf>, validate: bool) -> Result<PathBuf> {
    let report = build_patch_report(Some("top"), validate)?;
    let path = output.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Desktop")
            .join("patch_proposal_report.md")
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_patch_report(&report))?;
    Ok(path)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PatchPipelineStage {
    Proposed,
    DryRunGenerated,
    ReplayScored,
    Validated,
    PromotionGated,
    RollbackPlanned,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchRollbackPlan {
    pub proposal_id: String,
    pub reversible: bool,
    pub steps: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchPipelineRun {
    pub id: String,
    pub task_selector: String,
    pub created_at_ms: u128,
    pub stages: Vec<PatchPipelineStage>,
    pub proposal: PatchProposal,
    pub validation: PatchValidation,
    pub promotion_gate: PatchPromotionGate,
    pub rollback_plan: PatchRollbackPlan,
    pub replay_regression_detected: bool,
    pub blocked: bool,
    pub ledger_receipts: Vec<String>,
    pub summary: String,
}

pub fn patch_pipeline_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".neura")
        .join("self_improve_patch_pipeline.json")
}

pub fn run_patch_pipeline(task_selector: Option<&str>, validate: bool) -> Result<PatchPipelineRun> {
    let selector = task_selector.unwrap_or("top").to_string();
    let mut stages = vec![PatchPipelineStage::Proposed];
    let proposal = propose_patch(Some(&selector))?;
    stages.push(PatchPipelineStage::DryRunGenerated);
    stages.push(PatchPipelineStage::ReplayScored);

    let validation = if validate {
        let validation = validate_patch(&proposal);
        stages.push(PatchPipelineStage::Validated);
        validation
    } else {
        PatchValidation {
            proposal_id: proposal.id.clone(),
            passed: false,
            checks: vec![PatchValidationCheck {
                name: "validation skipped".to_string(),
                passed: false,
                exit_code: None,
                output_preview: "pipeline ran in dry validation mode".to_string(),
            }],
        }
    };

    let promotion_gate = promotion_gate(&proposal, &validation);
    stages.push(PatchPipelineStage::PromotionGated);
    let replay_regression_detected = proposal.replay_delta < 0.0;
    let rollback_plan = rollback_plan_for(&proposal, &promotion_gate);
    stages.push(PatchPipelineStage::RollbackPlanned);
    let blocked = !promotion_gate.allowed || replay_regression_detected;
    if blocked {
        stages.push(PatchPipelineStage::Blocked);
    }

    let mut ledger_receipts = proposal
        .ledger_receipt_hash
        .clone()
        .map(|hash| vec![hash])
        .unwrap_or_default();
    if let Ok(receipt) = append_evidence_with_links(
        EvidenceKind::PromotionDecision,
        format!("patch-pipeline:{}", proposal.id),
        format!(
            "pipeline blocked={} replay_delta={:.3} validation_passed={}",
            blocked, proposal.replay_delta, validation.passed
        ),
        Some(proposal.replay_delta),
        Some(!blocked),
        &promotion_gate,
        ledger_receipts.clone(),
        ledger_receipts.clone(),
        "patch-pipeline",
    ) {
        ledger_receipts.push(receipt.receipt.hash);
    }

    let summary = if blocked {
        "replay-scored patch pipeline completed with mutation blocked".to_string()
    } else {
        "replay-scored patch pipeline passed all gates".to_string()
    };
    let run = PatchPipelineRun {
        id: format!("pipeline-{}", now_ms()),
        task_selector: selector,
        created_at_ms: now_ms(),
        stages,
        proposal,
        validation,
        promotion_gate,
        rollback_plan,
        replay_regression_detected,
        blocked,
        ledger_receipts,
        summary,
    };
    let path = patch_pipeline_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&run)?)?;
    Ok(run)
}

fn rollback_plan_for(proposal: &PatchProposal, gate: &PatchPromotionGate) -> PatchRollbackPlan {
    PatchRollbackPlan {
        proposal_id: proposal.id.clone(),
        reversible: true,
        steps: vec![
            "do not apply mutation while gate is blocked".to_string(),
            "keep patch text as report-only artifact".to_string(),
            "re-run replay and validation before any future promotion".to_string(),
            "restore previous ledger-verified runtime if a promoted patch regresses".to_string(),
        ],
        reason: if gate.allowed {
            "rollback plan prepared before promotion".to_string()
        } else {
            format!(
                "rollback not needed because promotion is blocked: {}",
                gate.reasons.join("; ")
            )
        },
    }
}

pub fn render_patch_pipeline(run: &PatchPipelineRun) -> String {
    let mut out = String::new();
    out.push_str("# Neura Replay-Scored Self-Improvement Patch Pipeline\n\n");
    out.push_str(&format!("- Summary: `{}`\n", run.summary));
    out.push_str(&format!("- Pipeline: `{}`\n", run.id));
    out.push_str(&format!("- Task selector: `{}`\n", run.task_selector));
    out.push_str(&format!("- Blocked: `{}`\n", run.blocked));
    out.push_str(&format!(
        "- Replay regression detected: `{}`\n",
        run.replay_regression_detected
    ));
    out.push_str(&format!(
        "- Replay delta: `{:.3}`\n",
        run.proposal.replay_delta
    ));
    out.push_str(&format!(
        "- Validation passed: `{}`\n",
        run.validation.passed
    ));
    out.push_str(&format!(
        "- Promotion allowed: `{}`\n\n",
        run.promotion_gate.allowed
    ));
    out.push_str("## Stages\n\n");
    for stage in &run.stages {
        out.push_str(&format!("- `{:?}`\n", stage));
    }
    out.push_str("\n## Proposal Patch\n\n```text\n");
    out.push_str(&run.proposal.patch_text);
    out.push_str("\n```\n\n## Promotion Gate Reasons\n\n");
    for reason in &run.promotion_gate.reasons {
        out.push_str(&format!("- {reason}\n"));
    }
    out.push_str("\n## Rollback Plan\n\n");
    out.push_str(&format!(
        "- Reversible: `{}`\n",
        run.rollback_plan.reversible
    ));
    out.push_str(&format!("- Reason: {}\n", run.rollback_plan.reason));
    for step in &run.rollback_plan.steps {
        out.push_str(&format!("- {step}\n"));
    }
    out.push_str("\n## Ledger Receipts\n\n");
    for hash in &run.ledger_receipts {
        out.push_str(&format!("- `{hash}`\n"));
    }
    out
}

pub fn write_patch_pipeline_report(
    output: Option<PathBuf>,
    task_selector: Option<&str>,
    validate: bool,
) -> Result<PathBuf> {
    let run = run_patch_pipeline(task_selector, validate)?;
    let path = output.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Desktop")
            .join("self_improve_patch_pipeline.md")
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_patch_pipeline(&run))?;
    Ok(path)
}
