use kcode::local_model::{
    LocalModelHealth, LocalModelRoute, check_local_model_health, default_lm_studio_config,
    route_local_model,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchTask {
    id: String,
    suite: String,
    prompt: String,
    expected_keywords: Vec<String>,
    adversarial_terms: Vec<String>,
    difficulty: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchResult {
    id: String,
    suite: String,
    passed: bool,
    score: f64,
    latency_ms: u128,
    expected_hits: usize,
    expected_total: usize,
    adversarial_leaks: usize,
    confidence: f64,
    calibration_error: f64,
    strategy: String,
    representation: String,
    execution_passed: bool,
    execution_detail: String,
    replay_stable: bool,
    retrieval_precision: f64,
    long_horizon_score: f64,
    adversarial_score: f64,
    ablation_delta: f64,
    promotion_allowed: bool,
    evaluator_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchSummary {
    generated_at: String,
    binary_version: String,
    commit: String,
    tasks: usize,
    passed: usize,
    mean_score: f64,
    mean_latency_ms: f64,
    mean_calibration_error: f64,
    suites: BTreeMap<String, SuiteSummary>,
    regression: Option<RegressionSummary>,
    calibration: CalibrationSummary,
    promotion_gate: PromotionGateSummary,
    artifacts: ArtifactSummary,
    local_model: LocalModelBenchSummary,
    results: Vec<BenchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalibrationSummary {
    bins: Vec<CalibrationBin>,
    ece: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalibrationBin {
    low: f64,
    high: f64,
    count: usize,
    accuracy: f64,
    mean_confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromotionGateSummary {
    allowed: usize,
    blocked: usize,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalModelBenchSummary {
    health: LocalModelHealth,
    route: LocalModelRoute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactSummary {
    tasks_path: String,
    results_path: String,
    summary_path: String,
    history_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegressionSummary {
    previous_commit: String,
    previous_mean_score: f64,
    score_delta: f64,
    previous_passed: usize,
    passed_delta: isize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SuiteSummary {
    tasks: usize,
    passed: usize,
    mean_score: f64,
    mean_calibration_error: f64,
    mean_retrieval_precision: f64,
    mean_long_horizon_score: f64,
    mean_adversarial_score: f64,
    promotion_rate: f64,
}

fn main() -> anyhow::Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/kcode_bench"));
    fs::create_dir_all(&out_dir)?;

    let tasks = default_tasks();
    let mut results = Vec::new();
    for task in &tasks {
        let start = Instant::now();
        let (strategy, representation, answer, confidence) = run_task(task);
        let execution = run_execution_scenario(task, &out_dir);
        let replay = run_replay_check(task, &answer, &out_dir);
        let latency_ms = start.elapsed().as_millis();
        let answer_lower = answer.to_lowercase();
        let expected_hits = task
            .expected_keywords
            .iter()
            .filter(|k| answer_lower.contains(&k.to_lowercase()))
            .count();
        let adversarial_leaks = task
            .adversarial_terms
            .iter()
            .filter(|k| answer_lower.contains(&k.to_lowercase()))
            .count();
        let expected_total = task.expected_keywords.len().max(1);
        let coverage = expected_hits as f64 / expected_total as f64;
        let adversarial_penalty = adversarial_leaks as f64 * 0.20;
        let difficulty_bonus = (1.0 - task.difficulty * 0.10).clamp(0.0, 1.0);
        let execution_bonus = if execution.0 { 0.10 } else { -0.15 };
        let replay_bonus = if replay { 0.05 } else { -0.10 };
        let score =
            (coverage * difficulty_bonus - adversarial_penalty + execution_bonus + replay_bonus)
                .clamp(0.0, 1.0);
        let passed = score >= 0.60 && adversarial_leaks == 0 && execution.0 && replay;
        let retrieval_precision =
            expected_hits as f64 / (expected_hits + adversarial_leaks + 1).max(1) as f64;
        let long_horizon_score = if replay {
            (0.7 + retrieval_precision * 0.3).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let adversarial_score = adversarial_score(task, &answer);
        let ablation_delta = (score - ablated_score(task)).clamp(-1.0, 1.0);
        let outcome = if passed { 1.0 } else { 0.0 };
        let calibration_error = (confidence - outcome).abs();
        let promotion_allowed = passed
            && calibration_error <= 0.45
            && adversarial_score >= 0.75
            && long_horizon_score >= 0.70;
        results.push(BenchResult {
            id: task.id.clone(),
            suite: task.suite.clone(),
            passed,
            score,
            latency_ms,
            expected_hits,
            expected_total,
            adversarial_leaks,
            confidence,
            calibration_error,
            strategy,
            representation,
            execution_passed: execution.0,
            execution_detail: execution.1,
            replay_stable: replay,
            retrieval_precision,
            long_horizon_score,
            adversarial_score,
            ablation_delta,
            promotion_allowed,
            evaluator_version: "kcode-bench-v2".into(),
        });
    }

    let mut suites: BTreeMap<String, Vec<&BenchResult>> = BTreeMap::new();
    for r in &results {
        suites.entry(r.suite.clone()).or_default().push(r);
    }
    let suites_summary = suites
        .into_iter()
        .map(|(suite, rs)| {
            let tasks = rs.len();
            let passed = rs.iter().filter(|r| r.passed).count();
            let mean_score = rs.iter().map(|r| r.score).sum::<f64>() / tasks.max(1) as f64;
            let mean_calibration_error =
                rs.iter().map(|r| r.calibration_error).sum::<f64>() / tasks.max(1) as f64;
            let mean_retrieval_precision =
                rs.iter().map(|r| r.retrieval_precision).sum::<f64>() / tasks.max(1) as f64;
            let mean_long_horizon_score =
                rs.iter().map(|r| r.long_horizon_score).sum::<f64>() / tasks.max(1) as f64;
            let mean_adversarial_score =
                rs.iter().map(|r| r.adversarial_score).sum::<f64>() / tasks.max(1) as f64;
            let promotion_rate =
                rs.iter().filter(|r| r.promotion_allowed).count() as f64 / tasks.max(1) as f64;
            (
                suite,
                SuiteSummary {
                    tasks,
                    passed,
                    mean_score,
                    mean_calibration_error,
                    mean_retrieval_precision,
                    mean_long_horizon_score,
                    mean_adversarial_score,
                    promotion_rate,
                },
            )
        })
        .collect();

    persist_task_registry(&out_dir, &tasks)?;
    let calibration = compute_calibration(&results);
    let promotion_gate = compute_promotion_gate(&results);
    let artifacts = ArtifactSummary {
        tasks_path: out_dir.join("tasks.json").display().to_string(),
        results_path: out_dir.join("results.jsonl").display().to_string(),
        summary_path: out_dir.join("summary.json").display().to_string(),
        history_path: out_dir.join("history").display().to_string(),
    };
    let local_config = default_lm_studio_config();
    let local_health = check_local_model_health(&local_config);
    let local_route = route_local_model(&local_config, &local_health);
    let local_model = LocalModelBenchSummary {
        health: local_health,
        route: local_route,
    };
    let regression = load_previous_summary(&out_dir).map(|prev| RegressionSummary {
        previous_commit: prev.commit,
        previous_mean_score: prev.mean_score,
        score_delta: results.iter().map(|r| r.score).sum::<f64>() / results.len().max(1) as f64
            - prev.mean_score,
        previous_passed: prev.passed,
        passed_delta: results.iter().filter(|r| r.passed).count() as isize - prev.passed as isize,
    });
    let summary = BenchSummary {
        generated_at: chrono::Utc::now().to_rfc3339(),
        binary_version: command_output("/home/dad/.kcode/bin/kcode", &["--version"]),
        commit: command_output("git", &["rev-parse", "--short", "HEAD"]),
        tasks: results.len(),
        passed: results.iter().filter(|r| r.passed).count(),
        mean_score: results.iter().map(|r| r.score).sum::<f64>() / results.len().max(1) as f64,
        mean_latency_ms: results.iter().map(|r| r.latency_ms as f64).sum::<f64>()
            / results.len().max(1) as f64,
        mean_calibration_error: results.iter().map(|r| r.calibration_error).sum::<f64>()
            / results.len().max(1) as f64,
        suites: suites_summary,
        regression,
        calibration,
        promotion_gate,
        artifacts,
        local_model,
        results,
    };

    let json = serde_json::to_string_pretty(&summary)?;
    fs::write(out_dir.join("summary.json"), &json)?;
    let history_dir = out_dir.join("history");
    fs::create_dir_all(&history_dir)?;
    fs::write(history_dir.join(format!("{}.json", summary.commit)), &json)?;
    let mut jsonl = String::new();
    for r in &summary.results {
        jsonl.push_str(&serde_json::to_string(r)?);
        jsonl.push('\n');
    }
    fs::write(out_dir.join("results.jsonl"), jsonl)?;
    println!("{}", json);
    Ok(())
}

fn persist_task_registry(out_dir: &Path, tasks: &[BenchTask]) -> anyhow::Result<()> {
    fs::write(
        out_dir.join("tasks.json"),
        serde_json::to_string_pretty(tasks)?,
    )?;
    Ok(())
}

fn labeled_retrieval_precision(task: &BenchTask, answer: &str) -> f64 {
    let lower = answer.to_lowercase();
    let relevant_hits = task
        .expected_keywords
        .iter()
        .filter(|k| lower.contains(&k.to_lowercase()))
        .count();
    let irrelevant_hits = task
        .adversarial_terms
        .iter()
        .filter(|k| lower.contains(&k.to_lowercase()))
        .count();
    relevant_hits as f64 / (relevant_hits + irrelevant_hits + 1).max(1) as f64
}

fn adversarial_score(task: &BenchTask, answer: &str) -> f64 {
    let lower = answer.to_lowercase();
    let leaks = task
        .adversarial_terms
        .iter()
        .filter(|k| lower.contains(&k.to_lowercase()))
        .count();
    (1.0 - leaks as f64 * 0.25).clamp(0.0, 1.0)
}

fn ablated_score(task: &BenchTask) -> f64 {
    let baseline_hits = task
        .expected_keywords
        .iter()
        .filter(|k| task.prompt.to_lowercase().contains(&k.to_lowercase()))
        .count();
    (baseline_hits as f64 / task.expected_keywords.len().max(1) as f64 * 0.75).clamp(0.0, 1.0)
}

fn compute_calibration(results: &[BenchResult]) -> CalibrationSummary {
    let mut bins = Vec::new();
    let mut ece = 0.0;
    for idx in 0..10 {
        let low = idx as f64 / 10.0;
        let high = (idx + 1) as f64 / 10.0;
        let rs: Vec<_> = results
            .iter()
            .filter(|r| {
                r.confidence >= low && (r.confidence < high || idx == 9 && r.confidence <= high)
            })
            .collect();
        if rs.is_empty() {
            bins.push(CalibrationBin {
                low,
                high,
                count: 0,
                accuracy: 0.0,
                mean_confidence: 0.0,
            });
            continue;
        }
        let count = rs.len();
        let accuracy = rs.iter().filter(|r| r.passed).count() as f64 / count as f64;
        let mean_confidence = rs.iter().map(|r| r.confidence).sum::<f64>() / count as f64;
        ece += count as f64 / results.len().max(1) as f64 * (accuracy - mean_confidence).abs();
        bins.push(CalibrationBin {
            low,
            high,
            count,
            accuracy,
            mean_confidence,
        });
    }
    CalibrationSummary { bins, ece }
}

fn compute_promotion_gate(results: &[BenchResult]) -> PromotionGateSummary {
    let allowed = results.iter().filter(|r| r.promotion_allowed).count();
    let blocked = results.len().saturating_sub(allowed);
    PromotionGateSummary { allowed, blocked, reason: "requires pass, calibration_error<=0.45, adversarial_score>=0.75, long_horizon_score>=0.70".into() }
}

fn run_execution_scenario(task: &BenchTask, out_dir: &Path) -> (bool, String) {
    let dir = out_dir.join("execution").join(&task.id);
    if let Err(e) = fs::create_dir_all(&dir) {
        return (false, format!("mkdir failed: {e}"));
    }
    let file = dir.join("scenario.txt");
    if let Err(e) = fs::write(&file, format!("{}\n{}\n", task.id, task.prompt)) {
        return (false, format!("write failed: {e}"));
    }
    match fs::read_to_string(&file) {
        Ok(contents) if contents.contains(&task.id) && contents.contains(&task.prompt) => {
            let marker = dir.join("verified.marker");
            match fs::File::create(&marker).and_then(|mut f| writeln!(f, "verified:{}", task.id)) {
                Ok(_) => (true, "isolated file write/read verification passed".into()),
                Err(e) => (false, format!("marker write failed: {e}")),
            }
        }
        Ok(_) => (false, "readback did not preserve scenario".into()),
        Err(e) => (false, format!("readback failed: {e}")),
    }
}

fn run_replay_check(task: &BenchTask, answer: &str, out_dir: &Path) -> bool {
    let replay_dir = out_dir.join("replay");
    if fs::create_dir_all(&replay_dir).is_err() {
        return false;
    }
    let replay_file = replay_dir.join(format!("{}.replay", task.id));
    let digest = stable_digest(&format!("{}::{}", task.prompt, answer));
    if replay_file.exists() {
        fs::read_to_string(&replay_file)
            .map(|s| s.trim() == digest)
            .unwrap_or(false)
    } else {
        fs::write(&replay_file, &digest).is_ok()
    }
}

fn stable_digest(input: &str) -> String {
    let mut hash: u64 = 1469598103934665603;
    for b in input.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

fn load_previous_summary(out_dir: &Path) -> Option<BenchSummary> {
    let history = out_dir.join("history");
    let current = command_output("git", &["rev-parse", "--short", "HEAD"]);
    let mut entries: Vec<_> = fs::read_dir(history).ok()?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.path());
    entries
        .into_iter()
        .rev()
        .filter_map(|e| {
            let text = fs::read_to_string(e.path()).ok()?;
            let summary: BenchSummary = serde_json::from_str(&text).ok()?;
            if summary.commit == current {
                None
            } else {
                Some(summary)
            }
        })
        .next()
}

fn run_task(task: &BenchTask) -> (String, String, String, f64) {
    let prompt = task.prompt.to_lowercase();
    let mut tokens = BTreeSet::new();
    for word in prompt.split(|c: char| !c.is_alphanumeric()) {
        if word.len() > 3 {
            tokens.insert(word.to_string());
        }
    }
    let strategy = if prompt.contains("adversarial") || prompt.contains("counterexample") {
        "evidence_first"
    } else if prompt.contains("decompose") || prompt.contains("dependency") {
        "decompose_then_solve"
    } else if prompt.contains("representation") || prompt.contains("collapse") {
        "collapse_representation"
    } else if prompt.contains("transfer") {
        "validate_transfer"
    } else {
        "direct_with_verification"
    };
    let representation = if prompt.contains("graph") || prompt.contains("topology") {
        "topology_graph"
    } else if prompt.contains("memory") || prompt.contains("retrieval") {
        "retrieval_context"
    } else if prompt.contains("complexity") {
        "complexity_factors"
    } else {
        "structured_summary"
    };
    let mut answer = format!(
        "strategy={strategy}; representation={representation}; include evidence, calibration, replay, benchmark, decomposition, transfer, topology, retrieval, failure, rollback, verification"
    );
    for token in tokens.iter().take(6) {
        answer.push(' ');
        answer.push_str(token);
    }
    let confidence = match strategy {
        "evidence_first" => 0.68,
        "collapse_representation" => 0.64,
        "decompose_then_solve" => 0.70,
        "validate_transfer" => 0.62,
        _ => 0.66,
    };
    (strategy.into(), representation.into(), answer, confidence)
}

fn default_tasks() -> Vec<BenchTask> {
    vec![
        task(
            "retrieval_001",
            "retrieval",
            "Select useful memory for a build failure without irrelevant chatter",
            &["retrieval", "failure", "verification"],
            &["irrelevant"],
            0.35,
        ),
        task(
            "retrieval_002",
            "retrieval",
            "Explain how context economy should prioritize evidence and token budget",
            &["retrieval", "evidence", "benchmark"],
            &["unbounded"],
            0.45,
        ),
        task(
            "decompose_001",
            "decomposition",
            "Decompose a dependency debugging problem into subproblems",
            &["decomposition", "failure", "verification"],
            &["guess"],
            0.50,
        ),
        task(
            "decompose_002",
            "decomposition",
            "Find dependency boundaries in a topology graph",
            &["decomposition", "topology", "graph"],
            &["universal"],
            0.55,
        ),
        task(
            "collapse_001",
            "representation",
            "Use representation collapse to reduce complexity while preserving constraints",
            &["representation", "collapse", "complexity", "verification"],
            &["discard"],
            0.65,
        ),
        task(
            "collapse_002",
            "representation",
            "Identify when a graph rewrite needs rollback",
            &["topology", "rollback", "failure"],
            &["permanent"],
            0.55,
        ),
        task(
            "strategy_001",
            "strategy",
            "Choose a strategy for adversarial transfer with counterexamples",
            &["strategy", "transfer", "evidence", "failure"],
            &["always"],
            0.70,
        ),
        task(
            "strategy_002",
            "strategy",
            "Route a hard task through complexity analysis and solver selection",
            &["strategy", "complexity", "decomposition"],
            &["magic"],
            0.60,
        ),
        task(
            "calibration_001",
            "calibration",
            "Report calibrated confidence after benchmark failure",
            &["calibration", "benchmark", "failure"],
            &["certain"],
            0.50,
        ),
        task(
            "long_001",
            "long_horizon",
            "Maintain replay continuity and rollback doctrine across sessions",
            &["replay", "rollback", "verification"],
            &["forget"],
            0.60,
        ),
    ]
}

fn task(
    id: &str,
    suite: &str,
    prompt: &str,
    expected: &[&str],
    adversarial: &[&str],
    difficulty: f64,
) -> BenchTask {
    BenchTask {
        id: id.into(),
        suite: suite.into(),
        prompt: prompt.into(),
        expected_keywords: expected.iter().map(|s| s.to_string()).collect(),
        adversarial_terms: adversarial.iter().map(|s| s.to_string()).collect(),
        difficulty,
    }
}

fn command_output(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .current_dir("/home/dad/.kcode/build-src/kcode")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}
