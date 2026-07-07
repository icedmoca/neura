pub mod adaptive_cognition;
pub mod adversarial_eval;
pub mod agent;
pub mod ambient;
pub mod ambient_runner;
pub mod ambient_scheduler;
pub mod auth;
pub mod autonomous_improvement;
pub mod backend_work;
pub mod background;
pub mod browser;
pub mod build;
pub mod bus;
pub mod cache_tracker;
pub mod catchup;
pub mod channel;
pub mod cli;
pub mod compaction;
pub mod config;
pub mod copilot_usage;
pub mod dictation;
pub mod directive_memory;
#[cfg(feature = "embeddings")]
pub mod embedding;
#[cfg(not(feature = "embeddings"))]
pub mod embedding_stub;
pub mod evidence_ledger;
pub mod evidence_replay;
pub mod latent_learning;
pub mod latent_learning_background;
pub mod latent_memory;
pub mod latent_operational_recurrence;
pub mod long_horizon_pressure;
pub mod neura_ui;
pub mod patch_proposal;
pub mod self_model;
pub mod semantic_operational_layer;
#[cfg(not(feature = "embeddings"))]
pub use embedding_stub as embedding;
pub mod env;
pub mod gateway;
pub mod gmail;
pub mod goal;
pub mod id;
pub mod import;
pub mod interlang;
pub mod live_operational_fabric;
pub mod local_memory_sidecar;
pub mod local_model;
pub mod logging;
pub mod login_qr;
pub mod mcp;
pub mod memory;
pub mod memory_agent;
pub mod memory_eval;
pub mod memory_graph;
pub mod memory_log;
pub mod memory_types;
pub mod message;
pub mod neura_memory;
pub mod notifications;
pub mod operational_eval;
pub mod operational_policy;
pub mod operational_repair_learning;
pub mod perf;
pub mod plan;
pub mod platform;
pub mod policy_outcome_credit;
pub mod policy_runtime;
pub mod policy_shadow_simulation;
pub mod process_memory;
pub mod process_title;
pub mod prompt;
pub mod protocol;
pub mod provider;
pub mod provider_catalog;
pub mod registry;
pub mod replay;
pub mod restart_snapshot;
pub mod runtime_memory_log;
pub mod safety;
pub mod self_improvement;
pub mod server;
pub mod session;
pub mod setup_hints;
pub mod side_panel;
pub mod sidecar;
pub mod skill;
pub mod soft_interrupt_store;
pub mod startup_profile;
pub mod stdin_detect;
pub mod storage;
pub mod subscription_catalog;
pub mod subtext_client;
pub mod telegram;
pub mod telemetry;
pub mod todo;
pub mod token_abstraction;
pub mod tool;
pub mod transport;
pub mod tui;
pub mod update;
pub mod usage;
pub mod util;
pub mod video_export;

use anyhow::Result;
use std::sync::Mutex;

static CURRENT_SESSION_ID: Mutex<Option<String>> = Mutex::new(None);

pub fn set_current_session(session_id: &str) {
    if let Ok(mut guard) = CURRENT_SESSION_ID.lock() {
        *guard = Some(session_id.to_string());
    }
}

pub fn get_current_session() -> Option<String> {
    CURRENT_SESSION_ID.lock().ok()?.clone()
}

pub async fn run() -> Result<()> {
    cli::startup::run().await
}
pub mod latency;
pub mod runtime_governor;
pub mod runtime_ledger;
pub mod self_improvement_daemon;
pub mod work_queue;
