use crate::latent_learning_background::{
    command_event, ingest_runtime_event, run_background_cycle,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

pub const FABRIC_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum LiveEventKind {
    UserMessage,
    ProviderRequest,
    ProviderResponse,
    ToolStart,
    ToolResult,
    TokenUsage,
    LocalTokenAbstraction,
    MemoryBridge,
    BackgroundCycle,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveOperationalEvent {
    pub schema_version: u32,
    pub id: String,
    pub kind: LiveEventKind,
    pub outcome: String,
    pub source: String,
    pub tags: Vec<String>,
    pub magnitude: f32,
    pub token_input: Option<u64>,
    pub token_output: Option<u64>,
    pub token_ids_preview: Vec<u32>,
    pub token_text_preview: Vec<String>,
    pub payload_chars: usize,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FabricStatus {
    pub enabled: bool,
    pub paused: bool,
    pub total_events: usize,
    pub pending_background_samples: usize,
    pub counts_by_kind: BTreeMap<String, usize>,
    pub token_input_total: u64,
    pub token_output_total: u64,
    pub state_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FabricFusionReport {
    pub status: FabricStatus,
    pub last_events: Vec<LiveOperationalEvent>,
    pub latent_background: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FabricControl {
    paused: bool,
}
impl Default for FabricControl {
    fn default() -> Self {
        Self { paused: false }
    }
}

pub fn fabric_dir() -> PathBuf {
    std::env::var_os("KCODE_LIVE_FABRIC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".kcode").join("live_operational_fabric")
        })
}

pub fn emit(mut event: LiveOperationalEvent) -> anyhow::Result<()> {
    if !enabled() || paused()? {
        return Ok(());
    }
    event.schema_version = FABRIC_SCHEMA_VERSION;
    if event.timestamp_ms == 0 {
        event.timestamp_ms = crate::latent_operational_recurrence::now_ms();
    }
    if event.id.is_empty() {
        event.id = format!(
            "{}-{:?}-{}",
            event.timestamp_ms,
            event.kind,
            sanitize(&event.source)
        );
    }
    append_event(&event)?;
    bridge_to_latent(&event)?;
    if should_auto_cycle() {
        let _ = run_background_cycle(8);
    }
    Ok(())
}

pub fn emit_user_message(source: &str, text: &str) {
    let _ = emit(basic(
        LiveEventKind::UserMessage,
        "observed",
        source,
        text.len(),
        vec!["user".into(), "memory".into()],
    ));
}
pub fn emit_provider_request(source: &str, message_count: usize, tool_count: usize) {
    let _ = emit(basic(
        LiveEventKind::ProviderRequest,
        "started",
        source,
        message_count + tool_count,
        vec!["provider".into(), "token".into()],
    ));
}
pub fn emit_provider_response(source: &str, text_chars: usize, tool_calls: usize) {
    let _ = emit(basic(
        LiveEventKind::ProviderResponse,
        "success",
        source,
        text_chars + tool_calls,
        vec!["provider".into()],
    ));
}
pub fn emit_tool_start(tool: &str) {
    let _ = emit(basic(
        LiveEventKind::ToolStart,
        "started",
        tool,
        tool.len(),
        vec!["tool".into()],
    ));
}
pub fn emit_tool_result(tool: &str, ok: bool, chars: usize) {
    let _ = emit(basic(
        LiveEventKind::ToolResult,
        if ok { "success" } else { "failure" },
        tool,
        chars,
        vec![
            "tool".into(),
            if ok {
                "validation".into()
            } else {
                "error".into()
            },
        ],
    ));
}
pub fn emit_token_usage(source: &str, input: u64, output: u64) {
    let mut e = basic(
        LiveEventKind::TokenUsage,
        "observed",
        source,
        (input + output) as usize,
        vec!["token".into()],
    );
    e.token_input = Some(input);
    e.token_output = Some(output);
    let _ = emit(e);
}
pub fn emit_local_token_abstraction(source: &str, text: &str) {
    let rec = crate::token_abstraction::tokenize_text(text);
    let outcome = match rec.method {
        crate::token_abstraction::TokenizationMethod::HuggingFaceTokenizer => "tokenized",
        crate::token_abstraction::TokenizationMethod::DeterministicEstimate => "estimated",
    };
    let mut e = basic(
        LiveEventKind::LocalTokenAbstraction,
        outcome,
        source,
        rec.char_count,
        vec![
            "token".into(),
            "sidecar".into(),
            "local-model".into(),
            "token-stream".into(),
        ],
    );
    e.token_input = Some(rec.token_count);
    e.token_ids_preview = rec.token_ids_preview;
    e.token_text_preview = rec.token_text_preview;
    let _ = emit(e);
}
pub fn estimate_tokens(chars: usize) -> u64 {
    crate::token_abstraction::estimate_tokens(chars)
}
pub fn emit_memory_bridge(source: &str, chars: usize) {
    let _ = emit(basic(
        LiveEventKind::MemoryBridge,
        "observed",
        source,
        chars,
        vec!["memory".into(), "provenance".into()],
    ));
}

pub fn status() -> anyhow::Result<FabricStatus> {
    let events = load_events()?;
    let mut counts = BTreeMap::new();
    let mut tin = 0;
    let mut tout = 0;
    for e in &events {
        *counts.entry(format!("{:?}", e.kind)).or_insert(0) += 1;
        tin += e.token_input.unwrap_or(0);
        tout += e.token_output.unwrap_or(0);
    }
    let bg = crate::latent_learning_background::status()?;
    Ok(FabricStatus {
        enabled: enabled(),
        paused: paused()?,
        total_events: events.len(),
        pending_background_samples: bg.pending_samples,
        counts_by_kind: counts,
        token_input_total: tin,
        token_output_total: tout,
        state_dir: fabric_dir(),
    })
}

pub fn report() -> anyhow::Result<FabricFusionReport> {
    let mut events = load_events()?;
    let len = events.len();
    let start = len.saturating_sub(20);
    let last = events.drain(start..).collect();
    let bg = serde_json::to_value(crate::latent_learning_background::status()?)?;
    Ok(FabricFusionReport {
        status: status()?,
        last_events: last,
        latent_background: bg,
    })
}

pub fn render_markdown_report() -> anyhow::Result<String> {
    let r = report()?;
    Ok(format!(
        "# Live Operational Fabric Report\n\nTotal events: `{}`\nPaused: `{}`\nPending latent samples: `{}`\nToken input total: `{}`\nToken output total: `{}`\n\n## Counts by kind\n\n```json\n{}\n```\n\n## Last events\n\n```json\n{}\n```\n",
        r.status.total_events,
        r.status.paused,
        r.status.pending_background_samples,
        r.status.token_input_total,
        r.status.token_output_total,
        serde_json::to_string_pretty(&r.status.counts_by_kind)?,
        serde_json::to_string_pretty(&r.last_events)?
    ))
}

pub fn set_paused(paused_value: bool) -> anyhow::Result<FabricStatus> {
    fs::create_dir_all(fabric_dir())?;
    fs::write(
        control_path(),
        serde_json::to_string_pretty(&FabricControl {
            paused: paused_value,
        })?,
    )?;
    status()
}
pub fn events() -> anyhow::Result<Vec<LiveOperationalEvent>> {
    load_events()
}

fn basic(
    kind: LiveEventKind,
    outcome: &str,
    source: &str,
    payload_chars: usize,
    tags: Vec<String>,
) -> LiveOperationalEvent {
    LiveOperationalEvent {
        schema_version: FABRIC_SCHEMA_VERSION,
        id: String::new(),
        kind,
        outcome: outcome.into(),
        source: source.into(),
        tags,
        magnitude: 1.0,
        token_input: None,
        token_output: None,
        token_ids_preview: Vec::new(),
        token_text_preview: Vec::new(),
        payload_chars,
        timestamp_ms: 0,
    }
}
fn enabled() -> bool {
    std::env::var("KCODE_LIVE_FABRIC")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(true)
}
fn should_auto_cycle() -> bool {
    std::env::var("KCODE_LIVE_FABRIC_AUTO_CYCLE")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
}
fn paused() -> anyhow::Result<bool> {
    Ok(if control_path().exists() {
        serde_json::from_str::<FabricControl>(&fs::read_to_string(control_path())?)?.paused
    } else {
        false
    })
}
fn append_event(e: &LiveOperationalEvent) -> anyhow::Result<()> {
    fs::create_dir_all(fabric_dir())?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path())?;
    writeln!(f, "{}", serde_json::to_string(e)?)?;
    Ok(())
}
fn load_events() -> anyhow::Result<Vec<LiveOperationalEvent>> {
    let p = events_path();
    if !p.exists() {
        return Ok(vec![]);
    };
    let f = fs::File::open(p)?;
    let mut out = vec![];
    for line in BufReader::new(f).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(serde_json::from_str(&line)?);
        }
    }
    Ok(out)
}
fn bridge_to_latent(e: &LiveOperationalEvent) -> anyhow::Result<()> {
    let mut tags = e.tags.clone();
    tags.push("live-fabric".into());
    let mut ev = command_event(
        format!("live::{:?}", e.kind),
        e.outcome.clone(),
        tags,
        Some(e.source.clone()),
    );
    ev.weight = e.magnitude.max(0.1);
    ingest_runtime_event(ev, "live-operational-fabric")?;
    Ok(())
}
fn events_path() -> PathBuf {
    fabric_dir().join("events.jsonl")
}
fn control_path() -> PathBuf {
    fabric_dir().join("control.json")
}
fn sanitize(v: &str) -> String {
    v.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn emits_status_and_bridges() {
        let d = TempDir::new().unwrap();
        unsafe { std::env::set_var("KCODE_LIVE_FABRIC_DIR", d.path().join("fabric")) };
        unsafe { std::env::set_var("KCODE_LATENT_LEARNING_DIR", d.path().join("learning")) };
        emit_user_message("test", "hello");
        let s = status().unwrap();
        assert_eq!(s.total_events, 1);
        assert_eq!(s.pending_background_samples, 1);
    }
}
