use crate::latent_memory::{LatentMemoryBank, latent_memory_path};
use crate::operational_policy::{
    OperationalPolicyState, PolicyAction, PolicyDomain, policy_state_path,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const POLICY_CREDIT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyOutcomeCredit {
    pub schema_version: u32,
    pub audit_id: String,
    pub rule_id: Option<String>,
    pub domain: PolicyDomain,
    pub action: PolicyAction,
    pub target: String,
    pub outcome: String,
    pub score: f32,
    pub confidence_before: f32,
    pub confidence_after: f32,
    pub propagated_to_memory_id: Option<String>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyCreditLedger {
    pub schema_version: u32,
    pub credits: Vec<PolicyOutcomeCredit>,
}

impl Default for PolicyCreditLedger {
    fn default() -> Self {
        Self {
            schema_version: POLICY_CREDIT_SCHEMA_VERSION,
            credits: Vec::new(),
        }
    }
}

impl PolicyCreditLedger {
    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
        } else {
            Ok(Self::default())
        }
    }
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyCreditReport {
    pub total_credits: usize,
    pub mean_score: f32,
    pub positive: usize,
    pub negative: usize,
    pub by_domain: BTreeMap<PolicyDomain, usize>,
    pub confidence_delta_total: f32,
}

pub fn credit_ledger_path() -> PathBuf {
    std::env::var_os("NEURA_POLICY_CREDIT_LEDGER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".neura").join("policy_outcome_credit.json")
        })
}

pub fn score_outcome(outcome: &str) -> f32 {
    match outcome {
        "success"
        | "passed"
        | "validation-passed"
        | "provider-request-observed"
        | "runtime-hook-observed" => 1.0,
        "failure" | "failed" | "error" | "validation-failed" => -1.0,
        "skipped" | "blocked" => -0.25,
        _ => 0.25,
    }
}

pub fn assign_credit(audit_id: &str, outcome: &str) -> anyhow::Result<Option<PolicyOutcomeCredit>> {
    let mut state = OperationalPolicyState::load_or_default(&policy_state_path())?;
    let Some(audit) = state.audits.iter().find(|a| a.id == audit_id).cloned() else {
        return Ok(None);
    };
    let score = score_outcome(outcome);
    let mut confidence_before = audit.decision.confidence;
    let mut confidence_after = confidence_before;
    let mut propagated_to_memory_id = None;

    if let Some(rule_id) = &audit.decision.rule_id {
        if let Some(rule) = state.rules.iter_mut().find(|r| &r.id == rule_id) {
            confidence_before = rule.confidence;
            let learning_rate = if score >= 0.0 { 0.08 } else { 0.14 };
            rule.confidence = (rule.confidence * (1.0 - learning_rate)
                + score.max(0.0) * learning_rate)
                .clamp(0.0, 1.0);
            confidence_after = rule.confidence;
            rule.support += 1;
            propagated_to_memory_id = rule.source_memory_id.clone();
        }
    }
    state.record_outcome(audit_id, outcome);
    state.save(&policy_state_path())?;

    if let Some(memory_id) = &propagated_to_memory_id {
        let mut bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
        if let Some(entry) = bank.entries.iter_mut().find(|entry| &entry.id == memory_id) {
            entry.influence_count += 1;
            if score >= 0.0 {
                entry.positive_outcomes += 1;
            } else {
                entry.negative_outcomes += 1;
            }
            entry.usefulness_score =
                (entry.usefulness_score * 0.90 + score.max(0.0) * 0.10).clamp(0.0, 1.0);
        }
        bank.save(&latent_memory_path())?;
    }

    let credit = PolicyOutcomeCredit {
        schema_version: POLICY_CREDIT_SCHEMA_VERSION,
        audit_id: audit_id.into(),
        rule_id: audit.decision.rule_id.clone(),
        domain: audit.decision.domain,
        action: audit.decision.action,
        target: audit.target,
        outcome: outcome.into(),
        score,
        confidence_before,
        confidence_after,
        propagated_to_memory_id,
        timestamp_ms: crate::latent_operational_recurrence::now_ms(),
    };
    let mut ledger = PolicyCreditLedger::load_or_default(&credit_ledger_path())?;
    ledger.credits.push(credit.clone());
    if ledger.credits.len() > 1024 {
        ledger.credits.drain(0..ledger.credits.len() - 1024);
    }
    ledger.save(&credit_ledger_path())?;
    Ok(Some(credit))
}

pub fn report() -> anyhow::Result<PolicyCreditReport> {
    let ledger = PolicyCreditLedger::load_or_default(&credit_ledger_path())?;
    let total = ledger.credits.len();
    let mut by_domain = BTreeMap::new();
    let mut positive = 0;
    let mut negative = 0;
    let mut score_sum = 0.0;
    let mut delta_sum = 0.0;
    for credit in &ledger.credits {
        *by_domain.entry(credit.domain.clone()).or_insert(0) += 1;
        if credit.score >= 0.0 {
            positive += 1;
        } else {
            negative += 1;
        }
        score_sum += credit.score;
        delta_sum += credit.confidence_after - credit.confidence_before;
    }
    Ok(PolicyCreditReport {
        total_credits: total,
        mean_score: if total == 0 {
            0.0
        } else {
            score_sum / total as f32
        },
        positive,
        negative,
        by_domain,
        confidence_delta_total: delta_sum,
    })
}

pub fn render_credit_report() -> anyhow::Result<String> {
    let r = report()?;
    Ok(format!(
        "# Policy Outcome Credit Report\n\nCredits: `{}`\nMean score: `{:.3}`\nPositive: `{}`\nNegative: `{}`\nConfidence delta total: `{:.3}`\n\n## By domain\n\n```json\n{}\n```\n",
        r.total_credits,
        r.mean_score,
        r.positive,
        r.negative,
        r.confidence_delta_total,
        serde_json::to_string_pretty(&r.by_domain)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operational_policy::{PolicyDecision, PolicyInfluenceAudit};
    use tempfile::TempDir;

    #[test]
    fn assigns_credit_to_policy_audit() {
        let dir = TempDir::new().unwrap();
        unsafe {
            std::env::set_var(
                "NEURA_OPERATIONAL_POLICY_STATE",
                dir.path().join("policy.json"),
            )
        };
        unsafe { std::env::set_var("NEURA_POLICY_CREDIT_LEDGER", dir.path().join("credit.json")) };
        unsafe { std::env::set_var("NEURA_LATENT_MEMORY_STATE", dir.path().join("memory.json")) };
        let mut state = OperationalPolicyState::default();
        let decision = PolicyDecision {
            domain: PolicyDomain::TestValidation,
            action: PolicyAction::RequireValidation,
            rule_id: None,
            allowed: true,
            confidence: 0.8,
            reason: "test".into(),
            audit_id: "audit-1".into(),
        };
        state.audits.push(PolicyInfluenceAudit {
            id: "audit-1".into(),
            decision,
            target: "final".into(),
            outcome: "pending".into(),
            timestamp_ms: 0,
        });
        state.save(&policy_state_path()).unwrap();
        let credit = assign_credit("audit-1", "success").unwrap().unwrap();
        assert_eq!(credit.score, 1.0);
        assert_eq!(report().unwrap().total_credits, 1);
    }
}
