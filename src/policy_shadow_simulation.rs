use crate::latent_learning_background::{RuntimeLearningSample, learning_dir};
use crate::live_operational_fabric::{LiveEventKind, events as live_events};
use crate::operational_policy::{
    OperationalPolicyState, PolicyAction, PolicyDomain, load_policy_and_synthesize,
    policy_state_path,
};
use crate::policy_outcome_credit::score_outcome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const POLICY_SHADOW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyShadowResult {
    pub schema_version: u32,
    pub source: String,
    pub domain: PolicyDomain,
    pub target: String,
    pub rule_id: Option<String>,
    pub action: PolicyAction,
    pub baseline_outcome: String,
    pub baseline_score: f32,
    pub simulated_score: f32,
    pub delta: f32,
    pub safe_to_promote: bool,
    pub should_demote: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyShadowLedger {
    pub schema_version: u32,
    pub results: Vec<PolicyShadowResult>,
}

impl Default for PolicyShadowLedger {
    fn default() -> Self {
        Self {
            schema_version: POLICY_SHADOW_SCHEMA_VERSION,
            results: Vec::new(),
        }
    }
}

impl PolicyShadowLedger {
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
pub struct PolicyShadowReport {
    pub total_results: usize,
    pub positive: usize,
    pub negative: usize,
    pub neutral: usize,
    pub mean_delta: f32,
    pub promotable: usize,
    pub demotable: usize,
    pub by_domain: BTreeMap<PolicyDomain, usize>,
}

pub fn shadow_ledger_path() -> PathBuf {
    std::env::var_os("KCODE_POLICY_SHADOW_LEDGER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".kcode").join("policy_shadow_simulation.json")
        })
}

pub fn simulate(limit: usize) -> anyhow::Result<PolicyShadowReport> {
    let state = load_policy_and_synthesize()?;
    let mut results = Vec::new();
    for sample in load_runtime_samples()?.into_iter().rev().take(limit) {
        results.push(simulate_sample(&state, &sample));
    }
    for event in live_events()?.into_iter().rev().take(limit) {
        results.push(simulate_live_event(&state, &event));
    }
    let mut ledger = PolicyShadowLedger::load_or_default(&shadow_ledger_path())?;
    ledger.results.extend(results);
    if ledger.results.len() > 2048 {
        ledger.results.drain(0..ledger.results.len() - 2048);
    }
    ledger.save(&shadow_ledger_path())?;
    report()
}

pub fn report() -> anyhow::Result<PolicyShadowReport> {
    let ledger = PolicyShadowLedger::load_or_default(&shadow_ledger_path())?;
    let mut positive = 0;
    let mut negative = 0;
    let mut neutral = 0;
    let mut delta_sum = 0.0;
    let mut promotable = 0;
    let mut demotable = 0;
    let mut by_domain = BTreeMap::new();
    for result in &ledger.results {
        if result.delta > 0.05 {
            positive += 1;
        } else if result.delta < -0.05 {
            negative += 1;
        } else {
            neutral += 1;
        }
        if result.safe_to_promote {
            promotable += 1;
        }
        if result.should_demote {
            demotable += 1;
        }
        delta_sum += result.delta;
        *by_domain.entry(result.domain.clone()).or_insert(0) += 1;
    }
    let total = ledger.results.len();
    Ok(PolicyShadowReport {
        total_results: total,
        positive,
        negative,
        neutral,
        mean_delta: if total == 0 {
            0.0
        } else {
            delta_sum / total as f32
        },
        promotable,
        demotable,
        by_domain,
    })
}

pub fn promote_safe() -> anyhow::Result<usize> {
    let ledger = PolicyShadowLedger::load_or_default(&shadow_ledger_path())?;
    let mut state = OperationalPolicyState::load_or_default(&policy_state_path())?;
    let mut changed = 0;
    for result in ledger.results.iter().filter(|r| r.safe_to_promote) {
        if let Some(rule_id) = &result.rule_id {
            if let Some(rule) = state.rules.iter_mut().find(|r| &r.id == rule_id) {
                rule.enabled = true;
                rule.confidence = (rule.confidence + result.delta.max(0.0) * 0.1).clamp(0.0, 1.0);
                changed += 1;
            }
        }
    }
    state.save(&policy_state_path())?;
    Ok(changed)
}

pub fn demote_bad() -> anyhow::Result<usize> {
    let ledger = PolicyShadowLedger::load_or_default(&shadow_ledger_path())?;
    let mut state = OperationalPolicyState::load_or_default(&policy_state_path())?;
    let mut changed = 0;
    for result in ledger.results.iter().filter(|r| r.should_demote) {
        if let Some(rule_id) = &result.rule_id {
            if let Some(rule) = state.rules.iter_mut().find(|r| &r.id == rule_id) {
                rule.confidence = (rule.confidence + result.delta * 0.2).clamp(0.0, 1.0);
                if rule.confidence < state.min_confidence {
                    rule.enabled = false;
                }
                changed += 1;
            }
        }
    }
    state.save(&policy_state_path())?;
    Ok(changed)
}

pub fn render_shadow_report() -> anyhow::Result<String> {
    let r = report()?;
    Ok(format!(
        "# Policy Shadow Simulation Report\n\nResults: `{}`\nPositive: `{}`\nNegative: `{}`\nNeutral: `{}`\nMean delta: `{:.3}`\nPromotable: `{}`\nDemotable: `{}`\n\n## By domain\n\n```json\n{}\n```\n",
        r.total_results,
        r.positive,
        r.negative,
        r.neutral,
        r.mean_delta,
        r.promotable,
        r.demotable,
        serde_json::to_string_pretty(&r.by_domain)?
    ))
}

fn simulate_sample(
    state: &OperationalPolicyState,
    sample: &RuntimeLearningSample,
) -> PolicyShadowResult {
    let domain = classify_sample_domain(sample);
    let target = sample.event.kind.clone();
    let baseline_score = score_outcome(&sample.event.outcome);
    score_decision(
        state,
        domain,
        target,
        sample.event.outcome.clone(),
        baseline_score,
        "runtime-sample".into(),
    )
}

fn simulate_live_event(
    state: &OperationalPolicyState,
    event: &crate::live_operational_fabric::LiveOperationalEvent,
) -> PolicyShadowResult {
    let domain = match event.kind {
        LiveEventKind::ProviderRequest | LiveEventKind::ProviderResponse => {
            PolicyDomain::ProviderChoice
        }
        LiveEventKind::TokenUsage | LiveEventKind::LocalTokenAbstraction => {
            PolicyDomain::ContextBudget
        }
        LiveEventKind::MemoryBridge => PolicyDomain::MemoryRetrieval,
        LiveEventKind::ToolStart | LiveEventKind::ToolResult => PolicyDomain::ToolSelection,
        LiveEventKind::UserMessage => PolicyDomain::Introspection,
        LiveEventKind::BackgroundCycle | LiveEventKind::System => PolicyDomain::DriftControl,
    };
    let baseline_score = score_outcome(&event.outcome);
    score_decision(
        state,
        domain,
        event.source.clone(),
        event.outcome.clone(),
        baseline_score,
        "live-fabric".into(),
    )
}

fn score_decision(
    state: &OperationalPolicyState,
    domain: PolicyDomain,
    target: String,
    baseline_outcome: String,
    baseline_score: f32,
    source: String,
) -> PolicyShadowResult {
    let rule = state
        .rules
        .iter()
        .filter(|r| r.enabled && r.domain == domain && r.confidence >= state.min_confidence)
        .max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let (rule_id, action, simulated_score, reason) = if let Some(rule) = rule {
        let boost = match rule.action {
            PolicyAction::RequireValidation => 0.25,
            PolicyAction::Prefer => 0.15,
            PolicyAction::Downrank | PolicyAction::CapBudget => {
                if baseline_score < 0.5 {
                    0.20
                } else {
                    0.02
                }
            }
            PolicyAction::Suppress => {
                if baseline_score < 0.0 {
                    0.25
                } else {
                    -0.20
                }
            }
            PolicyAction::ForceReplay | PolicyAction::RequireAudit => 0.10,
            PolicyAction::Allow | PolicyAction::ObserveOnly => 0.0,
        } * rule.confidence;
        (
            Some(rule.id.clone()),
            rule.action.clone(),
            (baseline_score + boost).clamp(-1.0, 1.0),
            format!("shadow matched {}", rule.id),
        )
    } else {
        (
            None,
            PolicyAction::Allow,
            baseline_score,
            "no matching policy".into(),
        )
    };
    let delta = simulated_score - baseline_score;
    PolicyShadowResult {
        schema_version: POLICY_SHADOW_SCHEMA_VERSION,
        source,
        domain,
        target,
        rule_id,
        action,
        baseline_outcome,
        baseline_score,
        simulated_score,
        delta,
        safe_to_promote: delta > 0.05,
        should_demote: delta < -0.05,
        reason,
    }
}

fn classify_sample_domain(sample: &RuntimeLearningSample) -> PolicyDomain {
    let lower = sample.event.kind.to_ascii_lowercase();
    if sample
        .event
        .tags
        .iter()
        .any(|t| t == "test" || t == "validation")
    {
        PolicyDomain::TestValidation
    } else if sample.event.tags.iter().any(|t| t == "token") {
        PolicyDomain::ContextBudget
    } else if sample
        .event
        .tags
        .iter()
        .any(|t| t == "memory" || t == "provenance")
    {
        PolicyDomain::MemoryRetrieval
    } else if lower.contains("provider") {
        PolicyDomain::ProviderChoice
    } else if lower.contains("tool") {
        PolicyDomain::ToolSelection
    } else {
        PolicyDomain::DriftControl
    }
}

fn load_runtime_samples() -> anyhow::Result<Vec<RuntimeLearningSample>> {
    let path = learning_dir().join("samples.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(path)?;
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(serde_json::from_str(&line)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operational_policy::{OperationalPolicyRule, PolicyWiringStatus};
    use tempfile::TempDir;

    #[test]
    fn shadow_simulation_promotes_positive_policy() {
        let dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("KCODE_POLICY_SHADOW_LEDGER", dir.path().join("shadow.json")) };
        let mut state = OperationalPolicyState::default();
        state.rules.push(OperationalPolicyRule {
            id: "r1".into(),
            domain: PolicyDomain::TestValidation,
            action: PolicyAction::RequireValidation,
            condition: "test".into(),
            confidence: 0.9,
            support: 1,
            source_memory_id: None,
            enabled: true,
            wiring_status: PolicyWiringStatus::ActiveRuntimeHook,
        });
        let result = score_decision(
            &state,
            PolicyDomain::TestValidation,
            "final".into(),
            "success".into(),
            0.25,
            "test".into(),
        );
        assert!(result.safe_to_promote);
    }
}
