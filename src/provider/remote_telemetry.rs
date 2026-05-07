use serde_json::json;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn telemetry_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".kcode")
        .join("remote-provider-requests.jsonl")
}

fn turn_trace_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".kcode")
        .join("turn-trace.jsonl")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

thread_local! {
    /// Thread-local turn id. Set by `with_turn_id` for the duration of a turn so
    /// that telemetry events emitted from deep call stacks (SSE handlers,
    /// background sinks) can be correlated to the originating turn without
    /// threading the id through every signature.
    static CURRENT_TURN_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Construct a sortable per-turn id (`<ms_hex>-<uuid_short>`).
///
/// Sortable by timestamp prefix; collision-resistant via uuid v4 suffix. We
/// avoid pulling a dedicated `ulid` dep — the existing `uuid` crate is already
/// in Cargo.toml.
pub fn new_turn_id() -> String {
    let ms = now_ms();
    let uid = Uuid::new_v4();
    let short = format!("{}", uid.simple());
    format!("{:013x}-{}", ms, &short[..12])
}

/// Returns the active turn id (if any) for the current thread.
pub fn current_turn_id() -> Option<String> {
    CURRENT_TURN_ID.with(|cell| cell.borrow().clone())
}

/// Run `body` with `turn_id` installed as the active turn id for the calling
/// thread. The previous value is restored on exit, even on panic.
pub fn with_turn_id<R>(turn_id: &str, body: impl FnOnce() -> R) -> R {
    let _guard = TurnIdGuard::install(turn_id);
    body()
}

/// RAII guard that installs `turn_id` as the active per-thread turn id and
/// restores the previous value on drop. Useful when the scope of a turn spans
/// multiple statements and a closure-based helper would force an awkward
/// indentation jump.
#[must_use]
pub struct TurnIdGuard {
    previous: Option<String>,
}

impl TurnIdGuard {
    pub fn install(turn_id: &str) -> Self {
        let previous = CURRENT_TURN_ID.with(|cell| cell.replace(Some(turn_id.to_string())));
        Self { previous }
    }
}

impl Drop for TurnIdGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT_TURN_ID.with(|cell| *cell.borrow_mut() = previous);
    }
}

pub(crate) fn log_request(provider: &str, model: &str, endpoint: &str, body_bytes: usize) {
    let record = json!({
        "timestamp_ms": now_ms(),
        "event": "request",
        "turn_id": current_turn_id(),
        "provider": provider,
        "model": model,
        "endpoint": endpoint,
        "serialized_http_request_body_bytes": body_bytes,
        "serialized_http_request_body_chars": body_bytes,
        "excludes_local_sidecar_prompt": true,
    });
    append(record);
}

pub(crate) fn log_usage(
    provider: &str,
    model: &str,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
) {
    let record = json!({
        "timestamp_ms": now_ms(),
        "event": "usage",
        "turn_id": current_turn_id(),
        "provider": provider,
        "model": model,
        "provider_reported_input_tokens": input_tokens,
        "provider_reported_output_tokens": output_tokens,
        "provider_reported_total_tokens": total_tokens,
    });
    append(record);
}

/// Component-level snapshot of the upstream payload composition. Built by the
/// turn loop after admission and admission-tier compaction so the *actual*
/// per-component sizes (post-interlang, post-short-turn-diet) are recorded.
#[derive(Debug, Clone, Default)]
pub struct PayloadAccounting {
    pub admission: Option<&'static str>,
    pub provider: String,
    pub model: Option<String>,
    pub system_static_chars: usize,
    pub system_dynamic_chars: usize,
    pub messages_chars: usize,
    pub messages_text_chars: usize,
    pub messages_tool_use_chars: usize,
    pub messages_tool_result_chars: usize,
    pub messages_reasoning_chars: usize,
    pub messages_image_chars: usize,
    pub tools_json_chars: usize,
    pub tools_count: usize,
    pub interlang_refs_chars: usize,
    pub interlang_refs_blocks: usize,
    pub memory_inject_chars: usize,
    pub memory_inject_count: usize,
    pub memory_anchor_chars: usize,
    pub sidecar_prompt_chars: Option<usize>,
    pub compacted_short_turn: bool,
    pub top_blocks: Vec<TopBlockEntry>,
    pub provider_context_window: Option<usize>,
    pub locked_tools_cached_chars: Option<usize>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TopBlockEntry {
    pub kind: &'static str,
    pub chars: usize,
    pub hash: String,
    pub role: &'static str,
    pub message_index: usize,
}

pub(crate) fn log_pre_provider_payload(
    provider: &str,
    messages: &[crate::message::Message],
    tools: &[crate::message::ToolDefinition],
    static_prompt: &str,
    dynamic_prompt: &str,
    compacted_short_turn: bool,
) {
    log_pre_provider_payload_with_accounting(
        provider,
        messages,
        tools,
        static_prompt,
        dynamic_prompt,
        compacted_short_turn,
        None,
    );
}

/// Like `log_pre_provider_payload`, but accepts a pre-computed
/// `PayloadAccounting` snapshot from the turn loop. When `Some`, this skips
/// the per-block scans below (which the caller already did) and emits the
/// detailed component-attribution record.
pub(crate) fn log_pre_provider_payload_with_accounting(
    provider: &str,
    messages: &[crate::message::Message],
    tools: &[crate::message::ToolDefinition],
    static_prompt: &str,
    dynamic_prompt: &str,
    compacted_short_turn: bool,
    accounting: Option<&PayloadAccounting>,
) {
    let messages_chars = accounting
        .map(|a| a.messages_chars)
        .unwrap_or_else(|| message_chars(messages));
    let tools_json_chars = accounting
        .and_then(|a| a.locked_tools_cached_chars)
        .or_else(|| accounting.map(|a| a.tools_json_chars))
        .unwrap_or_else(|| crate::message::ToolDefinition::aggregate_prompt_chars(tools));
    let interlang_refs_chars = accounting
        .map(|a| a.interlang_refs_chars)
        .unwrap_or_else(|| {
            messages
                .iter()
                .flat_map(|message| message.content.iter())
                .map(content_text)
                .map(|text| count_ref_chars(&text))
                .sum::<usize>()
        });
    let memory_inject_chars = accounting
        .map(|a| a.memory_inject_chars)
        .unwrap_or_else(|| {
            if dynamic_prompt.contains("memory") || dynamic_prompt.contains("Memory") {
                dynamic_prompt.len()
            } else {
                0
            }
        });
    let memory_anchor_chars = accounting.map(|a| a.memory_anchor_chars).unwrap_or(0);
    let final_request_json_chars = static_prompt
        .len()
        .saturating_add(dynamic_prompt.len())
        .saturating_add(messages_chars)
        .saturating_add(tools_json_chars)
        .saturating_add(256);
    let record = json!({
        "timestamp_ms": now_ms(),
        "event": "pre_provider_payload",
        "turn_id": current_turn_id(),
        "provider": provider,
        "model": accounting.and_then(|a| a.model.clone()),
        "admission": accounting.and_then(|a| a.admission),
        "system_prompt_chars": static_prompt.len(),
        "developer_prompt_chars": dynamic_prompt.len(),
        "messages_chars": messages_chars,
        "messages_text_chars": accounting.map(|a| a.messages_text_chars),
        "messages_tool_use_chars": accounting.map(|a| a.messages_tool_use_chars),
        "messages_tool_result_chars": accounting.map(|a| a.messages_tool_result_chars),
        "messages_reasoning_chars": accounting.map(|a| a.messages_reasoning_chars),
        "messages_image_chars": accounting.map(|a| a.messages_image_chars),
        "tools_json_chars": tools_json_chars,
        "tools_count": accounting.map(|a| a.tools_count).unwrap_or(tools.len()),
        "memory_chars": memory_inject_chars,
        "memory_inject_count": accounting.map(|a| a.memory_inject_count).unwrap_or(0),
        "memory_anchor_chars": memory_anchor_chars,
        "interlang_refs_chars": interlang_refs_chars,
        "interlang_refs_blocks": accounting.map(|a| a.interlang_refs_blocks).unwrap_or(0),
        "sidecar_prompt_chars": accounting.and_then(|a| a.sidecar_prompt_chars),
        "final_request_json_chars_estimate": final_request_json_chars,
        "compact_provider_messages_for_short_turn_ran": compacted_short_turn,
        "provider_context_window": accounting.and_then(|a| a.provider_context_window),
        "top_blocks": accounting.map(|a| a.top_blocks.clone()),
        "excludes_local_sidecar_prompt": true,
    });
    append(record);
}

/// Emit a per-turn trace record summarising admission decisions, budget
/// allocations, and observed component sizes. Written to a separate file
/// (`turn-trace.jsonl`) so analytics consumers can join on `turn_id`.
pub fn log_turn_trace(record: serde_json::Value) {
    let mut record = record;
    if let Some(obj) = record.as_object_mut() {
        obj.entry("timestamp_ms".to_string())
            .or_insert_with(|| json!(now_ms()));
        obj.entry("turn_id".to_string())
            .or_insert_with(|| json!(current_turn_id()));
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(turn_trace_path())
    {
        let _ = writeln!(file, "{}", record);
    }
}

fn message_chars(messages: &[crate::message::Message]) -> usize {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(content_text)
        .map(|text| text.len())
        .sum()
}

fn content_text(block: &crate::message::ContentBlock) -> String {
    match block {
        crate::message::ContentBlock::Text { text, .. } => text.clone(),
        crate::message::ContentBlock::Reasoning { text } => text.clone(),
        crate::message::ContentBlock::ToolResult { content, .. } => content.clone(),
        crate::message::ContentBlock::ToolUse { name, input, .. } => {
            format!("{}{}", name, input)
        }
        crate::message::ContentBlock::Image { data, .. } => data.clone(),
        crate::message::ContentBlock::OpenAICompaction { encrypted_content } => {
            encrypted_content.clone()
        }
    }
}

fn count_ref_chars(text: &str) -> usize {
    let has_ref = ["<ctx ", "<il:seen", "<ctx_candidate", "<il:v1>"]
        .iter()
        .any(|needle| text.contains(*needle));
    if has_ref { text.len() } else { 0 }
}

fn append(record: serde_json::Value) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(telemetry_path())
    {
        let _ = writeln!(file, "{}", record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_id_format_is_sortable() {
        let a = new_turn_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_turn_id();
        // Format: <13 hex chars>-<12 hex chars>
        assert_eq!(a.len(), 13 + 1 + 12);
        assert_eq!(b.len(), 13 + 1 + 12);
        // Sortable by timestamp prefix
        assert!(a < b, "{} < {}", a, b);
    }

    #[test]
    fn with_turn_id_scopes_value() {
        assert_eq!(current_turn_id(), None);
        with_turn_id("abc", || {
            assert_eq!(current_turn_id().as_deref(), Some("abc"));
            with_turn_id("nested", || {
                assert_eq!(current_turn_id().as_deref(), Some("nested"));
            });
            assert_eq!(current_turn_id().as_deref(), Some("abc"));
        });
        assert_eq!(current_turn_id(), None);
    }

    #[test]
    fn count_ref_chars_does_not_double_count() {
        let text = "prefix <ctx id=foo /> middle <il:seen hash=bar /> suffix";
        // Should return the text length once even though both markers appear.
        assert_eq!(count_ref_chars(text), text.len());
    }
}
