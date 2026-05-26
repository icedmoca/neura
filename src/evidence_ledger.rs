use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const GENESIS_PREV_HASH: &str = "GENESIS";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EvidenceKind {
    OperationalEval,
    AdversarialEval,
    SelfImprovementCycle,
    EvidenceRankedTask,
    TinyPatchGate,
    Validation,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceBlock {
    pub index: u64,
    pub timestamp_ms: u128,
    pub kind: EvidenceKind,
    pub subject: String,
    pub summary: String,
    pub score: Option<f64>,
    pub passed: Option<bool>,
    pub payload_hash: String,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvidenceLedger {
    pub blocks: Vec<EvidenceBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerVerification {
    pub valid: bool,
    pub blocks: usize,
    pub last_hash: Option<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerAppendResult {
    pub appended: bool,
    pub block: EvidenceBlock,
    pub verification: LedgerVerification,
}

pub fn ledger_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kcode")
        .join("evidence_ledger_chain.json")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn payload_hash<T: Serialize>(payload: &T) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(payload)?))
}

fn block_hash(
    index: u64,
    timestamp_ms: u128,
    kind: &EvidenceKind,
    subject: &str,
    summary: &str,
    score: Option<f64>,
    passed: Option<bool>,
    payload_hash: &str,
    prev_hash: &str,
) -> Result<String> {
    let score_part = score
        .map(|value| format!("{value:.12}"))
        .unwrap_or_else(|| "none".to_string());
    let passed_part = passed
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string());
    block_hash_from_parts(
        index,
        timestamp_ms,
        &format!("{kind:?}"),
        subject,
        summary,
        &score_part,
        &passed_part,
        payload_hash,
        prev_hash,
    )
}

#[allow(clippy::too_many_arguments)]
fn block_hash_from_parts(
    index: u64,
    timestamp_ms: u128,
    kind_part: &str,
    subject: &str,
    summary: &str,
    score_part: &str,
    passed_part: &str,
    payload_hash: &str,
    prev_hash: &str,
) -> Result<String> {
    let canonical = format!(
        "idx={index}
ts={timestamp_ms}
kind={kind_part}
subject={subject}
summary={summary}
score_bits={score_part}
passed={passed_part}
payload={payload_hash}
prev={prev_hash}
"
    );
    Ok(sha256_hex(canonical.as_bytes()))
}

impl EvidenceLedger {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes).map_err(Into::into)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn append<T: Serialize>(
        &mut self,
        kind: EvidenceKind,
        subject: impl Into<String>,
        summary: impl Into<String>,
        score: Option<f64>,
        passed: Option<bool>,
        payload: &T,
    ) -> Result<EvidenceBlock> {
        let index = self.blocks.len() as u64;
        let timestamp_ms = now_ms();
        let subject = subject.into();
        let summary = summary.into();
        let payload_hash = payload_hash(payload)?;
        let prev_hash = self
            .blocks
            .last()
            .map(|b| b.hash.clone())
            .unwrap_or_else(|| GENESIS_PREV_HASH.to_string());
        let hash = block_hash(
            index,
            timestamp_ms,
            &kind,
            &subject,
            &summary,
            score,
            passed,
            &payload_hash,
            &prev_hash,
        )?;
        let block = EvidenceBlock {
            index,
            timestamp_ms,
            kind,
            subject,
            summary,
            score,
            passed,
            payload_hash,
            prev_hash,
            hash,
        };
        self.blocks.push(block.clone());
        Ok(block)
    }

    pub fn verify(&self) -> LedgerVerification {
        let mut errors = Vec::new();
        for (idx, block) in self.blocks.iter().enumerate() {
            if block.index != idx as u64 {
                errors.push(format!("block {idx} has index {}", block.index));
            }
            let expected_prev = if idx == 0 {
                GENESIS_PREV_HASH.to_string()
            } else {
                self.blocks[idx - 1].hash.clone()
            };
            if block.prev_hash != expected_prev {
                errors.push(format!("block {idx} prev_hash mismatch"));
            }
            match block_hash(
                block.index,
                block.timestamp_ms,
                &block.kind,
                &block.subject,
                &block.summary,
                block.score,
                block.passed,
                &block.payload_hash,
                &block.prev_hash,
            ) {
                Ok(expected) if expected == block.hash => {}
                Ok(_) => errors.push(format!("block {idx} hash mismatch")),
                Err(e) => errors.push(format!("block {idx} hash recompute error: {e}")),
            }
        }
        LedgerVerification {
            valid: errors.is_empty(),
            blocks: self.blocks.len(),
            last_hash: self.blocks.last().map(|b| b.hash.clone()),
            errors,
        }
    }
}

pub fn append_evidence<T: Serialize>(
    kind: EvidenceKind,
    subject: impl Into<String>,
    summary: impl Into<String>,
    score: Option<f64>,
    passed: Option<bool>,
    payload: &T,
) -> Result<LedgerAppendResult> {
    let path = ledger_path();
    let mut ledger = EvidenceLedger::load_or_default(&path)?;
    let pre = ledger.verify();
    if !pre.valid {
        return Err(anyhow!(
            "evidence ledger verification failed before append: {:?}",
            pre.errors
        ));
    }
    let block = ledger.append(kind, subject, summary, score, passed, payload)?;
    let verification = ledger.verify();
    if !verification.valid {
        return Err(anyhow!(
            "evidence ledger verification failed after append: {:?}",
            verification.errors
        ));
    }
    ledger.save(&path)?;
    Ok(LedgerAppendResult {
        appended: true,
        block,
        verification,
    })
}

pub fn verify_ledger() -> Result<LedgerVerification> {
    Ok(EvidenceLedger::load_or_default(&ledger_path())?.verify())
}

pub fn render_ledger_report(ledger: &EvidenceLedger) -> String {
    let verification = ledger.verify();
    let mut out = String::new();
    out.push_str("# Kcode Cognition Evidence Chain\n\n");
    out.push_str(&format!("- Valid: `{}`\n", verification.valid));
    out.push_str(&format!("- Blocks: `{}`\n", verification.blocks));
    out.push_str(&format!(
        "- Last hash: `{}`\n\n",
        verification.last_hash.unwrap_or_else(|| "none".into())
    ));
    if !verification.errors.is_empty() {
        out.push_str("## Verification Errors\n\n");
        for err in &verification.errors {
            out.push_str(&format!("- {err}\n"));
        }
        out.push('\n');
    }
    out.push_str("## Blocks\n\n");
    for block in ledger.blocks.iter().rev().take(50) {
        out.push_str(&format!(
            "- #{} `{:?}` subject=`{}` passed=`{:?}` score=`{:?}`\n  - hash=`{}`\n  - prev=`{}`\n  - payload=`{}`\n  - {}\n",
            block.index,
            block.kind,
            block.subject,
            block.passed,
            block.score,
            block.hash,
            block.prev_hash,
            block.payload_hash,
            block.summary
        ));
    }
    out
}

pub fn write_ledger_report(output: Option<PathBuf>) -> Result<PathBuf> {
    let ledger = EvidenceLedger::load_or_default(&ledger_path())?;
    let path = output.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Desktop")
            .join("evidence_ledger_report.md")
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, render_ledger_report(&ledger))?;
    Ok(path)
}
