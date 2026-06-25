use crate::latent_memory::{LatentMemoryBank, LatentMemoryUsefulnessReport, latent_memory_path};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const OPERATIONAL_POLICY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyDomain {
    ToolSelection,
    ProviderChoice,
    ContextBudget,
    TestValidation,
    MemoryRetrieval,
    DriftControl,
    ToolBudget,
    RepairStrategy,
    Replay,
    Introspection,
    RiskControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PolicyAction {
    Allow,
    Prefer,
    Downrank,
    RequireValidation,
    Suppress,
    ObserveOnly,
    CapBudget,
    ForceReplay,
    RequireAudit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PolicyWiringStatus {
    ActiveRuntimeHook,
    PolicyApiOnly,
    ReportOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalPolicyRule {
    pub id: String,
    pub domain: PolicyDomain,
    pub action: PolicyAction,
    pub condition: String,
    pub confidence: f32,
    pub support: u64,
    pub source_memory_id: Option<String>,
    pub enabled: bool,
    pub wiring_status: PolicyWiringStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyDecision {
    pub domain: PolicyDomain,
    pub action: PolicyAction,
    pub rule_id: Option<String>,
    pub allowed: bool,
    pub confidence: f32,
    pub reason: String,
    pub audit_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyInfluenceAudit {
    pub id: String,
    pub decision: PolicyDecision,
    pub target: String,
    pub outcome: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationalPolicyState {
    pub schema_version: u32,
    pub observe_only: bool,
    pub min_confidence: f32,
    pub rules: Vec<OperationalPolicyRule>,
    pub audits: Vec<PolicyInfluenceAudit>,
}

impl Default for OperationalPolicyState {
    fn default() -> Self {
        Self {
            schema_version: OPERATIONAL_POLICY_SCHEMA_VERSION,
            observe_only: false,
            min_confidence: 0.55,
            rules: Vec::new(),
            audits: Vec::new(),
        }
    }
}

impl OperationalPolicyState {
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

    pub fn decide(&mut self, domain: PolicyDomain, target: &str) -> PolicyDecision {
        let selected = self
            .rules
            .iter()
            .filter(|r| r.enabled && r.domain == domain && r.confidence >= self.min_confidence)
            .max_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let decision = if self.observe_only {
            PolicyDecision {
                domain,
                action: PolicyAction::ObserveOnly,
                rule_id: selected.map(|r| r.id.clone()),
                allowed: true,
                confidence: selected.map(|r| r.confidence).unwrap_or(0.0),
                reason: "policy observe-only mode".into(),
                audit_id: audit_id(),
            }
        } else if let Some(rule) = selected {
            let allowed = !matches!(rule.action, PolicyAction::Suppress);
            PolicyDecision {
                domain,
                action: rule.action.clone(),
                rule_id: Some(rule.id.clone()),
                allowed,
                confidence: rule.confidence,
                reason: format!("matched policy rule {} for {target}", rule.id),
                audit_id: audit_id(),
            }
        } else {
            PolicyDecision {
                domain,
                action: PolicyAction::Allow,
                rule_id: None,
                allowed: true,
                confidence: 0.0,
                reason: "no gated policy rule matched".into(),
                audit_id: audit_id(),
            }
        };
        self.audits.push(PolicyInfluenceAudit {
            id: decision.audit_id.clone(),
            decision: decision.clone(),
            target: target.into(),
            outcome: "pending".into(),
            timestamp_ms: now_ms(),
        });
        trim(&mut self.audits, 512);
        decision
    }

    pub fn record_outcome(&mut self, audit_id: &str, outcome: &str) {
        if let Some(audit) = self.audits.iter_mut().find(|a| a.id == audit_id) {
            audit.outcome = outcome.into();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicySynthesisReport {
    pub rule_count: usize,
    pub audit_count: usize,
    pub observe_only: bool,
    pub usefulness: LatentMemoryUsefulnessReport,
    pub rules_by_domain: BTreeMap<PolicyDomain, usize>,
    pub active_runtime_domains: Vec<PolicyDomain>,
    pub policy_api_domains: Vec<PolicyDomain>,
}

pub fn policy_state_path() -> PathBuf {
    std::env::var_os("NEURA_OPERATIONAL_POLICY_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".neura").join("operational_policy_state.json")
        })
}

pub fn synthesize_rules_from_latent_memory(
    state: &mut OperationalPolicyState,
    bank: &LatentMemoryBank,
) {
    for entry in &bank.entries {
        if entry.usefulness_score < state.min_confidence
            || (entry.influence_count == 0 && entry.support == 0)
        {
            continue;
        }
        let lower_summary = entry.summary.to_ascii_lowercase();
        let (domain, action, wiring_status) =
            if entry.tags.iter().any(|t| t == "test" || t == "validation") {
                (
                    PolicyDomain::TestValidation,
                    PolicyAction::RequireValidation,
                    PolicyWiringStatus::ActiveRuntimeHook,
                )
            } else if entry
                .tags
                .iter()
                .any(|t| t == "token" || t == "token-stream")
            {
                (
                    PolicyDomain::ContextBudget,
                    PolicyAction::CapBudget,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            } else if entry
                .tags
                .iter()
                .any(|t| t == "memory" || t == "provenance")
            {
                (
                    PolicyDomain::MemoryRetrieval,
                    PolicyAction::Prefer,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            } else if lower_summary.contains("provider") {
                (
                    PolicyDomain::ProviderChoice,
                    PolicyAction::Downrank,
                    PolicyWiringStatus::ActiveRuntimeHook,
                )
            } else if lower_summary.contains("tool") {
                (
                    PolicyDomain::ToolSelection,
                    PolicyAction::Prefer,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            } else if lower_summary.contains("repair") || lower_summary.contains("error") {
                (
                    PolicyDomain::RepairStrategy,
                    PolicyAction::Prefer,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            } else if lower_summary.contains("replay") {
                (
                    PolicyDomain::Replay,
                    PolicyAction::ForceReplay,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            } else if lower_summary.contains("risk") || lower_summary.contains("destructive") {
                (
                    PolicyDomain::RiskControl,
                    PolicyAction::RequireAudit,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            } else {
                (
                    PolicyDomain::DriftControl,
                    PolicyAction::Prefer,
                    PolicyWiringStatus::PolicyApiOnly,
                )
            };
        let id = format!("policy:{}", entry.id);
        let rule = OperationalPolicyRule {
            id: id.clone(),
            domain,
            action,
            condition: entry.ctx_block.clone(),
            confidence: entry.usefulness_score,
            support: entry.influence_count.max(entry.support),
            source_memory_id: Some(entry.id.clone()),
            enabled: true,
            wiring_status,
        };
        if let Some(existing) = state.rules.iter_mut().find(|r| r.id == id) {
            *existing = rule;
        } else {
            state.rules.push(rule);
        }
    }
}

pub fn load_policy_and_synthesize() -> anyhow::Result<OperationalPolicyState> {
    let mut state = OperationalPolicyState::load_or_default(&policy_state_path())?;
    let bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
    synthesize_rules_from_latent_memory(&mut state, &bank);
    state.save(&policy_state_path())?;
    Ok(state)
}

pub fn report() -> anyhow::Result<PolicySynthesisReport> {
    let state = load_policy_and_synthesize()?;
    let bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
    let mut rules_by_domain = BTreeMap::new();
    for rule in &state.rules {
        *rules_by_domain.entry(rule.domain.clone()).or_insert(0) += 1;
    }
    Ok(PolicySynthesisReport {
        rule_count: state.rules.len(),
        audit_count: state.audits.len(),
        observe_only: state.observe_only,
        usefulness: bank.usefulness_report(),
        rules_by_domain,
        active_runtime_domains: vec![PolicyDomain::ProviderChoice, PolicyDomain::TestValidation],
        policy_api_domains: vec![
            PolicyDomain::MemoryRetrieval,
            PolicyDomain::ContextBudget,
            PolicyDomain::ToolSelection,
            PolicyDomain::DriftControl,
            PolicyDomain::ToolBudget,
            PolicyDomain::RepairStrategy,
            PolicyDomain::Replay,
            PolicyDomain::Introspection,
            PolicyDomain::RiskControl,
        ],
    })
}

pub fn render_policy_report() -> anyhow::Result<String> {
    let r = report()?;
    Ok(format!(
        "# Operational Policy Influence Report\n\nRules: `{}`\nAudits: `{}`\nObserve-only: `{}`\nActive runtime domains: `{:?}`\nPolicy API domains: `{:?}`\nUseful latent attributions: `{}`\nImproved: `{}`\nDrift reduced: `{}`\n\n## Rules by domain\n\n```json\n{}\n```\n",
        r.rule_count,
        r.audit_count,
        r.observe_only,
        r.active_runtime_domains,
        r.policy_api_domains,
        r.usefulness.total_attributions,
        r.usefulness.improved_count,
        r.usefulness.drift_reduced,
        serde_json::to_string_pretty(&r.rules_by_domain)?
    ))
}

fn audit_id() -> String {
    format!("policy-{}", now_ms())
}
fn now_ms() -> u64 {
    crate::latent_operational_recurrence::now_ms()
}
fn trim<T>(items: &mut Vec<T>, max: usize) {
    if items.len() > max {
        items.drain(0..items.len() - max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latent_memory::{LatentMemoryEntry, LatentMemoryKind};
    use crate::latent_operational_recurrence::LatentVector;

    #[test]
    fn synthesizes_policy_from_useful_latent_memory() {
        let mut state = OperationalPolicyState::default();
        let mut bank = LatentMemoryBank::default();
        bank.entries.push(LatentMemoryEntry {
            id: "m1".into(),
            kind: LatentMemoryKind::ValidationDoctrine,
            summary: "validate".into(),
            ctx_block: "<ctx k=\"latent-memory\">validate</ctx>".into(),
            vector: LatentVector::default(),
            tags: vec!["test".into(), "validation".into()],
            confidence: 0.9,
            usefulness_score: 0.9,
            influence_count: 3,
            positive_outcomes: 3,
            negative_outcomes: 0,
            drift_reduction_total: 0.2,
            support: 3,
            last_seen_ms: 0,
        });
        synthesize_rules_from_latent_memory(&mut state, &bank);
        assert_eq!(state.rules.len(), 1);
        assert_eq!(
            state.decide(PolicyDomain::TestValidation, "finish").action,
            PolicyAction::RequireValidation
        );
    }
}
