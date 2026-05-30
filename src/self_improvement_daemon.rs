//! Safe self-improvement daemon supervisor.
//!
//! This module wires the existing autonomous improvement engine into a controlled
//! recurring loop. It is intentionally conservative: dry-run is the default, and
//! mutation is never enabled unless explicitly requested by configuration.

use crate::autonomous_improvement::{
    ImprovementConfig, SelfImprovementReport, run_self_improvement_cycle,
};
use crate::runtime_ledger::{self, RuntimeReceiptKind};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfImprovementDaemonConfig {
    pub enabled: bool,
    pub dry_run: bool,
    pub interval_secs: u64,
    pub allow_mutation: bool,
}

impl Default for SelfImprovementDaemonConfig {
    fn default() -> Self {
        Self {
            // Enabled by default so the runtime can continuously audit itself,
            // but still dry-run and non-mutating unless explicitly opted in.
            enabled: true,
            dry_run: true,
            interval_secs: 60 * 60,
            allow_mutation: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfImprovementTickResult {
    Disabled,
    SkippedCooldown { remaining_secs: u64 },
    Ran { tasks: usize, applied: usize },
}

#[derive(Debug)]
pub struct SelfImprovementDaemon {
    config: SelfImprovementDaemonConfig,
    last_run: Option<Instant>,
}

impl SelfImprovementDaemon {
    pub fn new(config: SelfImprovementDaemonConfig) -> Self {
        Self {
            config,
            last_run: None,
        }
    }

    pub fn config(&self) -> &SelfImprovementDaemonConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: SelfImprovementDaemonConfig) {
        self.config = config;
    }

    pub fn tick(&mut self) -> Result<SelfImprovementTickResult> {
        if !self.config.enabled {
            return Ok(SelfImprovementTickResult::Disabled);
        }

        if let Some(last_run) = self.last_run {
            let interval = Duration::from_secs(self.config.interval_secs);
            if let Some(remaining) = interval.checked_sub(last_run.elapsed()) {
                return Ok(SelfImprovementTickResult::SkippedCooldown {
                    remaining_secs: remaining.as_secs(),
                });
            }
        }

        let report = self.run_once()?;
        self.last_run = Some(Instant::now());
        let result = SelfImprovementTickResult::Ran {
            tasks: report
                .iterations
                .iter()
                .map(|it| it.candidate_actions.len())
                .sum::<usize>(),
            applied: report
                .iterations
                .iter()
                .map(|it| it.applied_actions.len())
                .sum::<usize>(),
        };
        self.record_receipt(&report, &result);
        Ok(result)
    }

    fn run_once(&self) -> Result<SelfImprovementReport> {
        let mut config = ImprovementConfig::default();
        config.dry_run = self.config.dry_run || !self.config.allow_mutation;
        run_self_improvement_cycle(config)
    }

    fn record_receipt(&self, report: &SelfImprovementReport, result: &SelfImprovementTickResult) {
        if !runtime_ledger::enabled() {
            return;
        }
        runtime_ledger::append_receipt_best_effort(
            RuntimeReceiptKind::BackendWork,
            "self_improvement_tick",
            serde_json::json!({
                "enabled": self.config.enabled,
                "dry_run": self.config.dry_run,
                "allow_mutation": self.config.allow_mutation,
                "tasks": report.iterations.iter().map(|it| it.candidate_actions.len()).sum::<usize>(),
                "applied": report.iterations.iter().map(|it| it.applied_actions.len()).sum::<usize>(),
                "result": format!("{:?}", result),
            }),
        );
    }
}

static GLOBAL_DAEMON: OnceLock<Mutex<SelfImprovementDaemon>> = OnceLock::new();

fn global_daemon() -> &'static Mutex<SelfImprovementDaemon> {
    GLOBAL_DAEMON.get_or_init(|| Mutex::new(SelfImprovementDaemon::new(config_from_env())))
}

pub fn config_from_env() -> SelfImprovementDaemonConfig {
    let mut config = SelfImprovementDaemonConfig::default();
    if let Ok(value) = std::env::var("KCODE_SELF_IMPROVEMENT") {
        config.enabled = matches!(value.as_str(), "1" | "true" | "on" | "yes");
    }
    if let Ok(value) = std::env::var("KCODE_SELF_IMPROVEMENT_DRY_RUN") {
        config.dry_run = !matches!(value.as_str(), "0" | "false" | "off" | "no");
    }
    if let Ok(value) = std::env::var("KCODE_SELF_IMPROVEMENT_ALLOW_MUTATION") {
        config.allow_mutation = matches!(value.as_str(), "1" | "true" | "on" | "yes");
    }
    if let Ok(value) = std::env::var("KCODE_SELF_IMPROVEMENT_INTERVAL_SECS")
        && let Ok(seconds) = value.parse::<u64>()
    {
        config.interval_secs = seconds.max(60);
    }
    config
}

pub fn current_config() -> SelfImprovementDaemonConfig {
    global_daemon()
        .lock()
        .map(|daemon| daemon.config().clone())
        .unwrap_or_default()
}

pub fn set_global_config(config: SelfImprovementDaemonConfig) {
    if let Ok(mut daemon) = global_daemon().lock() {
        daemon.set_config(config);
    }
}

pub fn tick_global() -> Result<SelfImprovementTickResult> {
    let mut daemon = global_daemon()
        .lock()
        .map_err(|_| anyhow::anyhow!("self-improvement daemon lock poisoned"))?;
    daemon.tick()
}

pub fn format_config(config: &SelfImprovementDaemonConfig) -> String {
    format!(
        "self-improvement: enabled={} dry_run={} allow_mutation={} interval_secs={}",
        config.enabled, config.dry_run, config.allow_mutation, config.interval_secs
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_but_safe() {
        let config = SelfImprovementDaemonConfig::default();
        assert!(config.enabled);
        assert!(config.dry_run);
        assert!(!config.allow_mutation);
        assert_eq!(config.interval_secs, 60 * 60);
    }

    #[test]
    fn disabled_daemon_does_not_run() {
        let mut config = SelfImprovementDaemonConfig::default();
        config.enabled = false;
        let mut daemon = SelfImprovementDaemon::new(config);
        assert_eq!(daemon.tick().unwrap(), SelfImprovementTickResult::Disabled);
    }

    #[test]
    fn formats_config() {
        let text = format_config(&SelfImprovementDaemonConfig::default());
        assert!(text.contains("self-improvement"));
        assert!(text.contains("dry_run=true"));
    }
}
