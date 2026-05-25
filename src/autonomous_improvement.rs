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
