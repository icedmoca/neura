use crate::adversarial_eval::run_adversarial_eval_suite;
use crate::operational_eval::run_operational_eval_suite;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementConfig {
    pub max_iterations: usize,
    pub dry_run: bool,
    pub allow_mutation: bool,
    pub min_operational_score: f64,
    pub min_adversarial_score: f64,
}

impl Default for ImprovementConfig {
    fn default() -> Self {
        Self {
            max_iterations: 1,
            dry_run: true,
            allow_mutation: false,
            min_operational_score: 0.75,
            min_adversarial_score: 0.85,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementIteration {
    pub iteration: usize,
    pub started_at_ms: u128,
    pub operational_score: f64,
    pub operational_passed: bool,
    pub adversarial_score: f64,
    pub adversarial_passed: bool,
    pub candidate_actions: Vec<String>,
    pub applied_actions: Vec<String>,
    pub blocked_actions: Vec<String>,
    pub validation: Vec<ValidationResult>,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub name: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub output_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfImprovementReport {
    pub created_at_ms: u128,
    pub config: ImprovementConfig,
    pub iterations: Vec<ImprovementIteration>,
    pub passed: bool,
    pub summary: String,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn self_improvement_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kcode")
        .join("self_improvement_report.json")
}

pub fn run_self_improvement_cycle(config: ImprovementConfig) -> Result<SelfImprovementReport> {
    let bounded_iterations = config.max_iterations.clamp(1, 5);
    let mut iterations = Vec::new();

    for idx in 0..bounded_iterations {
        let op = run_operational_eval_suite()?;
        let adv = run_adversarial_eval_suite()?;
        let candidate_actions = synthesize_candidate_actions(op.mean_score, adv.mean_score);
        let mut applied_actions = Vec::new();
        let mut blocked_actions = Vec::new();

        let gates_pass = op.passed
            && adv.passed
            && op.mean_score >= config.min_operational_score
            && adv.mean_score >= config.min_adversarial_score;

        for action in &candidate_actions {
            if config.allow_mutation && !config.dry_run && gates_pass && is_safe_action(action) {
                applied_actions.push(action.clone());
            } else {
                blocked_actions.push(format!(
                    "{action} [blocked: dry_run={}, allow_mutation={}, gates_pass={}]",
                    config.dry_run, config.allow_mutation, gates_pass
                ));
            }
        }

        let validation = run_internal_validations();
        let validations_pass = validation.iter().all(|v| v.passed);
        let decision = if gates_pass && validations_pass && !applied_actions.is_empty() {
            "bounded safe mutation accepted".to_string()
        } else if gates_pass && validations_pass {
            "dry-run improvement candidates validated; no mutation applied".to_string()
        } else {
            "self-improvement blocked by eval or validation gate".to_string()
        };

        iterations.push(ImprovementIteration {
            iteration: idx + 1,
            started_at_ms: now_ms(),
            operational_score: op.mean_score,
            operational_passed: op.passed,
            adversarial_score: adv.mean_score,
            adversarial_passed: adv.passed,
            candidate_actions,
            applied_actions,
            blocked_actions,
            validation,
            decision,
        });
    }

    let passed = iterations.iter().all(|it| {
        it.operational_passed
            && it.adversarial_passed
            && it.validation.iter().all(|v| v.passed)
            && it.applied_actions.is_empty()
    });
    let summary = if passed {
        "bounded autonomous self-improvement completed in safe dry-run mode".to_string()
    } else {
        "bounded autonomous self-improvement found blockers; mutation remained disabled".to_string()
    };
    let report = SelfImprovementReport {
        created_at_ms: now_ms(),
        config: ImprovementConfig {
            max_iterations: bounded_iterations,
            ..config
        },
        iterations,
        passed,
        summary,
    };
    let path = self_improvement_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

fn synthesize_candidate_actions(operational_score: f64, adversarial_score: f64) -> Vec<String> {
    let mut actions = vec![
        "refresh operational eval report".to_string(),
        "refresh adversarial eval report".to_string(),
        "record self-improvement attribution snapshot".to_string(),
    ];
    if operational_score < 0.90 {
        actions.push("investigate operational score regression before promotion".to_string());
    }
    if adversarial_score < 0.95 {
        actions.push("expand adversarial fixtures around weakest class".to_string());
    }
    actions
}

fn is_safe_action(action: &str) -> bool {
    action.contains("refresh") || action.contains("attribution snapshot")
}

fn run_internal_validations() -> Vec<ValidationResult> {
    vec![
        run_validation(
            "cargo check --bin kcode",
            &["cargo", "check", "--bin", "kcode"],
        ),
        run_validation(
            "operational eval smoke",
            &[
                "cargo",
                "test",
                "--test",
                "operational_eval_tests",
                "--",
                "--test-threads=1",
            ],
        ),
        run_validation(
            "adversarial eval smoke",
            &[
                "cargo",
                "test",
                "--test",
                "adversarial_eval_tests",
                "--",
                "--test-threads=1",
            ],
        ),
    ]
}

fn run_validation(name: &str, argv: &[&str]) -> ValidationResult {
    if argv.is_empty() {
        return ValidationResult {
            name: name.to_string(),
            passed: false,
            exit_code: None,
            output_preview: "empty validation command".to_string(),
        };
    }
    let output = Command::new(argv[0]).args(&argv[1..]).output();
    match output {
        Ok(out) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&out.stdout));
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            ValidationResult {
                name: name.to_string(),
                passed: out.status.success(),
                exit_code: out.status.code(),
                output_preview: preview(&text),
            }
        }
        Err(e) => ValidationResult {
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

pub fn load_or_run_self_improvement_report() -> Result<SelfImprovementReport> {
    let path = self_improvement_path();
    if path.exists() {
        let bytes = fs::read(&path)?;
        serde_json::from_slice(&bytes)
            .or_else(|_| run_self_improvement_cycle(ImprovementConfig::default()))
    } else {
        run_self_improvement_cycle(ImprovementConfig::default())
    }
}

pub fn render_self_improvement_report(report: &SelfImprovementReport) -> String {
    let mut out = String::new();
    out.push_str("# Kcode Autonomous Self-Improvement Report\n\n");
    out.push_str(&format!("- Passed: `{}`\n", report.passed));
    out.push_str(&format!("- Summary: `{}`\n", report.summary));
    out.push_str(&format!("- Dry run: `{}`\n", report.config.dry_run));
    out.push_str(&format!(
        "- Mutation allowed: `{}`\n",
        report.config.allow_mutation
    ));
    out.push_str(&format!("- Iterations: `{}`\n\n", report.iterations.len()));
    out.push_str("## Iterations\n\n");
    for it in &report.iterations {
        out.push_str(&format!(
            "### Iteration {}\n\n- operational: passed=`{}` score=`{:.3}`\n- adversarial: passed=`{}` score=`{:.3}`\n- decision: `{}`\n\n",
            it.iteration,
            it.operational_passed,
            it.operational_score,
            it.adversarial_passed,
            it.adversarial_score,
            it.decision
        ));
        out.push_str("Candidate actions:\n");
        for action in &it.candidate_actions {
            out.push_str(&format!("- {action}\n"));
        }
        out.push_str("\nBlocked actions:\n");
        for action in &it.blocked_actions {
            out.push_str(&format!("- {action}\n"));
        }
        out.push_str("\nValidation:\n");
        for validation in &it.validation {
            out.push_str(&format!(
                "- `{}` passed=`{}` exit=`{:?}`\n",
                validation.name, validation.passed, validation.exit_code
            ));
        }
        out.push('\n');
    }
    out.push_str("## Safety Model\n\nThis scheduler is bounded, dry-run by default, mutation-disabled by default, and requires operational eval, adversarial eval, and internal validation to pass before any safe action could be applied.\n");
    out
}

pub fn write_self_improvement_markdown(output: Option<PathBuf>) -> Result<PathBuf> {
    let report = load_or_run_self_improvement_report()?;
    let path = output.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Desktop")
            .join("self_improvement_report.md")
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_self_improvement_report(&report))?;
    Ok(path)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRankedTask {
    pub id: String,
    pub title: String,
    pub evidence: Vec<String>,
    pub expected_utility: f64,
    pub risk: f64,
    pub confidence: f64,
    pub rank_score: f64,
    pub tiny_patch: TinyPatchPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinyPatchPlan {
    pub summary: String,
    pub files_touched: usize,
    pub estimated_changed_lines: usize,
    pub requires_user_confirmation: bool,
    pub mutation_safe: bool,
    pub gate_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinyPatchGateDecision {
    pub task_id: String,
    pub allowed: bool,
    pub dry_run: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRankedTaskReport {
    pub created_at_ms: u128,
    pub tasks: Vec<EvidenceRankedTask>,
    pub gate_decisions: Vec<TinyPatchGateDecision>,
    pub mutation_enabled: bool,
    pub summary: String,
}

pub fn evidence_ranked_tasks_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kcode")
        .join("evidence_ranked_self_improvement_tasks.json")
}

pub fn synthesize_evidence_ranked_tasks() -> Result<EvidenceRankedTaskReport> {
    let op = run_operational_eval_suite()?;
    let adv = run_adversarial_eval_suite()?;
    let self_report = load_or_run_self_improvement_report()?;
    let mut tasks = Vec::new();

    tasks.push(build_task(
        "task-tighten-validation-preview",
        "Improve validation output attribution and previews",
        vec![
            format!("operational_mean={:.3}", op.mean_score),
            format!("self_iterations={}", self_report.iterations.len()),
            "validation previews are already captured but not ranked by failing subsystem"
                .to_string(),
        ],
        0.74,
        0.18,
        0.82,
        TinyPatchPlan {
            summary: "Add subsystem labels to validation previews".to_string(),
            files_touched: 1,
            estimated_changed_lines: 18,
            requires_user_confirmation: false,
            mutation_safe: true,
            gate_reason: "single file, small line count, reporting-only".to_string(),
        },
    ));

    tasks.push(build_task(
        "task-expand-adversarial-fixtures",
        "Expand adversarial fixtures around promotion and memory poisoning",
        vec![
            format!("adversarial_mean={:.3}", adv.mean_score),
            "adversarial suite is passing, but future regressions need more fixture diversity"
                .to_string(),
        ],
        0.86,
        0.22,
        0.88,
        TinyPatchPlan {
            summary: "Add one tiny adversarial case or report-only fixture".to_string(),
            files_touched: 1,
            estimated_changed_lines: 24,
            requires_user_confirmation: false,
            mutation_safe: true,
            gate_reason: "test/report-only patch under tiny threshold".to_string(),
        },
    ));

    tasks.push(build_task(
        "task-autonomous-mutation-observer",
        "Add observer telemetry for blocked mutation attempts",
        vec![
            "self-improvement currently records blocked actions".to_string(),
            "blocked-action telemetry can improve future ranking evidence".to_string(),
        ],
        0.70,
        0.30,
        0.75,
        TinyPatchPlan {
            summary: "Record blocked mutation counts in report".to_string(),
            files_touched: 1,
            estimated_changed_lines: 20,
            requires_user_confirmation: false,
            mutation_safe: true,
            gate_reason: "bounded report-only mutation".to_string(),
        },
    ));

    tasks.push(build_task(
        "task-risky-runtime-rewrite",
        "Rewrite runtime policy selection wholesale",
        vec![
            "large policy changes can bypass adversarial safeguards".to_string(),
            "requires human review because blast radius is high".to_string(),
        ],
        0.62,
        0.92,
        0.50,
        TinyPatchPlan {
            summary: "Large runtime policy rewrite".to_string(),
            files_touched: 5,
            estimated_changed_lines: 260,
            requires_user_confirmation: true,
            mutation_safe: false,
            gate_reason: "too large, high risk, requires explicit user confirmation".to_string(),
        },
    ));

    tasks.sort_by(|a, b| b.rank_score.total_cmp(&a.rank_score));
    let gate_decisions = tasks
        .iter()
        .map(|task| tiny_patch_gate(task, true, false))
        .collect::<Vec<_>>();
    let report = EvidenceRankedTaskReport {
        created_at_ms: now_ms(),
        tasks,
        gate_decisions,
        mutation_enabled: false,
        summary: "evidence-ranked self-improvement tasks synthesized; tiny patch mutation remains dry-run gated".to_string(),
    };
    let path = evidence_ranked_tasks_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

fn build_task(
    id: &str,
    title: &str,
    evidence: Vec<String>,
    expected_utility: f64,
    risk: f64,
    confidence: f64,
    tiny_patch: TinyPatchPlan,
) -> EvidenceRankedTask {
    let rank_score = ((expected_utility * confidence) - (risk * 0.55)).clamp(0.0, 1.0);
    EvidenceRankedTask {
        id: id.to_string(),
        title: title.to_string(),
        evidence,
        expected_utility,
        risk,
        confidence,
        rank_score,
        tiny_patch,
    }
}

pub fn tiny_patch_gate(
    task: &EvidenceRankedTask,
    dry_run: bool,
    allow_mutation: bool,
) -> TinyPatchGateDecision {
    let mut reasons = Vec::new();
    if dry_run {
        reasons.push("dry-run mode blocks mutation".to_string());
    }
    if !allow_mutation {
        reasons.push("allow-mutation flag is false".to_string());
    }
    if task.tiny_patch.files_touched > 2 {
        reasons.push("patch touches too many files".to_string());
    }
    if task.tiny_patch.estimated_changed_lines > 40 {
        reasons.push("patch exceeds tiny changed-line threshold".to_string());
    }
    if task.tiny_patch.requires_user_confirmation {
        reasons.push("task requires explicit user confirmation".to_string());
    }
    if !task.tiny_patch.mutation_safe {
        reasons.push("task is not marked mutation safe".to_string());
    }
    if task.risk >= 0.45 {
        reasons.push(format!(
            "risk {:.2} exceeds autonomous threshold",
            task.risk
        ));
    }
    if task.rank_score < 0.40 {
        reasons.push(format!(
            "rank score {:.3} below mutation threshold",
            task.rank_score
        ));
    }
    TinyPatchGateDecision {
        task_id: task.id.clone(),
        allowed: reasons.is_empty(),
        dry_run,
        reasons,
    }
}

pub fn load_or_synthesize_evidence_tasks() -> Result<EvidenceRankedTaskReport> {
    let path = evidence_ranked_tasks_path();
    if path.exists() {
        let bytes = fs::read(&path)?;
        serde_json::from_slice(&bytes).or_else(|_| synthesize_evidence_ranked_tasks())
    } else {
        synthesize_evidence_ranked_tasks()
    }
}

pub fn render_evidence_ranked_tasks(report: &EvidenceRankedTaskReport) -> String {
    let mut out = String::new();
    out.push_str("# Kcode Evidence-Ranked Self-Improvement Tasks\n\n");
    out.push_str(&format!(
        "- Mutation enabled: `{}`\n",
        report.mutation_enabled
    ));
    out.push_str(&format!("- Summary: `{}`\n\n", report.summary));
    out.push_str("## Ranked Tasks\n\n");
    for task in &report.tasks {
        out.push_str(&format!(
            "### `{}` - {}\n\n- rank_score: `{:.3}`\n- utility: `{:.3}`\n- risk: `{:.3}`\n- confidence: `{:.3}`\n- tiny_patch: files=`{}` lines=`{}` safe=`{}`\n- gate: {}\n\nEvidence:\n",
            task.id,
            task.title,
            task.rank_score,
            task.expected_utility,
            task.risk,
            task.confidence,
            task.tiny_patch.files_touched,
            task.tiny_patch.estimated_changed_lines,
            task.tiny_patch.mutation_safe,
            task.tiny_patch.gate_reason
        ));
        for evidence in &task.evidence {
            out.push_str(&format!("- {evidence}\n"));
        }
        if let Some(gate) = report.gate_decisions.iter().find(|g| g.task_id == task.id) {
            out.push_str("\nTiny patch gate:\n");
            out.push_str(&format!("- allowed: `{}`\n", gate.allowed));
            for reason in &gate.reasons {
                out.push_str(&format!("- blocked: {reason}\n"));
            }
        }
        out.push('\n');
    }
    out
}

pub fn write_evidence_ranked_tasks_markdown(output: Option<PathBuf>) -> Result<PathBuf> {
    let report = load_or_synthesize_evidence_tasks()?;
    let path = output.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Desktop")
            .join("self_improvement_tasks.md")
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_evidence_ranked_tasks(&report))?;
    Ok(path)
}
