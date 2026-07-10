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
    PolicyDecision,
    ToolInvocation,
    MemoryUpdate,
    TokenEvent,
    PromotionDecision,
    /// Architectural expectation recorded before acting (knowledge layer).
    Prediction,
    /// Post-execution comparison of predictions against reality.
    Reflection,
    /// Read-only architectural observation from the semantic graph.
    ArchitecturalInsight,
    /// Preserved engineering decision: reasoning, alternatives, tradeoffs.
    EngineeringDecision,
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
    pub parent_hashes: Vec<String>,
    pub cause_hashes: Vec<String>,
    pub subsystem: String,
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
    pub receipt: EvidenceReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceReceipt {
    pub index: u64,
    pub hash: String,
    pub prev_hash: String,
    pub payload_hash: String,
    pub kind: EvidenceKind,
    pub subject: String,
    pub subsystem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerQuery {
    pub kind: Option<EvidenceKind>,
    pub subject_contains: Option<String>,
    pub subsystem: Option<String>,
    pub limit: usize,
}

impl Default for LedgerQuery {
    fn default() -> Self {
        Self {
            kind: None,
            subject_contains: None,
            subsystem: None,
            limit: 25,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceExplanation {
    pub block: EvidenceBlock,
    pub parents: Vec<EvidenceBlock>,
    pub causes: Vec<EvidenceBlock>,
    pub verifies: bool,
}

pub fn ledger_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".neura")
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

#[allow(clippy::too_many_arguments)]
fn block_hash(
    index: u64,
    timestamp_ms: u128,
    kind: &EvidenceKind,
    subject: &str,
    summary: &str,
    score: Option<f64>,
    passed: Option<bool>,
    payload_hash: &str,
    parent_hashes: &[String],
    cause_hashes: &[String],
    subsystem: &str,
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
        parent_hashes,
        cause_hashes,
        subsystem,
        prev_hash,
    )
}

#[allow(clippy::too_many_arguments)]
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
    parent_hashes: &[String],
    cause_hashes: &[String],
    subsystem: &str,
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
parents={}
causes={}
subsystem={subsystem}
prev={prev_hash}
",
        parent_hashes.join(","),
        cause_hashes.join(","),
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
        self.append_with_links(
            kind,
            subject,
            summary,
            score,
            passed,
            payload,
            Vec::new(),
            Vec::new(),
            "general",
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_with_links<T: Serialize>(
        &mut self,
        kind: EvidenceKind,
        subject: impl Into<String>,
        summary: impl Into<String>,
        score: Option<f64>,
        passed: Option<bool>,
        payload: &T,
        parent_hashes: Vec<String>,
        cause_hashes: Vec<String>,
        subsystem: impl Into<String>,
    ) -> Result<EvidenceBlock> {
        let index = self.blocks.len() as u64;
        let timestamp_ms = now_ms();
        let subject = subject.into();
        let summary = summary.into();
        let subsystem = subsystem.into();
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
            &parent_hashes,
            &cause_hashes,
            &subsystem,
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
            parent_hashes,
            cause_hashes,
            subsystem,
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
                &block.parent_hashes,
                &block.cause_hashes,
                &block.subsystem,
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
    let receipt = EvidenceReceipt {
        index: block.index,
        hash: block.hash.clone(),
        prev_hash: block.prev_hash.clone(),
        payload_hash: block.payload_hash.clone(),
        kind: block.kind.clone(),
        subject: block.subject.clone(),
        subsystem: block.subsystem.clone(),
    };
    Ok(LedgerAppendResult {
        appended: true,
        block,
        verification,
        receipt,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn append_evidence_with_links<T: Serialize>(
    kind: EvidenceKind,
    subject: impl Into<String>,
    summary: impl Into<String>,
    score: Option<f64>,
    passed: Option<bool>,
    payload: &T,
    parent_hashes: Vec<String>,
    cause_hashes: Vec<String>,
    subsystem: impl Into<String>,
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
    let block = ledger.append_with_links(
        kind,
        subject,
        summary,
        score,
        passed,
        payload,
        parent_hashes,
        cause_hashes,
        subsystem,
    )?;
    let verification = ledger.verify();
    if !verification.valid {
        return Err(anyhow!(
            "evidence ledger verification failed after append: {:?}",
            verification.errors
        ));
    }
    ledger.save(&path)?;
    let receipt = EvidenceReceipt {
        index: block.index,
        hash: block.hash.clone(),
        prev_hash: block.prev_hash.clone(),
        payload_hash: block.payload_hash.clone(),
        kind: block.kind.clone(),
        subject: block.subject.clone(),
        subsystem: block.subsystem.clone(),
    };
    Ok(LedgerAppendResult {
        appended: true,
        block,
        verification,
        receipt,
    })
}

pub fn query_ledger(query: LedgerQuery) -> Result<Vec<EvidenceBlock>> {
    let ledger = EvidenceLedger::load_or_default(&ledger_path())?;
    let mut blocks = ledger.blocks;
    blocks.reverse();
    let limit = query.limit.clamp(1, 500);
    Ok(blocks
        .into_iter()
        .filter(|block| query.kind.as_ref().is_none_or(|kind| &block.kind == kind))
        .filter(|block| {
            query.subject_contains.as_ref().is_none_or(|needle| {
                block
                    .subject
                    .to_lowercase()
                    .contains(&needle.to_lowercase())
                    || block
                        .summary
                        .to_lowercase()
                        .contains(&needle.to_lowercase())
            })
        })
        .filter(|block| {
            query
                .subsystem
                .as_ref()
                .is_none_or(|subsystem| block.subsystem == *subsystem)
        })
        .take(limit)
        .collect())
}

pub fn explain_evidence(hash_or_index: &str) -> Result<Option<EvidenceExplanation>> {
    let ledger = EvidenceLedger::load_or_default(&ledger_path())?;
    let verification = ledger.verify();
    let block = if let Ok(index) = hash_or_index.parse::<u64>() {
        ledger
            .blocks
            .iter()
            .find(|block| block.index == index)
            .cloned()
    } else {
        ledger
            .blocks
            .iter()
            .find(|block| block.hash.starts_with(hash_or_index))
            .cloned()
    };
    let Some(block) = block else {
        return Ok(None);
    };
    let parents = block
        .parent_hashes
        .iter()
        .filter_map(|hash| {
            ledger
                .blocks
                .iter()
                .find(|candidate| candidate.hash == *hash)
                .cloned()
        })
        .collect();
    let causes = block
        .cause_hashes
        .iter()
        .filter_map(|hash| {
            ledger
                .blocks
                .iter()
                .find(|candidate| candidate.hash == *hash)
                .cloned()
        })
        .collect();
    Ok(Some(EvidenceExplanation {
        block,
        parents,
        causes,
        verifies: verification.valid,
    }))
}

pub fn verify_ledger() -> Result<LedgerVerification> {
    Ok(EvidenceLedger::load_or_default(&ledger_path())?.verify())
}

pub fn render_ledger_report(ledger: &EvidenceLedger) -> String {
    let verification = ledger.verify();
    let mut out = String::new();
    out.push_str("# Neura Cognition Evidence Chain\n\n");
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
