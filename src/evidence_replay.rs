use crate::evidence_ledger::{
    EvidenceBlock, EvidenceKind, EvidenceLedger, LedgerQuery, explain_evidence, ledger_path,
    query_ledger,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayConfig {
    pub limit: usize,
    pub include_alternatives: bool,
    pub max_index: Option<u64>,
    pub subject: Option<String>,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            limit: 25,
            include_alternatives: true,
            max_index: None,
            subject: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayCase {
    pub source_index: u64,
    pub source_hash: String,
    pub kind: EvidenceKind,
    pub subject: String,
    pub subsystem: String,
    pub original_passed: Option<bool>,
    pub original_score: Option<f64>,
    pub replay_score: f64,
    pub replay_passed: bool,
    pub no_future_leak: bool,
    pub alternatives: Vec<ReplayAlternative>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayAlternative {
    pub name: String,
    pub score: f64,
    pub passed: bool,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub created_at_ms: u128,
    pub config: ReplayConfig,
    pub cases: Vec<ReplayCase>,
    pub replayed: usize,
    pub no_future_leaks: bool,
    pub mean_replay_score: f64,
    pub alternatives_considered: usize,
    pub summary: String,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn replay_report_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kcode")
        .join("evidence_replay_report.json")
}

pub fn run_replay(config: ReplayConfig) -> Result<ReplayReport> {
    let ledger = EvidenceLedger::load_or_default(&ledger_path())?;
    let verification = ledger.verify();
    let mut blocks = ledger.blocks.clone();
    blocks.reverse();
    let limit = config.limit.clamp(1, 500);
    let mut cases = Vec::new();

    for block in blocks {
        if cases.len() >= limit {
            break;
        }
        if let Some(max_index) = config.max_index {
            if block.index > max_index {
                continue;
            }
        }
        if let Some(subject) = &config.subject {
            let needle = subject.to_ascii_lowercase();
            if !block.subject.to_ascii_lowercase().contains(&needle)
                && !block.summary.to_ascii_lowercase().contains(&needle)
            {
                continue;
            }
        }
        if !is_replayable_kind(&block.kind) {
            continue;
        }
        cases.push(replay_block(
            &ledger,
            &block,
            config.include_alternatives,
            verification.valid,
        )?);
    }

    cases.reverse();
    let replayed = cases.len();
    let alternatives_considered = cases.iter().map(|case| case.alternatives.len()).sum();
    let mean_replay_score = if replayed == 0 {
        0.0
    } else {
        cases.iter().map(|case| case.replay_score).sum::<f64>() / replayed as f64
    };
    let no_future_leaks = cases.iter().all(|case| case.no_future_leak);
    let summary: String = if verification.valid && no_future_leaks {
        "replay completed against verified ledger without future leakage".to_string()
    } else if !verification.valid {
        "replay completed but source ledger verification failed".to_string()
    } else {
        "replay detected future-leak risk".to_string()
    };
    let report = ReplayReport {
        created_at_ms: now_ms(),
        config,
        cases,
        replayed,
        no_future_leaks,
        mean_replay_score,
        alternatives_considered,
        summary,
    };
    let path = replay_report_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    Ok(report)
}

fn is_replayable_kind(kind: &EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::OperationalEval
            | EvidenceKind::AdversarialEval
            | EvidenceKind::SelfImprovementCycle
            | EvidenceKind::EvidenceRankedTask
            | EvidenceKind::TinyPatchGate
            | EvidenceKind::PolicyDecision
            | EvidenceKind::PromotionDecision
    )
}

fn replay_block(
    ledger: &EvidenceLedger,
    block: &EvidenceBlock,
    include_alternatives: bool,
    ledger_valid: bool,
) -> Result<ReplayCase> {
    let historical = historical_context(ledger, block.index);
    let no_future_leak = historical
        .iter()
        .all(|candidate| candidate.index <= block.index)
        && block
            .parent_hashes
            .iter()
            .chain(block.cause_hashes.iter())
            .all(|hash| {
                ledger
                    .blocks
                    .iter()
                    .find(|candidate| candidate.hash == *hash)
                    .is_none_or(|candidate| candidate.index <= block.index)
            });
    let replay_score = deterministic_replay_score(block, &historical, ledger_valid, no_future_leak);
    let replay_passed = replay_score >= threshold_for_kind(&block.kind);
    let alternatives = if include_alternatives {
        alternative_paths(block, replay_score)
    } else {
        Vec::new()
    };
    let explanation = format!(
        "replayed with {} historical blocks, ledger_valid={}, no_future_leak={}",
        historical.len(),
        ledger_valid,
        no_future_leak
    );
    Ok(ReplayCase {
        source_index: block.index,
        source_hash: block.hash.clone(),
        kind: block.kind.clone(),
        subject: block.subject.clone(),
        subsystem: block.subsystem.clone(),
        original_passed: block.passed,
        original_score: block.score,
        replay_score,
        replay_passed,
        no_future_leak,
        alternatives,
        explanation,
    })
}

fn historical_context(ledger: &EvidenceLedger, index: u64) -> Vec<EvidenceBlock> {
    ledger
        .blocks
        .iter()
        .filter(|block| block.index <= index)
        .cloned()
        .collect()
}

fn deterministic_replay_score(
    block: &EvidenceBlock,
    historical: &[EvidenceBlock],
    ledger_valid: bool,
    no_future_leak: bool,
) -> f64 {
    let base = block.score.unwrap_or_else(|| {
        if block.passed.unwrap_or(false) {
            1.0
        } else {
            0.35
        }
    });
    let same_subsystem = historical
        .iter()
        .filter(|candidate| candidate.subsystem == block.subsystem)
        .count() as f64;
    let causal_bonus = (block.parent_hashes.len() + block.cause_hashes.len()) as f64 * 0.015;
    let subsystem_bonus = (same_subsystem.min(6.0)) * 0.01;
    let validity_penalty = if ledger_valid { 0.0 } else { 0.30 };
    let future_penalty = if no_future_leak { 0.0 } else { 0.40 };
    (base + causal_bonus + subsystem_bonus - validity_penalty - future_penalty).clamp(0.0, 1.0)
}

fn threshold_for_kind(kind: &EvidenceKind) -> f64 {
    match kind {
        EvidenceKind::AdversarialEval => 0.85,
        EvidenceKind::TinyPatchGate => 0.50,
        EvidenceKind::EvidenceRankedTask => 0.40,
        EvidenceKind::SelfImprovementCycle => 0.75,
        EvidenceKind::OperationalEval => 0.75,
        _ => 0.60,
    }
}

fn alternative_paths(block: &EvidenceBlock, replay_score: f64) -> Vec<ReplayAlternative> {
    let conservative = (replay_score - 0.08).clamp(0.0, 1.0);
    let exploratory =
        (replay_score + 0.05 - block.parent_hashes.len() as f64 * 0.01).clamp(0.0, 1.0);
    let audit_first = (replay_score + 0.03).clamp(0.0, 1.0);
    vec![
        ReplayAlternative {
            name: "conservative-require-more-evidence".to_string(),
            score: conservative,
            passed: conservative >= threshold_for_kind(&block.kind),
            rationale: "downranks action until more historical evidence exists".to_string(),
        },
        ReplayAlternative {
            name: "exploratory-shadow-only".to_string(),
            score: exploratory,
            passed: exploratory >= threshold_for_kind(&block.kind),
            rationale: "allows shadow simulation but not live mutation".to_string(),
        },
        ReplayAlternative {
            name: "audit-first".to_string(),
            score: audit_first,
            passed: audit_first >= threshold_for_kind(&block.kind),
            rationale: "requires ledger receipt plus human-readable audit before promotion"
                .to_string(),
        },
    ]
}

pub fn render_replay_report(report: &ReplayReport) -> String {
    let mut out = String::new();
    out.push_str("# Kcode Evidence Replay Report\n\n");
    out.push_str(&format!("- Summary: `{}`\n", report.summary));
    out.push_str(&format!("- Replayed: `{}`\n", report.replayed));
    out.push_str(&format!(
        "- No future leaks: `{}`\n",
        report.no_future_leaks
    ));
    out.push_str(&format!(
        "- Mean replay score: `{:.3}`\n",
        report.mean_replay_score
    ));
    out.push_str(&format!(
        "- Alternatives considered: `{}`\n\n",
        report.alternatives_considered
    ));
    out.push_str("## Replay Cases\n\n");
    for case in &report.cases {
        out.push_str(&format!(
            "### #{} `{:?}` {}\n\n- subsystem: `{}`\n- original: passed=`{:?}` score=`{:?}`\n- replay: passed=`{}` score=`{:.3}` no_future_leak=`{}`\n- hash: `{}`\n- explanation: {}\n\n",
            case.source_index,
            case.kind,
            case.subject,
            case.subsystem,
            case.original_passed,
            case.original_score,
            case.replay_passed,
            case.replay_score,
            case.no_future_leak,
            case.source_hash,
            case.explanation
        ));
        if !case.alternatives.is_empty() {
            out.push_str("Alternatives:\n");
            for alt in &case.alternatives {
                out.push_str(&format!(
                    "- `{}` passed=`{}` score=`{:.3}` - {}\n",
                    alt.name, alt.passed, alt.score, alt.rationale
                ));
            }
            out.push('\n');
        }
    }
    out
}

pub fn write_replay_report(output: Option<PathBuf>, config: ReplayConfig) -> Result<PathBuf> {
    let report = run_replay(config)?;
    let path = output.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Desktop")
            .join("evidence_replay_report.md")
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_replay_report(&report))?;
    Ok(path)
}

pub fn replay_explain(target: &str) -> Result<String> {
    let Some(explanation) = explain_evidence(target)? else {
        return Ok(format!("no evidence block found for {target}"));
    };
    let query = LedgerQuery {
        subject_contains: Some(explanation.block.subject.clone()),
        limit: 5,
        ..LedgerQuery::default()
    };
    let related = query_ledger(query)?;
    Ok(format!(
        "block #{} {:?} subject={} parents={} causes={} related={} verifies={}",
        explanation.block.index,
        explanation.block.kind,
        explanation.block.subject,
        explanation.parents.len(),
        explanation.causes.len(),
        related.len(),
        explanation.verifies
    ))
}
